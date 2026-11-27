//! Strongly typed `fabric.cli.v1` JSON schemas for `FabricO11y`.

use crate::schema_ext::deserialize_string_or_null;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use crate::schema_ext::{
    FabricGpu, FabricLineage, FabricLineageEdge, FabricLineageNode, custody_lag, format_bytes_iec,
    mem_used_pct,
};

/// Expected schema version emitted by `fabric ... --json`.
pub const EXPECTED_SCHEMA_VERSION: &str = "fabric.cli.v1";

/// Parses and validates a `fabric.cli.v1` JSON envelope.
pub fn parse_envelope<T: DeserializeOwned>(bytes: &[u8]) -> Result<FabricEnvelope<T>, String> {
    let env: FabricEnvelope<T> =
        serde_json::from_slice(bytes).map_err(|e| format!("json parse: {e}"))?;
    if env.schema_version != EXPECTED_SCHEMA_VERSION {
        return Err(format!(
            "unsupported schema_version {:?} (expected {:?})",
            env.schema_version, EXPECTED_SCHEMA_VERSION
        ));
    }
    Ok(env)
}

/// Versioned outer wrapper emitted by every `fabric ... --json` subcommand.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct FabricEnvelope<T> {
    pub schema_version: String,
    #[serde(default)]
    pub generated_at_unix_ms: u64,
    #[serde(default)]
    pub command: String,
    pub data: T,
}

/// Host, slice, agent, and self-health telemetry snapshot from `fabric snapshot --json`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricSnapshot {
    pub timestamp_unix_ms: u64,
    pub hostname: String,
    pub psi: FabricPsi,
    pub memory: FabricMemory,
    pub mounts: Vec<FabricMount>,
    pub slices: Vec<FabricSlice>,
    pub agents: Vec<FabricAgent>,
    pub gpus: Vec<FabricGpu>,
    pub self_health: FabricSelfHealth,
}

/// Aggregated Linux `/proc/pressure/{cpu,memory,io}` metrics.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricPsi {
    pub cpu_some: PsiStat,
    pub cpu_full: PsiStat,
    pub memory_some: PsiStat,
    pub memory_full: PsiStat,
    pub io_some: PsiStat,
    pub io_full: PsiStat,
}

/// Kernel Pressure Stall Information sample for one resource window.
#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct PsiStat {
    pub avg10: f64,
    pub avg60: f64,
    pub avg300: f64,
    pub total_usec: u64,
}

/// Host RAM, swap, and optional zram summary.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricMemory {
    pub mem_total_bytes: u64,
    pub mem_available_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub zram: Option<FabricZram>,
}

/// `/sys/block/zram0/mm_stat` telemetry.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricZram {
    pub device: String,
    pub orig_data_size_bytes: u64,
    pub compr_data_size_bytes: u64,
    pub mem_used_total_bytes: u64,
    pub mem_limit_bytes: u64,
    pub compression_ratio: f64,
    pub same_pages: u64,
    pub pages_compacted: u64,
}

/// Mounted filesystem utilization entry.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricMount {
    pub mount_point: String,
    pub fs_type: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,
    pub used_pct: f64,
}

/// Top-level systemd slice resource summary.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricSlice {
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

/// Autonomous agent workload summary.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricAgent {
    pub session_id: String,
    pub agent_name: String,
    pub slice: String,
    #[serde(default, deserialize_with = "deserialize_string_or_null")]
    pub unit: String,
    pub pids: Vec<u64>,
    pub rss_bytes: u64,
    pub write_bytes: u64,
    pub touched_paths: Vec<String>,
}

/// Self-preservation health summary embedded in [`FabricSnapshot`].
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricSelfHealth {
    pub pid: u64,
    pub rss_bytes: u64,
    pub open_fds: u64,
    pub tmp_files_created: u64,
    pub cgroup_path: String,
    pub fabrico11y_slice_confined: bool,
    pub dropped_events_total: u64,
    pub self_shedding_active: bool,
}

/// Spindle and canonical receiver custody summary from `fabric custody --json`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricCustody {
    #[serde(default, deserialize_with = "deserialize_string_or_null")]
    pub spindle_socket: String,
    pub spindle_online: bool,
    pub spindle_origin: Option<String>,
    pub spindle_error: Option<String>,
    pub records: u64,
    pub highest_record_id: u64,
    pub canonical_through: u64,
    pub reclaimed_through: u64,
    pub forwarding_error: Option<String>,
    pub server_endpoint: Option<String>,
    pub server_online: bool,
    pub server_authority: Option<String>,
    pub server_error: Option<String>,
    pub server_frontiers: BTreeMap<String, u64>,
    pub collector_spindle_watermark_id: Option<u64>,
    pub catalog_error: Option<String>,
}

/// Ranked resource pressure blame response from `fabric blame <resource> --json`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricBlame {
    pub resource: String,
    pub total_observed: u64,
    pub unit_label: String,
    pub pressure_summary: String,
    pub entries: Vec<FabricBlameEntry>,
}

/// Single ranked resource contributor inside [`FabricBlame`].
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricBlameEntry {
    pub rank: usize,
    pub node_id: String,
    pub label: String,
    pub contribution_value: u64,
    pub unit_label: String,
    pub share_pct: f64,
    pub pid: Option<u64>,
    pub comm: Option<String>,
    pub unit: Option<String>,
    pub slice: Option<String>,
    pub agent: Option<String>,
    pub target_path: Option<String>,
    pub lineage_chain: Vec<String>,
}

/// Five Data Observability Pillars drift summary from `fabric drift --json`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricDrift {
    pub total_findings: usize,
    pub critical_count: usize,
    pub warn_count: usize,
    pub info_count: usize,
    pub findings: Vec<FabricDriftFinding>,
}

/// Individual pillar finding inside [`FabricDrift`].
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricDriftFinding {
    pub pillar: String,
    pub severity: String,
    pub target: String,
    pub summary: String,
    pub metric_value: f64,
    pub threshold_value: f64,
}

/// Self-preservation verification report from `fabric doctor --json`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricDoctor {
    pub healthy: bool,
    pub daemon_running: bool,
    pub pid: Option<u64>,
    pub rss_bytes: u64,
    pub rss_budget_bytes: u64,
    pub open_fds: u64,
    pub max_open_fds_budget: u64,
    pub tmp_files_created: u64,
    pub fabrico11y_slice_confined: bool,
    pub data_dir: String,
    pub tmp_dir: String,
    pub cgroup_path: String,
    pub dropped_events_total: u64,
    pub watchdog_shedding_threshold_bytes: u64,
    pub self_shedding_active: bool,
    pub checks: Vec<FabricDoctorCheck>,
}

/// Individual check row inside [`FabricDoctor`].
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricDoctorCheck {
    pub name: String,
    pub passed: bool,
    pub actual: String,
    pub threshold: String,
    pub detail: String,
}

/// Query output payload from `fabric query metrics --json`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricMetricQuery {
    pub domain: String,
    pub row_count: usize,
    pub rows: Vec<FabricMetricRow>,
}

/// Single metric row inside [`FabricMetricQuery`].
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricMetricRow {
    pub host: String,
    pub name: String,
    pub slice: Option<String>,
    pub unit: Option<String>,
    pub timestamp_ms: u64,
    pub value: f64,
}

/// Complete telemetry bundle collected concurrently from `FabricO11y`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FabricBundle {
    pub online: bool,
    pub source_desc: String,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub snapshot: Option<FabricSnapshot>,
    pub custody: Option<FabricCustody>,
    pub blame_memory: Option<FabricBlame>,
    pub drift: Option<FabricDrift>,
    pub doctor: Option<FabricDoctor>,
    pub metric_series: Vec<(u64, f64)>,
}
