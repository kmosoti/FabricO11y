//! Arrow schemas, RecordBatch builders, and IPC stream serialization.

use crate::anomaly::{DriftReport, PillarFinding};
use crate::model::{
    AgentSessionRecord, BlameReport, DoctorReport, FsChurnEntry, FsEventRecord, GraphEdgeRecord,
    HostSnapshot, LogRecord, MetricPoint, TraceSpanRecord, UnitRecord,
};
use arrow::array::{BooleanArray, Float64Array, Int64Array, StringArray, UInt32Array, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use std::sync::{Arc, LazyLock};

/// Canonical OLAP Arrow schema for host and OTLP metric points.
pub static METRICS_SCHEMA: LazyLock<SchemaRef> = LazyLock::new(|| {
    Arc::new(Schema::new(vec![
        Field::new("timestamp_ms", DataType::Int64, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("value", DataType::Float64, false),
        Field::new("host", DataType::Utf8, false),
        Field::new("slice", DataType::Utf8, false),
        Field::new("unit", DataType::Utf8, false),
        Field::new("pid", DataType::UInt32, true),
        Field::new("labels_json", DataType::Utf8, false),
    ]))
});

/// Canonical OLAP Arrow schema for structured logs.
pub static LOGS_SCHEMA: LazyLock<SchemaRef> = LazyLock::new(|| {
    Arc::new(Schema::new(vec![
        Field::new("timestamp_ms", DataType::Int64, false),
        Field::new("level", DataType::Utf8, false),
        Field::new("source", DataType::Utf8, false),
        Field::new("unit", DataType::Utf8, false),
        Field::new("message", DataType::Utf8, false),
        Field::new("attributes_json", DataType::Utf8, false),
    ]))
});

/// Canonical OLAP Arrow schema for distributed/agent trace spans.
pub static TRACES_SCHEMA: LazyLock<SchemaRef> = LazyLock::new(|| {
    Arc::new(Schema::new(vec![
        Field::new("timestamp_ms", DataType::Int64, false),
        Field::new("trace_id", DataType::Utf8, false),
        Field::new("span_id", DataType::Utf8, false),
        Field::new("parent_span_id", DataType::Utf8, true),
        Field::new("name", DataType::Utf8, false),
        Field::new("service", DataType::Utf8, false),
        Field::new("duration_ms", DataType::Float64, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("attributes_json", DataType::Utf8, false),
    ]))
});

/// Canonical OLAP Arrow schema for high-churn filesystem events.
pub static FS_EVENTS_SCHEMA: LazyLock<SchemaRef> = LazyLock::new(|| {
    Arc::new(Schema::new(vec![
        Field::new("timestamp_ms", DataType::Int64, false),
        Field::new("path", DataType::Utf8, false),
        Field::new("mount_point", DataType::Utf8, false),
        Field::new("event_kind", DataType::Utf8, false),
        Field::new("size_bytes", DataType::UInt64, false),
        Field::new("delta_bytes", DataType::Int64, false),
        Field::new("owner_pid", DataType::UInt32, true),
        Field::new("owner_unit", DataType::Utf8, false),
        Field::new("owner_slice", DataType::Utf8, false),
    ]))
});

/// Canonical OLAP Arrow schema for persisted causal graph edges.
pub static GRAPH_EDGES_SCHEMA: LazyLock<SchemaRef> = LazyLock::new(|| {
    Arc::new(Schema::new(vec![
        Field::new("timestamp_ms", DataType::Int64, false),
        Field::new("src_id", DataType::Utf8, false),
        Field::new("src_kind", DataType::Utf8, false),
        Field::new("edge_kind", DataType::Utf8, false),
        Field::new("dst_id", DataType::Utf8, false),
        Field::new("dst_kind", DataType::Utf8, false),
        Field::new("weight", DataType::Float64, false),
        Field::new("note", DataType::Utf8, false),
    ]))
});

/// Arrow schema for systemd unit and transient scope state.
pub static UNITS_SCHEMA: LazyLock<SchemaRef> = LazyLock::new(|| {
    Arc::new(Schema::new(vec![
        Field::new("name", DataType::Utf8, false),
        Field::new("manager", DataType::Utf8, false),
        Field::new("slice", DataType::Utf8, false),
        Field::new("active_state", DataType::Utf8, false),
        Field::new("sub_state", DataType::Utf8, false),
        Field::new("transient", DataType::Boolean, false),
        Field::new("collect_mode_enabled", DataType::Boolean, false),
        Field::new("orphaned_reason", DataType::Utf8, true),
        Field::new("main_pid", DataType::UInt32, true),
        Field::new("memory_current_bytes", DataType::UInt64, false),
        Field::new("cpu_usage_usec", DataType::UInt64, false),
        Field::new("populated", DataType::Boolean, false),
    ]))
});

/// Arrow schema for agent session workload summary.
pub static AGENTS_SCHEMA: LazyLock<SchemaRef> = LazyLock::new(|| {
    Arc::new(Schema::new(vec![
        Field::new("session_id", DataType::Utf8, false),
        Field::new("agent_name", DataType::Utf8, false),
        Field::new("slice", DataType::Utf8, false),
        Field::new("unit", DataType::Utf8, true),
        Field::new("pid_count", DataType::UInt64, false),
        Field::new("rss_bytes", DataType::UInt64, false),
        Field::new("write_bytes", DataType::UInt64, false),
        Field::new("touched_paths", DataType::Utf8, false),
    ]))
});

/// Arrow schema for `fabric snapshot --arrow`.
pub static SNAPSHOT_SLICES_SCHEMA: LazyLock<SchemaRef> = LazyLock::new(|| {
    Arc::new(Schema::new(vec![
        Field::new("timestamp_unix_ms", DataType::Int64, false),
        Field::new("hostname", DataType::Utf8, false),
        Field::new("slice_name", DataType::Utf8, false),
        Field::new("cgroup_path", DataType::Utf8, false),
        Field::new("memory_current_bytes", DataType::UInt64, false),
        Field::new("memory_high_bytes", DataType::UInt64, true),
        Field::new("memory_max_bytes", DataType::UInt64, true),
        Field::new("cpu_usage_usec", DataType::UInt64, false),
        Field::new("io_write_bytes", DataType::UInt64, false),
        Field::new("pids_current", DataType::UInt64, false),
        Field::new("swap_used_bytes", DataType::UInt64, false),
        Field::new("mem_available_bytes", DataType::UInt64, false),
    ]))
});

/// Arrow schema for `fabric blame --arrow`.
pub static BLAME_SCHEMA: LazyLock<SchemaRef> = LazyLock::new(|| {
    Arc::new(Schema::new(vec![
        Field::new("rank", DataType::UInt32, false),
        Field::new("resource", DataType::Utf8, false),
        Field::new("node_id", DataType::Utf8, false),
        Field::new("label", DataType::Utf8, false),
        Field::new("contribution_value", DataType::UInt64, false),
        Field::new("unit_label", DataType::Utf8, false),
        Field::new("share_pct", DataType::Float64, false),
        Field::new("pid", DataType::UInt32, true),
        Field::new("comm", DataType::Utf8, true),
        Field::new("unit", DataType::Utf8, true),
        Field::new("slice", DataType::Utf8, false),
        Field::new("agent", DataType::Utf8, true),
        Field::new("target_path", DataType::Utf8, true),
    ]))
});

/// Arrow schema for `fabric drift --arrow`.
pub static PILLAR_FINDINGS_SCHEMA: LazyLock<SchemaRef> = LazyLock::new(|| {
    Arc::new(Schema::new(vec![
        Field::new("pillar", DataType::Utf8, false),
        Field::new("severity", DataType::Utf8, false),
        Field::new("target", DataType::Utf8, false),
        Field::new("summary", DataType::Utf8, false),
        Field::new("metric_value", DataType::Float64, false),
        Field::new("threshold_value", DataType::Float64, false),
    ]))
});

/// Arrow schema for `fabric lineage --arrow` node listings.
pub static LINEAGE_NODES_SCHEMA: LazyLock<SchemaRef> = LazyLock::new(|| {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("label", DataType::Utf8, false),
        Field::new("metadata_json", DataType::Utf8, false),
    ]))
});

/// Arrow schema for `fabric doctor --arrow`.
pub static DOCTOR_SCHEMA: LazyLock<SchemaRef> = LazyLock::new(|| {
    Arc::new(Schema::new(vec![
        Field::new("check_name", DataType::Utf8, false),
        Field::new("passed", DataType::Boolean, false),
        Field::new("actual", DataType::Utf8, false),
        Field::new("threshold", DataType::Utf8, false),
        Field::new("detail", DataType::Utf8, false),
        Field::new("healthy", DataType::Boolean, false),
        Field::new("rss_bytes", DataType::UInt64, false),
        Field::new("open_fds", DataType::UInt32, false),
        Field::new("tmp_files_created", DataType::UInt32, false),
        Field::new("fabrico11y_slice_confined", DataType::Boolean, false),
    ]))
});

/// Encodes Arrow record batches into a single Arrow IPC stream buffer.
pub fn encode_batches_to_ipc_stream(
    schema: &SchemaRef,
    batches: &[RecordBatch],
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    {
        let mut writer =
            StreamWriter::try_new(&mut bytes, schema.as_ref()).map_err(|e| e.to_string())?;
        for batch in batches {
            writer.write(batch).map_err(|e| e.to_string())?;
        }
        writer.finish().map_err(|e| e.to_string())?;
    }
    Ok(bytes)
}

/// Converts metric points into an Arrow `RecordBatch`.
pub fn metrics_to_record_batch(rows: &[MetricPoint]) -> Result<RecordBatch, String> {
    let schema = METRICS_SCHEMA.clone();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|r| r.timestamp_ms),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.name.as_str()),
            )),
            Arc::new(Float64Array::from_iter_values(rows.iter().map(|r| r.value))),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.host.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.slice.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.unit.as_str()),
            )),
            Arc::new(UInt32Array::from(
                rows.iter().map(|r| r.pid).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.labels_json.as_str()),
            )),
        ],
    )
    .map_err(|e| e.to_string())
}

/// Converts log records into an Arrow `RecordBatch`.
pub fn logs_to_record_batch(rows: &[LogRecord]) -> Result<RecordBatch, String> {
    let schema = LOGS_SCHEMA.clone();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|r| r.timestamp_ms),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.level.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.source.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.unit.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.message.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.attributes_json.as_str()),
            )),
        ],
    )
    .map_err(|e| e.to_string())
}

/// Converts trace spans into an Arrow `RecordBatch`.
pub fn traces_to_record_batch(rows: &[TraceSpanRecord]) -> Result<RecordBatch, String> {
    let schema = TRACES_SCHEMA.clone();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|r| r.timestamp_ms),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.trace_id.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.span_id.as_str()),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|r| r.parent_span_id.as_deref())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.name.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.service.as_str()),
            )),
            Arc::new(Float64Array::from_iter_values(
                rows.iter().map(|r| r.duration_ms),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.status.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.attributes_json.as_str()),
            )),
        ],
    )
    .map_err(|e| e.to_string())
}

/// Converts filesystem churn events into an Arrow `RecordBatch`.
pub fn fs_events_to_record_batch(rows: &[FsEventRecord]) -> Result<RecordBatch, String> {
    let schema = FS_EVENTS_SCHEMA.clone();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|r| r.timestamp_ms),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.path.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.mount_point.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.event_kind.as_str()),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|r| r.size_bytes),
            )),
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|r| r.delta_bytes),
            )),
            Arc::new(UInt32Array::from(
                rows.iter().map(|r| r.owner_pid).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.owner_unit.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.owner_slice.as_str()),
            )),
        ],
    )
    .map_err(|e| e.to_string())
}

/// Converts `FsChurnEntry` records into an `FS_EVENTS_SCHEMA` Arrow `RecordBatch`.
pub fn fs_churn_to_record_batch(
    timestamp_ms: i64,
    rows: &[FsChurnEntry],
) -> Result<RecordBatch, String> {
    let events: Vec<FsEventRecord> = rows
        .iter()
        .map(|entry| FsEventRecord {
            timestamp_ms: if entry.mtime_unix_ms > 0 {
                entry.mtime_unix_ms
            } else {
                timestamp_ms
            },
            path: entry.path.clone(),
            mount_point: entry.mount_point.clone(),
            event_kind: if entry.unlinked_open_fds > 0 {
                "unlinked_open_fd".into()
            } else {
                "churn".into()
            },
            size_bytes: entry.size_bytes,
            delta_bytes: entry.recent_delta_bytes,
            owner_pid: entry.owner_pid,
            owner_unit: entry.owner_unit.clone().unwrap_or_default(),
            owner_slice: entry.owner_slice.clone().unwrap_or_default(),
        })
        .collect();
    fs_events_to_record_batch(&events)
}

/// Converts causal graph edges into an Arrow `RecordBatch`.
pub fn graph_edges_to_record_batch(rows: &[GraphEdgeRecord]) -> Result<RecordBatch, String> {
    let schema = GRAPH_EDGES_SCHEMA.clone();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|r| r.timestamp_ms),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.src_id.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.src_kind.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.edge_kind.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.dst_id.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.dst_kind.as_str()),
            )),
            Arc::new(Float64Array::from_iter_values(
                rows.iter().map(|r| r.weight),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.note.as_str()),
            )),
        ],
    )
    .map_err(|e| e.to_string())
}

/// Converts unit hygiene records into an Arrow `RecordBatch`.
pub fn units_to_record_batch(rows: &[UnitRecord]) -> Result<RecordBatch, String> {
    let schema = UNITS_SCHEMA.clone();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.name.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.manager.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.slice.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.active_state.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.sub_state.as_str()),
            )),
            Arc::new(BooleanArray::from_iter(
                rows.iter().map(|r| Some(r.transient)),
            )),
            Arc::new(BooleanArray::from_iter(
                rows.iter().map(|r| Some(r.collect_mode_enabled)),
            )),
            Arc::new(StringArray::from(
                rows.iter()
                    .map(|r| r.orphaned_reason.as_deref())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(UInt32Array::from(
                rows.iter().map(|r| r.main_pid).collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|r| r.memory_current_bytes),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|r| r.cpu_usage_usec),
            )),
            Arc::new(BooleanArray::from_iter(
                rows.iter().map(|r| Some(r.populated)),
            )),
        ],
    )
    .map_err(|e| e.to_string())
}

/// Converts agent session summaries into an Arrow `RecordBatch`.
pub fn agents_to_record_batch(rows: &[AgentSessionRecord]) -> Result<RecordBatch, String> {
    let schema = AGENTS_SCHEMA.clone();
    let paths_joined: Vec<String> = rows.iter().map(|r| r.touched_paths.join(";")).collect();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.session_id.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.agent_name.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.slice.as_str()),
            )),
            Arc::new(StringArray::from(
                rows.iter().map(|r| r.unit.as_deref()).collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|r| r.pids.len() as u64),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|r| r.rss_bytes),
            )),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|r| r.write_bytes),
            )),
            Arc::new(StringArray::from_iter_values(
                paths_joined.iter().map(String::as_str),
            )),
        ],
    )
    .map_err(|e| e.to_string())
}

/// Converts a `HostSnapshot` into a slice-level Arrow `RecordBatch`.
pub fn snapshot_to_record_batch(snapshot: &HostSnapshot) -> Result<RecordBatch, String> {
    let schema = SNAPSHOT_SLICES_SCHEMA.clone();
    let len = snapshot.slices.len();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(vec![snapshot.timestamp_unix_ms; len])),
            Arc::new(StringArray::from(vec![snapshot.hostname.as_str(); len])),
            Arc::new(StringArray::from_iter_values(
                snapshot.slices.iter().map(|s| s.slice_name.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                snapshot.slices.iter().map(|s| s.cgroup_path.as_str()),
            )),
            Arc::new(UInt64Array::from_iter_values(
                snapshot.slices.iter().map(|s| s.memory_current_bytes),
            )),
            Arc::new(UInt64Array::from(
                snapshot
                    .slices
                    .iter()
                    .map(|s| s.memory_high_bytes)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from(
                snapshot
                    .slices
                    .iter()
                    .map(|s| s.memory_max_bytes)
                    .collect::<Vec<_>>(),
            )),
            Arc::new(UInt64Array::from_iter_values(
                snapshot.slices.iter().map(|s| s.cpu_usage_usec),
            )),
            Arc::new(UInt64Array::from_iter_values(
                snapshot.slices.iter().map(|s| s.io_write_bytes),
            )),
            Arc::new(UInt64Array::from_iter_values(
                snapshot.slices.iter().map(|s| s.pids_current),
            )),
            Arc::new(UInt64Array::from(vec![
                snapshot.memory.swap_used_bytes;
                len
            ])),
            Arc::new(UInt64Array::from(vec![
                snapshot.memory.mem_available_bytes;
                len
            ])),
        ],
    )
    .map_err(|e| e.to_string())
}

/// Converts a `BlameReport` into an Arrow `RecordBatch`.
pub fn blame_to_record_batch(report: &BlameReport) -> Result<RecordBatch, String> {
    let schema = BLAME_SCHEMA.clone();
    let len = report.entries.len();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(UInt32Array::from_iter_values(
                report.entries.iter().map(|e| e.rank),
            )),
            Arc::new(StringArray::from(vec![report.resource.as_str(); len])),
            Arc::new(StringArray::from_iter_values(
                report.entries.iter().map(|e| e.node_id.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                report.entries.iter().map(|e| e.label.as_str()),
            )),
            Arc::new(UInt64Array::from_iter_values(
                report.entries.iter().map(|e| e.contribution_value),
            )),
            Arc::new(StringArray::from_iter_values(
                report.entries.iter().map(|e| e.unit_label.as_str()),
            )),
            Arc::new(Float64Array::from_iter_values(
                report.entries.iter().map(|e| e.share_pct),
            )),
            Arc::new(UInt32Array::from(
                report.entries.iter().map(|e| e.pid).collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                report
                    .entries
                    .iter()
                    .map(|e| e.comm.as_deref())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                report
                    .entries
                    .iter()
                    .map(|e| e.unit.as_deref())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from_iter_values(
                report.entries.iter().map(|e| e.slice.as_str()),
            )),
            Arc::new(StringArray::from(
                report
                    .entries
                    .iter()
                    .map(|e| e.agent.as_deref())
                    .collect::<Vec<_>>(),
            )),
            Arc::new(StringArray::from(
                report
                    .entries
                    .iter()
                    .map(|e| e.target_path.as_deref())
                    .collect::<Vec<_>>(),
            )),
        ],
    )
    .map_err(|e| e.to_string())
}

/// Converts a `DriftReport` into an Arrow `RecordBatch`.
pub fn drift_to_record_batch(report: &DriftReport) -> Result<RecordBatch, String> {
    let findings: &[PillarFinding] = &report.findings;
    RecordBatch::try_new(
        PILLAR_FINDINGS_SCHEMA.clone(),
        vec![
            Arc::new(StringArray::from_iter_values(
                findings.iter().map(|f| f.pillar.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                findings.iter().map(|f| f.severity.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                findings.iter().map(|f| f.target.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                findings.iter().map(|f| f.summary.as_str()),
            )),
            Arc::new(Float64Array::from_iter_values(
                findings.iter().map(|f| f.metric_value),
            )),
            Arc::new(Float64Array::from_iter_values(
                findings.iter().map(|f| f.threshold_value),
            )),
        ],
    )
    .map_err(|e| e.to_string())
}

/// Converts a `DoctorReport` into an Arrow `RecordBatch`.
pub fn doctor_to_record_batch(report: &DoctorReport) -> Result<RecordBatch, String> {
    let len = report.checks.len();
    RecordBatch::try_new(
        DOCTOR_SCHEMA.clone(),
        vec![
            Arc::new(StringArray::from_iter_values(
                report.checks.iter().map(|c| c.name.as_str()),
            )),
            Arc::new(BooleanArray::from_iter(
                report.checks.iter().map(|c| Some(c.passed)),
            )),
            Arc::new(StringArray::from_iter_values(
                report.checks.iter().map(|c| c.actual.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                report.checks.iter().map(|c| c.threshold.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                report.checks.iter().map(|c| c.detail.as_str()),
            )),
            Arc::new(BooleanArray::from_iter(vec![Some(report.healthy); len])),
            Arc::new(UInt64Array::from(vec![report.rss_bytes; len])),
            Arc::new(UInt32Array::from(vec![report.open_fds; len])),
            Arc::new(UInt32Array::from(vec![report.tmp_files_created; len])),
            Arc::new(BooleanArray::from_iter(vec![
                Some(
                    report.fabrico11y_slice_confined
                );
                len
            ])),
        ],
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::ipc::reader::StreamReader;
    use std::io::Cursor;

    #[test]
    fn arrow_ipc_stream_roundtrip() {
        let rows = vec![MetricPoint {
            timestamp_ms: 1_700_000_000_000,
            name: "cgroup.memory.current".into(),
            value: 536_870_912.0,
            host: "fedora-blade".into(),
            slice: "agent.slice".into(),
            unit: "synth.scope".into(),
            pid: Some(4242),
            labels_json: "{}".into(),
        }];
        let batch = metrics_to_record_batch(&rows).unwrap();
        let bytes = encode_batches_to_ipc_stream(&METRICS_SCHEMA, &[batch]).unwrap();
        let reader = StreamReader::try_new(Cursor::new(bytes), None).unwrap();
        let decoded: Vec<RecordBatch> = reader.collect::<Result<_, _>>().unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].num_rows(), 1);
    }
}
