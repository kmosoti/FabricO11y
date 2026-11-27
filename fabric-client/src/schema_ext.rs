//! Extended `fabric.cli.v1` schema types (GPU, causal lineage), serde helpers,
//! and formatting utilities.

use crate::schema::{FabricCustody, FabricMemory};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// Deserializes an optional JSON string (`null` or missing) as an empty string
/// so live `fabric --json` nullable string fields never fail deserialization.
pub(crate) fn deserialize_string_or_null<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

/// Optional DRM/sysfs GPU telemetry entry from `HostSnapshot`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricGpu {
    pub card: String,
    pub vendor: String,
    pub name: Option<String>,
    pub vram_used_bytes: Option<u64>,
    pub vram_total_bytes: Option<u64>,
    pub busy_pct: Option<f64>,
    pub temperature_c: Option<f64>,
}

/// Causal lineage subgraph returned by `fabric lineage --json`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricLineage {
    pub target: String,
    pub matched_roots: Vec<String>,
    pub direction: String,
    pub max_depth: usize,
    pub nodes: Vec<FabricLineageNode>,
    pub edges: Vec<FabricLineageEdge>,
}

/// Vertex inside a [`FabricLineage`] subgraph.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricLineageNode {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub metadata: BTreeMap<String, serde_json::Value>,
}

/// Directed causal edge inside a [`FabricLineage`] subgraph.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricLineageEdge {
    pub src: String,
    pub dst: String,
    pub kind: String,
    pub weight: f64,
    pub note: String,
}

/// Formats byte counts compactly using IEC binary units (`GiB`, `MiB`, `KiB`, `B`).
#[must_use]
pub fn format_bytes_iec(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const KIB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.1} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.0} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.0} KiB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

/// Computes used RAM percentage in `[0.0, 100.0]`.
#[must_use]
pub fn mem_used_pct(mem: &FabricMemory) -> f64 {
    if mem.mem_total_bytes == 0 {
        return 0.0;
    }
    let used = mem.mem_total_bytes.saturating_sub(mem.mem_available_bytes);
    (used as f64 / mem.mem_total_bytes as f64) * 100.0
}

/// Returns unprojected record lag between Spindle highest record ID and canonical receiver.
#[must_use]
pub fn custody_lag(custody: &FabricCustody) -> u64 {
    custody
        .highest_record_id
        .saturating_sub(custody.canonical_through)
}
