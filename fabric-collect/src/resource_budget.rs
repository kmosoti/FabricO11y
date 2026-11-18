//! Linux observations for the verified elastic memory policy.

use admission_kernel::telemetry::elastic_spendable;
use fabric_core::DEFAULT_RING_BUDGET_BYTES;
use std::{
    fs,
    path::{Component, Path},
};

const CGROUP_ROOT: &str = "/sys/fs/cgroup";
const PROCESS_CGROUP: &str = "/proc/self/cgroup";

/// The configured hot cache budget and the Linux limit used to derive it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingBudget {
    pub budget_bytes: usize,
    pub cgroup_limit_bytes: Option<u64>,
}

#[derive(Default)]
struct MemoryInputs {
    limit: Option<u64>,
    pressured: bool,
}

fn number(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn memory_inputs(root: &Path, membership: &str) -> MemoryInputs {
    let Some(path) = membership.lines().find_map(|line| line.strip_prefix("0::")) else {
        return MemoryInputs::default();
    };
    let path = Path::new(path);
    let Ok(relative) = path.strip_prefix("/") else {
        return MemoryInputs::default();
    };
    if relative
        .components()
        .any(|part| part == Component::ParentDir)
    {
        return MemoryInputs::default();
    }
    let mut inputs = MemoryInputs::default();
    for directory in relative.ancestors().take(32) {
        let directory = root.join(directory);
        let limit = ["memory.high", "memory.max"]
            .iter()
            .filter_map(|file| number(&directory.join(file)))
            .min();
        if let Some(limit) = limit {
            inputs.limit = Some(inputs.limit.map_or(limit, |prior| prior.min(limit)));
            if let Some(current) = number(&directory.join("memory.current")) {
                inputs.pressured |= current > 0 && current >= elastic_spendable(limit);
            }
        }
    }
    inputs
}

fn process_memory() -> MemoryInputs {
    let membership = fs::read_to_string(PROCESS_CGROUP).unwrap_or_default();
    memory_inputs(Path::new(CGROUP_ROOT), &membership)
}

fn ring_budget(limit: Option<u64>) -> RingBudget {
    let budget_bytes = limit.map_or(DEFAULT_RING_BUDGET_BYTES, |limit| {
        (elastic_spendable(limit) / 8).clamp(4096, DEFAULT_RING_BUDGET_BYTES as u64) as usize
    });
    RingBudget {
        budget_bytes,
        cgroup_limit_bytes: limit,
    }
}

/// Allocate one eighth of spendable cgroup memory to the optional hot ring.
#[must_use]
pub fn automatic_ring_budget() -> RingBudget {
    ring_budget(process_memory().limit)
}

pub(crate) fn cgroup_memory_pressure() -> bool {
    process_memory().pressured
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(inputs.limit, Some(64 * 1024 * 1024));
        assert_eq!(ring_budget(inputs.limit).budget_bytes, 6 * 1024 * 1024);
        fs::write(root.join("slice/memory.current"), "50331647").unwrap();
        assert!(!memory_inputs(root, "0::/slice/service").pressured);
        assert_eq!(memory_inputs(root, "0::/../slice").limit, None);
    }

    #[test]
    fn missing_and_extreme_limits_have_repeatable_defaults() {
        assert_eq!(ring_budget(None).budget_bytes, DEFAULT_RING_BUDGET_BYTES);
        assert_eq!(
            ring_budget(Some(u64::MAX)).budget_bytes,
            DEFAULT_RING_BUDGET_BYTES
        );
        assert_eq!(ring_budget(Some(1)).budget_bytes, 4096);
        assert_eq!(elastic_spendable(5), 3);
    }
}
