//! Versioned domain contracts for FabricO11y telemetry, blame, and doctor reports.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Versioned CLI JSON envelope schema identifier.
pub const SCHEMA_VERSION: &str = "fabric.cli.v1";

/// RSS budget enforced as healthy threshold in `fabric doctor` (`100 MiB`).
pub const HEALTHY_RSS_BUDGET_BYTES: u64 = 100 * 1024 * 1024;

/// Watchdog self-shedding threshold in `fabric daemon` (`200 MiB`).
pub const WATCHDOG_SHEDDING_RSS_BYTES: u64 = 200 * 1024 * 1024;

/// Maximum open file descriptor budget reported by `fabric doctor`.
pub const MAX_OPEN_FDS_BUDGET: u32 = 64;

/// Versioned outer wrapper emitted by every `fabric ... --json` command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CliEnvelope<T> {
    pub schema_version: String,
    pub generated_at_unix_ms: i64,
    pub command: String,
    pub data: T,
}

impl<T> CliEnvelope<T> {
    /// Wraps payload data with the current schema version and command tag.
    #[must_use]
    pub fn new(generated_at_unix_ms: i64, command: impl Into<String>, data: T) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_owned(),
            generated_at_unix_ms,
            command: command.into(),
            data,
        }
    }
}

/// Kernel Pressure Stall Information sample for one resource (`some` or `full`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PsiResource {
    pub avg10: f64,
    pub avg60: f64,
    pub avg300: f64,
    pub total_usec: u64,
}

/// Aggregated Linux `/proc/pressure/{cpu,memory,io}` state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PsiMetrics {
    pub cpu_some: PsiResource,
    pub cpu_full: PsiResource,
    pub memory_some: PsiResource,
    pub memory_full: PsiResource,
    pub io_some: PsiResource,
    pub io_full: PsiResource,
}

/// `/sys/block/zram0/mm_stat` telemetry.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ZramStats {
    pub device: String,
    pub orig_data_size_bytes: u64,
    pub compr_data_size_bytes: u64,
    pub mem_used_total_bytes: u64,
    pub mem_limit_bytes: u64,
    pub compression_ratio: f64,
    pub same_pages: u64,
    pub pages_compacted: u64,
}

/// Host memory, swap, and zram summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MemorySnapshot {
    pub mem_total_bytes: u64,
    pub mem_available_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub zram: Option<ZramStats>,
}

/// Mounted filesystem utilization via `statvfs`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MountUsage {
    pub mount_point: String,
    pub fs_type: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,
    pub used_pct: f64,
}

/// Top-level systemd slice resource summary (`app.slice`, `agent.slice`, `fabrico11y.slice`, etc.).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SliceSummary {
    pub slice_name: String,
    pub cgroup_path: String,
    pub memory_current_bytes: u64,
    pub memory_high_bytes: Option<u64>,
    pub memory_max_bytes: Option<u64>,
    pub memory_peak_bytes: Option<u64>,
    pub cpu_usage_usec: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub pids_current: u64,
    pub memory_high_events: u64,
    pub oom_events: u64,
}

/// Individual cgroup leaf/unit sample.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CgroupSample {
    pub path: String,
    pub slice: String,
    pub unit: Option<String>,
    pub memory_current_bytes: u64,
    pub memory_peak_bytes: Option<u64>,
    pub memory_swap_current_bytes: u64,
    pub cpu_usage_usec: u64,
    pub io_read_bytes: u64,
    pub io_write_bytes: u64,
    pub pids_current: u64,
    pub populated: bool,
}

/// Process observation enriched with cgroup/systemd/agent attribution and open paths.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProcessSample {
    pub pid: u32,
    pub ppid: u32,
    pub comm: String,
    pub cmdline: String,
    pub unit: Option<String>,
    pub slice: String,
    pub cgroup_path: String,
    pub rss_bytes: u64,
    pub swap_bytes: u64,
    pub cpu_time_ticks: u64,
    pub open_fds: u32,
    pub unlinked_fds: u32,
    pub cwd: Option<String>,
    pub open_paths: Vec<String>,
    pub agent_hint: Option<String>,
}

/// Systemd unit and transient scope hygiene state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UnitRecord {
    pub name: String,
    pub manager: String,
    pub slice: String,
    pub active_state: String,
    pub sub_state: String,
    pub transient: bool,
    pub collect_mode_enabled: bool,
    pub orphaned_reason: Option<String>,
    pub main_pid: Option<u32>,
    pub memory_current_bytes: u64,
    pub cpu_usage_usec: u64,
    pub populated: bool,
}

/// High-churn path inventory entry (`/tmp`, `/data/tmp`, `~/.cache`, `target/`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FsChurnEntry {
    pub path: String,
    pub mount_point: String,
    pub size_bytes: u64,
    pub recent_delta_bytes: i64,
    pub file_count: u64,
    pub unlinked_open_fds: u64,
    pub mtime_unix_ms: i64,
    pub owner_pid: Option<u32>,
    pub owner_comm: Option<String>,
    pub owner_unit: Option<String>,
    pub owner_slice: Option<String>,
    pub owner_agent: Option<String>,
}

/// Optional GPU device stats read from DRM / NVIDIA sysfs nodes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GpuDeviceStats {
    pub card: String,
    pub vendor: String,
    pub vram_used_bytes: Option<u64>,
    pub vram_total_bytes: Option<u64>,
    pub busy_pct: Option<f64>,
    pub temperature_c: Option<f64>,
}

/// Aggregated autonomous agent workload summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentSessionRecord {
    pub session_id: String,
    pub agent_name: String,
    pub slice: String,
    pub unit: Option<String>,
    pub pids: Vec<u32>,
    pub rss_bytes: u64,
    pub write_bytes: u64,
    pub touched_paths: Vec<String>,
}

/// Compact self-health summary embedded inside `HostSnapshot`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SelfHealthSummary {
    pub pid: u32,
    pub rss_bytes: u64,
    pub open_fds: u32,
    pub tmp_files_created: u32,
    pub cgroup_path: String,
    pub fabrico11y_slice_confined: bool,
    pub dropped_events_total: u64,
    pub self_shedding_active: bool,
}

/// Full point-in-time host, agent, and self-preservation telemetry snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HostSnapshot {
    pub timestamp_unix_ms: i64,
    pub hostname: String,
    pub psi: PsiMetrics,
    pub memory: MemorySnapshot,
    pub mounts: Vec<MountUsage>,
    pub slices: Vec<SliceSummary>,
    pub cgroups: Vec<CgroupSample>,
    pub processes: Vec<ProcessSample>,
    pub units: Vec<UnitRecord>,
    pub fs_churn: Vec<FsChurnEntry>,
    pub gpus: Vec<GpuDeviceStats>,
    pub agents: Vec<AgentSessionRecord>,
    pub self_health: SelfHealthSummary,
}

/// Internal Parquet field reserved for provenance supplied by the Spindle collector.
pub const SPINDLE_PROVENANCE_COLUMN: &str = "_spindle_provenance";

/// Source identity attached to an individual raw Spindle projection row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SpindleProvenance {
    pub origin: String,
    pub record_id: u64,
    pub producer: String,
    pub generation: String,
    pub sequence: u64,
}

/// OLAP time-series metric sample.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MetricPoint {
    pub timestamp_ms: i64,
    pub name: String,
    pub value: f64,
    pub host: String,
    pub slice: String,
    pub unit: String,
    pub pid: Option<u32>,
    pub labels_json: String,
}

/// OLAP structured log record.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LogRecord {
    pub timestamp_ms: i64,
    pub level: String,
    pub source: String,
    pub unit: String,
    pub message: String,
    pub attributes_json: String,
}

/// OLAP trace span record.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TraceSpanRecord {
    pub timestamp_ms: i64,
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub name: String,
    pub service: String,
    pub duration_ms: f64,
    pub status: String,
    pub attributes_json: String,
}

/// OLAP filesystem churn event record.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FsEventRecord {
    pub timestamp_ms: i64,
    pub path: String,
    pub mount_point: String,
    pub event_kind: String,
    pub size_bytes: u64,
    pub delta_bytes: i64,
    pub owner_pid: Option<u32>,
    pub owner_unit: String,
    pub owner_slice: String,
}

/// OLAP causal graph edge record.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GraphEdgeRecord {
    pub timestamp_ms: i64,
    pub src_id: String,
    pub src_kind: String,
    pub edge_kind: String,
    pub dst_id: String,
    pub dst_kind: String,
    pub weight: f64,
    pub note: String,
}

/// Contended resource targets supported by `fabric blame <resource>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlameResource {
    Swap,
    Memory,
    Tmpfs,
    Disk(String),
    Cpu,
}

impl fmt::Display for BlameResource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Swap => f.write_str("swap"),
            Self::Memory => f.write_str("memory"),
            Self::Tmpfs => f.write_str("tmpfs"),
            Self::Disk(mount) => write!(f, "disk:{mount}"),
            Self::Cpu => f.write_str("cpu"),
        }
    }
}

impl FromStr for BlameResource {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let trimmed = raw.trim();
        match trimmed {
            "swap" => Ok(Self::Swap),
            "memory" | "mem" | "ram" => Ok(Self::Memory),
            "tmpfs" | "/tmp" => Ok(Self::Tmpfs),
            "cpu" => Ok(Self::Cpu),
            other => {
                if let Some(mount) = other.strip_prefix("disk:") {
                    if mount.is_empty() {
                        Err("disk blame target requires a mount path such as disk:/data".into())
                    } else {
                        Ok(Self::Disk(mount.to_owned()))
                    }
                } else {
                    Err(format!(
                        "unsupported blame resource '{other}' (expected swap, memory, tmpfs, cpu, or disk:<mount>)"
                    ))
                }
            }
        }
    }
}

/// Ranked resource consumer emitted by `fabric blame`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BlameEntry {
    pub rank: u32,
    pub node_id: String,
    pub label: String,
    pub contribution_value: u64,
    pub unit_label: String,
    pub share_pct: f64,
    pub pid: Option<u32>,
    pub comm: Option<String>,
    pub unit: Option<String>,
    pub slice: String,
    pub agent: Option<String>,
    pub target_path: Option<String>,
    pub lineage_chain: Vec<String>,
}

/// Complete ranked blame response emitted by `fabric blame <resource>`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BlameReport {
    pub resource: String,
    pub total_observed: u64,
    pub unit_label: String,
    pub pressure_summary: String,
    pub entries: Vec<BlameEntry>,
}

/// Individual check row inside `DoctorReport`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DoctorCheck {
    pub name: String,
    pub passed: bool,
    pub actual: String,
    pub threshold: String,
    pub detail: String,
}

/// Self-preservation health verification report returned by `fabric doctor`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DoctorReport {
    pub healthy: bool,
    pub daemon_running: bool,
    pub pid: u32,
    pub rss_bytes: u64,
    pub rss_budget_bytes: u64,
    pub watchdog_shedding_threshold_bytes: u64,
    pub self_shedding_active: bool,
    pub open_fds: u32,
    pub max_open_fds_budget: u32,
    pub tmp_files_created: u32,
    pub cgroup_path: String,
    pub fabrico11y_slice_confined: bool,
    pub data_dir: String,
    pub tmp_dir: String,
    pub dropped_events_total: u64,
    pub checks: Vec<DoctorCheck>,
}
