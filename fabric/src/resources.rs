//! Linux observations shared by the services that apply elastic resource budgets.

use admission_kernel::telemetry::elastic_spendable;
use serde::Serialize;
use std::{
    fs, io,
    path::{Component, Path},
};

const CGROUP_ROOT: &str = "/sys/fs/cgroup";
const PROCESS_CGROUP: &str = "/proc/self/cgroup";

#[derive(Default)]
pub struct MemoryPressure {
    sample: Option<WorkingSet>,
    pub limit_bytes: Option<u64>,
    pub pressured: bool,
}

#[derive(Serialize)]
struct WorkingSet {
    working_set_bytes: u64,
    pressure_threshold_bytes: u64,
}

fn number(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

// Inactive file pages can be reclaimed by the kernel under pressure.
fn inactive_file_bytes(directory: &Path) -> u64 {
    fs::read_to_string(directory.join("memory.stat"))
        .ok()
        .and_then(|stat| {
            stat.lines()
                .find_map(|line| line.strip_prefix("inactive_file "))
                .and_then(|bytes| bytes.trim().parse().ok())
        })
        .unwrap_or(0)
}

fn memory_inputs(root: &Path, membership: &str) -> MemoryPressure {
    let Some(path) = membership.lines().find_map(|line| line.strip_prefix("0::")) else {
        return MemoryPressure::default();
    };
    let path = Path::new(path);
    let Ok(relative) = path.strip_prefix("/") else {
        return MemoryPressure::default();
    };
    if relative
        .components()
        .any(|part| part == Component::ParentDir)
    {
        return MemoryPressure::default();
    }
    let mut inputs = MemoryPressure::default();
    for directory in relative.ancestors() {
        let directory = root.join(directory);
        let limit = ["memory.high", "memory.max"]
            .iter()
            .filter_map(|file| number(&directory.join(file)))
            .min();
        if let Some(limit) = limit {
            inputs.limit_bytes = Some(inputs.limit_bytes.map_or(limit, |prior| prior.min(limit)));
            if let Some(current) = number(&directory.join("memory.current")) {
                let working_set_bytes = current.saturating_sub(inactive_file_bytes(&directory));
                let pressure_threshold_bytes = elastic_spendable(limit);
                let pressured =
                    working_set_bytes > 0 && working_set_bytes >= pressure_threshold_bytes;
                if inputs.sample.is_none() || (pressured && !inputs.pressured) {
                    inputs.sample = Some(WorkingSet {
                        working_set_bytes,
                        pressure_threshold_bytes,
                    });
                }
                inputs.pressured |= pressured;
            }
        }
    }
    inputs
}

pub fn process_memory() -> MemoryPressure {
    let membership = fs::read_to_string(PROCESS_CGROUP).unwrap_or_default();
    memory_inputs(Path::new(CGROUP_ROOT), &membership)
}

pub const DEFAULT_RECEIVER_CONNECTIONS: usize = 16;
pub const MAX_RECEIVER_CONNECTIONS: usize = 32;
pub const RECEIVER_PRESSURE_SAMPLE_MS: u64 = 100;

#[derive(Serialize)]
struct DescriptorBudget {
    soft_limit: u64,
    spendable: u64,
    // The directory scan itself consumes one descriptor for turnover.
    open_descriptors: u64,
}

impl DescriptorBudget {
    fn pressure(&self) -> bool {
        self.open_descriptors >= self.spendable
    }
}

fn descriptor_budget() -> io::Result<Option<DescriptorBudget>> {
    use rustix::process::{Resource, getrlimit};
    let Some(soft_limit) = getrlimit(Resource::Nofile).current else {
        return Ok(None);
    };
    let open_descriptors = fs::read_dir("/proc/self/fd")?
        .try_fold(0_u64, |open, entry| entry.map(|_| open.saturating_add(1)))?;
    Ok(Some(DescriptorBudget {
        soft_limit,
        spendable: elastic_spendable(soft_limit),
        open_descriptors,
    }))
}

/// Keep a single connection trial if the descriptor budget is exhausted.
fn connection_cap(
    configured: usize,
    sample: &io::Result<Option<DescriptorBudget>>,
) -> (usize, &'static str) {
    match sample {
        Ok(Some(budget)) => {
            let available = budget
                .spendable
                .saturating_sub(budget.open_descriptors)
                .max(1);
            let cap = configured.min(usize::try_from(available).unwrap_or(usize::MAX));
            (cap, "rlimit_nofile_and_proc_fd")
        }
        Err(error) if is_resource_error(error) => (configured.min(1), "descriptor_resource_error"),
        _ => (configured, "configured"),
    }
}

pub fn connection_limit(configured: usize) -> usize {
    connection_cap(configured, &descriptor_budget()).0
}

/// Linux cannot safely lend more descriptors to optional work at this boundary.
pub fn descriptor_pressure() -> bool {
    match descriptor_budget() {
        Ok(budget) => budget.as_ref().is_some_and(DescriptorBudget::pressure),
        Err(error) => is_resource_error(&error),
    }
}

/// Values sampled in the process that emits the report.
#[derive(Serialize)]
pub struct ReceiverResources {
    configured_connection_limit: usize,
    pub connection_limit: usize,
    connection_limit_source: &'static str,
    descriptors: Option<DescriptorBudget>,
    descriptor_pressure: Option<bool>,
    descriptor_pressure_source: &'static str,
    cgroup_limit_bytes: Option<u64>,
    memory: Option<WorkingSet>,
    memory_pressure: Option<bool>,
    memory_pressure_source: &'static str,
    sample_interval_ms: u64,
}

pub fn receiver_resources(configured: usize) -> ReceiverResources {
    let sample = descriptor_budget();
    let (descriptor_pressure, descriptor_pressure_source) = match &sample {
        Ok(Some(budget)) => (Some(budget.pressure()), "rlimit_nofile_and_proc_fd"),
        Err(error) if is_resource_error(error) => (Some(true), "descriptor_resource_error"),
        _ => (None, "missing"),
    };
    let (connection_limit, connection_limit_source) = connection_cap(configured, &sample);
    let descriptors = sample.ok().flatten();
    let memory = process_memory();
    let memory_sampled = memory.sample.is_some();
    ReceiverResources {
        configured_connection_limit: configured,
        connection_limit,
        connection_limit_source,
        descriptors,
        descriptor_pressure,
        descriptor_pressure_source,
        cgroup_limit_bytes: memory.limit_bytes,
        memory: memory.sample,
        memory_pressure: memory_sampled.then_some(memory.pressured),
        memory_pressure_source: if memory_sampled {
            "cgroup_v2_working_set"
        } else {
            "missing"
        },
        sample_interval_ms: RECEIVER_PRESSURE_SAMPLE_MS,
    }
}

pub fn is_resource_error(error: &io::Error) -> bool {
    use rustix::io::Errno;
    [Errno::MFILE, Errno::NFILE, Errno::NOBUFS, Errno::NOMEM]
        .iter()
        .any(|errno| error.raw_os_error() == Some(errno.raw_os_error()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_descriptor_exhaustion_keeps_only_one_trial() {
        let exhausted = Err(rustix::io::Errno::MFILE.into());
        assert_eq!(
            connection_cap(32, &exhausted),
            (1, "descriptor_resource_error")
        );
        let missing = Err(rustix::io::Errno::NOENT.into());
        assert_eq!(connection_cap(32, &missing), (32, "configured"));
        assert_eq!(connection_cap(32, &Ok(None)), (32, "configured"));
    }

    #[test]
    fn reads_limits_above_deep_cgroup_descendants() {
        let temp = tempfile::tempdir().unwrap();
        let relative = format!("slice/{}service", "child/".repeat(40));
        let root = temp.path();
        fs::create_dir_all(root.join(&relative)).unwrap();
        fs::write(root.join("slice/memory.max"), "400").unwrap();
        fs::write(root.join("slice/memory.current"), "300").unwrap();
        let inputs = memory_inputs(root, &format!("0::/{relative}"));
        assert_eq!(inputs.limit_bytes, Some(400));
        assert!(inputs.pressured);
    }

    #[test]
    fn resolves_ancestor_limits_and_pressure_separately() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("slice/service")).unwrap();
        for (file, value) in [
            ("memory.max", "max"),
            ("slice/memory.high", "67108864"),
            ("slice/memory.max", "134217728"),
            ("slice/memory.current", "50331648"),
            ("slice/service/memory.max", "268435456"),
            ("slice/service/memory.current", "2097152"),
        ] {
            fs::write(root.join(file), value).unwrap();
        }
        let inputs = memory_inputs(root, "0::/slice/service\n");
        assert!(inputs.pressured);
        assert_eq!(inputs.limit_bytes, Some(64 * 1024 * 1024));
        fs::write(root.join("slice/memory.current"), "50331647").unwrap();
        assert!(!memory_inputs(root, "0::/slice/service").pressured);
        fs::write(root.join("slice/memory.current"), "65000000").unwrap();
        fs::write(
            root.join("slice/memory.stat"),
            "anon 8388608\ninactive_file 40000000\n",
        )
        .unwrap();
        assert!(!memory_inputs(root, "0::/slice/service").pressured);
        assert_eq!(memory_inputs(root, "0::/../slice").limit_bytes, None);
    }
}
