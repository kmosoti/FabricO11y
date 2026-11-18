//! Process self-preservation monitor enforcing RSS, FD, `/tmp`, and slice limits.

use fabric_core::model::{
    DoctorCheck, DoctorReport, HEALTHY_RSS_BUDGET_BYTES, MAX_OPEN_FDS_BUDGET, SelfHealthSummary,
    WATCHDOG_SHEDDING_RSS_BYTES,
};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Monitors the current binary or running daemon's own RSS, FDs, `/tmp` hygiene, and slice confinement.
#[derive(Debug, Clone)]
pub struct SelfPreservationGuard {
    data_dir: String,
    tmp_dir: String,
    dropped_events: Arc<AtomicU64>,
    self_shedding: Arc<AtomicBool>,
}

impl SelfPreservationGuard {
    #[must_use]
    pub fn new(
        data_dir: impl Into<String>,
        tmp_dir: impl Into<String>,
        dropped_events: Arc<AtomicU64>,
    ) -> Self {
        Self {
            data_dir: data_dir.into(),
            tmp_dir: tmp_dir.into(),
            dropped_events,
            self_shedding: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Reads `/proc/self/statm`, `/proc/self/fd`, and `/proc/self/cgroup` and updates shedding flag.
    #[must_use]
    pub fn inspect_summary(&self) -> SelfHealthSummary {
        let pid = std::process::id();
        let rss_bytes = read_self_rss_bytes();
        let (open_fds, tmp_files_created) = inspect_self_fds();
        let cgroup_path = read_self_cgroup_path();
        let fabrico11y_slice_confined = cgroup_path.contains("fabrico11y.slice");

        let shedding = admission_kernel::telemetry::rss_shedding_decision(
            rss_bytes,
            WATCHDOG_SHEDDING_RSS_BYTES,
        );
        self.self_shedding.store(shedding, Ordering::Relaxed);

        SelfHealthSummary {
            pid,
            rss_bytes,
            open_fds,
            tmp_files_created,
            cgroup_path,
            fabrico11y_slice_confined,
            dropped_events_total: self.dropped_events.load(Ordering::Relaxed),
            self_shedding_active: shedding || crate::resource_budget::cgroup_memory_pressure(),
        }
    }

    #[must_use]
    pub fn is_shedding(&self) -> bool {
        self.self_shedding.load(Ordering::Relaxed)
    }

    /// Builds a complete `DoctorReport` for `fabric doctor --json`.
    #[must_use]
    pub fn doctor_report(&self, daemon_running: bool) -> DoctorReport {
        let summary = self.inspect_summary();
        build_doctor_report_from_summary(&summary, daemon_running, &self.data_dir, &self.tmp_dir)
    }
}

/// Converts a `SelfHealthSummary` (local or queried from `daemon.sock`) into `DoctorReport`.
#[must_use]
pub fn build_doctor_report_from_summary(
    summary: &SelfHealthSummary,
    daemon_running: bool,
    data_dir: &str,
    tmp_dir: &str,
) -> DoctorReport {
    let rss_ok = summary.rss_bytes < HEALTHY_RSS_BUDGET_BYTES;
    let fds_ok = summary.open_fds < MAX_OPEN_FDS_BUDGET;
    let tmp_ok = summary.tmp_files_created == 0;
    let slice_ok = summary.fabrico11y_slice_confined;
    let data_dir_ok = fabric_store::validate_non_tmp_path_on_disk(Path::new(data_dir)).is_ok();
    let tmp_dir_ok = fabric_store::validate_non_tmp_path_on_disk(Path::new(tmp_dir)).is_ok();

    let checks = vec![
        DoctorCheck {
            name: "rss_budget".into(),
            passed: rss_ok,
            actual: format!(
                "{} MiB ({} B)",
                summary.rss_bytes / (1024 * 1024),
                summary.rss_bytes
            ),
            threshold: format!("< {} MiB", HEALTHY_RSS_BUDGET_BYTES / (1024 * 1024)),
            detail: "resident set size of active telemetry process".into(),
        },
        DoctorCheck {
            name: "open_fds".into(),
            passed: fds_ok,
            actual: summary.open_fds.to_string(),
            threshold: format!("< {MAX_OPEN_FDS_BUDGET}"),
            detail: "live entries in /proc/<pid>/fd".into(),
        },
        DoctorCheck {
            name: "tmp_files_created".into(),
            passed: tmp_ok,
            actual: summary.tmp_files_created.to_string(),
            threshold: "== 0".into(),
            detail: "open descriptors targeting /tmp or /dev/shm".into(),
        },
        DoctorCheck {
            name: "fabrico11y_slice".into(),
            passed: slice_ok,
            actual: summary.cgroup_path.clone(),
            threshold: "contains fabrico11y.slice".into(),
            detail: "systemd cgroup v2 slice confinement".into(),
        },
        DoctorCheck {
            name: "storage_path_policy".into(),
            passed: data_dir_ok && tmp_dir_ok,
            actual: format!("data={data_dir}, tmp={tmp_dir}"),
            threshold: "non-tmpfs paths".into(),
            detail: "rejects /tmp and /dev/shm storage routing".into(),
        },
    ];

    let healthy = rss_ok
        && fds_ok
        && tmp_ok
        && slice_ok
        && data_dir_ok
        && tmp_dir_ok
        && !summary.self_shedding_active;

    DoctorReport {
        healthy,
        daemon_running,
        pid: summary.pid,
        rss_bytes: summary.rss_bytes,
        rss_budget_bytes: HEALTHY_RSS_BUDGET_BYTES,
        watchdog_shedding_threshold_bytes: WATCHDOG_SHEDDING_RSS_BYTES,
        self_shedding_active: summary.self_shedding_active,
        open_fds: summary.open_fds,
        max_open_fds_budget: MAX_OPEN_FDS_BUDGET,
        tmp_files_created: summary.tmp_files_created,
        cgroup_path: summary.cgroup_path.clone(),
        fabrico11y_slice_confined: summary.fabrico11y_slice_confined,
        data_dir: data_dir.to_owned(),
        tmp_dir: tmp_dir.to_owned(),
        dropped_events_total: summary.dropped_events_total,
        checks,
    }
}

#[must_use]
pub fn page_size_bytes() -> u64 {
    4096
}

fn read_self_rss_bytes() -> u64 {
    let Ok(statm) = fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    let resident_pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    resident_pages.saturating_mul(page_size_bytes())
}

fn is_readonly_directory_fd(fd_path: &Path) -> bool {
    if !fs::metadata(fd_path).is_ok_and(|m| m.is_dir()) {
        return false;
    }
    let Some(name) = fd_path.file_name() else {
        return false;
    };
    let info_path = Path::new("/proc/self/fdinfo").join(name);
    let Ok(info) = fs::read_to_string(info_path) else {
        return false;
    };
    for line in info.lines() {
        if let Some(raw_flags) = line.strip_prefix("flags:") {
            return u64::from_str_radix(raw_flags.trim(), 8).is_ok_and(|flags| (flags & 0o3) == 0);
        }
    }
    false
}

fn inspect_self_fds() -> (u32, u32) {
    let Ok(entries) = fs::read_dir("/proc/self/fd") else {
        return (0, 0);
    };
    let mut count = 0u32;
    let mut tmp_count = 0u32;
    for entry in entries.flatten() {
        count = count.saturating_add(1);
        let fd_path = entry.path();
        if let Ok(target) = fs::read_link(&fd_path) {
            let s = target.to_string_lossy();
            if (s.starts_with("/tmp/")
                || s == "/tmp"
                || s.starts_with("/dev/shm/")
                || s == "/dev/shm")
                && !is_readonly_directory_fd(&fd_path)
            {
                tmp_count = tmp_count.saturating_add(1);
            }
        }
    }
    (count, tmp_count)
}

fn read_self_cgroup_path() -> String {
    let Ok(raw) = fs::read_to_string("/proc/self/cgroup") else {
        return "unknown".into();
    };
    raw.lines()
        .next()
        .and_then(|line| line.splitn(3, ':').nth(2))
        .unwrap_or("unknown")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readonly_tmp_dir_scan_does_not_flag_tmp_files_created() {
        let _dir = fs::read_dir("/tmp").ok();
        assert_eq!(inspect_self_fds().1, 0);
    }
}
