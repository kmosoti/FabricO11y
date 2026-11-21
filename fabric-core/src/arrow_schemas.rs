//! Arrow schemas, RecordBatch builders, and IPC stream serialization.

use crate::anomaly::{DriftReport, PillarFinding};
use crate::model::{
    AgentSessionRecord, BlameReport, DoctorReport, FsChurnEntry, FsEventRecord, GraphEdgeRecord,
    HostSnapshot, LogRecord, MetricPoint, TraceSpanRecord, UnitRecord,
};
use arrow::array::{
    BooleanArray, BooleanBuilder, Float64Array, Float64Builder, Int64Array, Int64Builder,
    StringArray, StringBuilder, UInt32Array, UInt32Builder, UInt64Array, UInt64Builder,
};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use std::fmt::Write as _;
use std::sync::{Arc, LazyLock};

const LIVE_ARRAY_STRUCT_OVERHEAD_BYTES: usize = 256;
const RECORD_BATCH_BASE_OVERHEAD_BYTES: usize = 128;
const RECORD_BATCH_COL_REF_OVERHEAD_BYTES: usize = 64;

/// Estimates retained heap footprint for an Arrow `RecordBatch` including
/// `RecordBatch`, `Arc<dyn Array>`, schema ref, and array buffer memory.
#[must_use]
pub fn retained_record_batch_bytes(batch: &RecordBatch) -> usize {
    batch
        .get_array_memory_size()
        .saturating_add(RECORD_BATCH_BASE_OVERHEAD_BYTES)
        .saturating_add(
            batch
                .num_columns()
                .saturating_mul(RECORD_BATCH_COL_REF_OVERHEAD_BYTES),
        )
}

fn null_bitmap_bytes(rows: usize) -> usize {
    if rows == 0 {
        0
    } else {
        rows.div_ceil(8).next_multiple_of(64)
    }
}

fn utf8_column_bytes(rows: usize, data_bytes: usize, nullable: bool) -> usize {
    let offsets = rows.saturating_add(1).saturating_mul(4);
    let nulls = if nullable { null_bitmap_bytes(rows) } else { 0 };
    offsets
        .saturating_add(data_bytes)
        .saturating_add(nulls)
        .saturating_add(LIVE_ARRAY_STRUCT_OVERHEAD_BYTES)
}

fn checked_utf8_capacity(total_bytes: usize, column: &str) -> Result<usize, String> {
    if i32::try_from(total_bytes).is_err() {
        return Err(format!(
            "Arrow Utf8 column '{column}' byte length {total_bytes} exceeds i32::MAX"
        ));
    }
    Ok(total_bytes)
}

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

/// Estimates peak Arrow allocation bytes for [`metrics_to_record_batch`].
#[must_use]
pub fn estimate_metrics_batch_peak_bytes(rows: &[MetricPoint]) -> usize {
    let n = rows.len();
    let mut name_bytes = 0_usize;
    let mut host_bytes = 0_usize;
    let mut slice_bytes = 0_usize;
    let mut unit_bytes = 0_usize;
    let mut labels_bytes = 0_usize;
    for r in rows {
        name_bytes = name_bytes.saturating_add(r.name.len());
        host_bytes = host_bytes.saturating_add(r.host.len());
        slice_bytes = slice_bytes.saturating_add(r.slice.len());
        unit_bytes = unit_bytes.saturating_add(r.unit.len());
        labels_bytes = labels_bytes.saturating_add(r.labels_json.len());
    }
    let timestamp_col = n
        .saturating_mul(8)
        .saturating_add(LIVE_ARRAY_STRUCT_OVERHEAD_BYTES);
    let value_col = n
        .saturating_mul(8)
        .saturating_add(LIVE_ARRAY_STRUCT_OVERHEAD_BYTES);
    let pid_col = n
        .saturating_mul(4)
        .saturating_add(null_bitmap_bytes(n))
        .saturating_add(LIVE_ARRAY_STRUCT_OVERHEAD_BYTES);

    timestamp_col
        .saturating_add(utf8_column_bytes(n, name_bytes, false))
        .saturating_add(value_col)
        .saturating_add(utf8_column_bytes(n, host_bytes, false))
        .saturating_add(utf8_column_bytes(n, slice_bytes, false))
        .saturating_add(utf8_column_bytes(n, unit_bytes, false))
        .saturating_add(pid_col)
        .saturating_add(utf8_column_bytes(n, labels_bytes, false))
        .saturating_add(RECORD_BATCH_BASE_OVERHEAD_BYTES)
}

/// Converts metric points into an Arrow `RecordBatch`.
pub fn metrics_to_record_batch(rows: &[MetricPoint]) -> Result<RecordBatch, String> {
    let n = rows.len();
    let mut name_bytes = 0_usize;
    let mut host_bytes = 0_usize;
    let mut slice_bytes = 0_usize;
    let mut unit_bytes = 0_usize;
    let mut labels_bytes = 0_usize;
    for r in rows {
        name_bytes = name_bytes.saturating_add(r.name.len());
        host_bytes = host_bytes.saturating_add(r.host.len());
        slice_bytes = slice_bytes.saturating_add(r.slice.len());
        unit_bytes = unit_bytes.saturating_add(r.unit.len());
        labels_bytes = labels_bytes.saturating_add(r.labels_json.len());
    }

    let mut ts_b = Int64Builder::with_capacity(n);
    let mut name_b = StringBuilder::with_capacity(n, checked_utf8_capacity(name_bytes, "name")?);
    let mut val_b = Float64Builder::with_capacity(n);
    let mut host_b = StringBuilder::with_capacity(n, checked_utf8_capacity(host_bytes, "host")?);
    let mut slice_b = StringBuilder::with_capacity(n, checked_utf8_capacity(slice_bytes, "slice")?);
    let mut unit_b = StringBuilder::with_capacity(n, checked_utf8_capacity(unit_bytes, "unit")?);
    let mut pid_b = UInt32Builder::with_capacity(n);
    let mut labels_b =
        StringBuilder::with_capacity(n, checked_utf8_capacity(labels_bytes, "labels_json")?);

    for r in rows {
        ts_b.append_value(r.timestamp_ms);
        name_b.append_value(&r.name);
        val_b.append_value(r.value);
        host_b.append_value(&r.host);
        slice_b.append_value(&r.slice);
        unit_b.append_value(&r.unit);
        pid_b.append_option(r.pid);
        labels_b.append_value(&r.labels_json);
    }

    let schema = METRICS_SCHEMA.clone();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(ts_b.finish()),
            Arc::new(name_b.finish()),
            Arc::new(val_b.finish()),
            Arc::new(host_b.finish()),
            Arc::new(slice_b.finish()),
            Arc::new(unit_b.finish()),
            Arc::new(pid_b.finish()),
            Arc::new(labels_b.finish()),
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
            Arc::new(UInt32Array::from_iter(rows.iter().map(|r| r.owner_pid))),
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

/// Estimates peak Arrow allocation bytes for [`fs_churn_to_record_batch`].
#[must_use]
pub fn estimate_fs_churn_batch_peak_bytes(rows: &[FsChurnEntry]) -> usize {
    let n = rows.len();
    let mut path_bytes = 0_usize;
    let mut mount_bytes = 0_usize;
    let mut kind_bytes = 0_usize;
    let mut unit_bytes = 0_usize;
    let mut slice_bytes = 0_usize;
    for entry in rows {
        path_bytes = path_bytes.saturating_add(entry.path.len());
        mount_bytes = mount_bytes.saturating_add(entry.mount_point.len());
        kind_bytes = kind_bytes.saturating_add(if entry.unlinked_open_fds > 0 {
            "unlinked_open_fd".len()
        } else {
            "churn".len()
        });
        unit_bytes = unit_bytes.saturating_add(entry.owner_unit.as_deref().map_or(0, str::len));
        slice_bytes = slice_bytes.saturating_add(entry.owner_slice.as_deref().map_or(0, str::len));
    }
    let timestamp_col = n
        .saturating_mul(8)
        .saturating_add(LIVE_ARRAY_STRUCT_OVERHEAD_BYTES);
    let size_col = n
        .saturating_mul(8)
        .saturating_add(LIVE_ARRAY_STRUCT_OVERHEAD_BYTES);
    let delta_col = n
        .saturating_mul(8)
        .saturating_add(LIVE_ARRAY_STRUCT_OVERHEAD_BYTES);
    let pid_col = n
        .saturating_mul(4)
        .saturating_add(null_bitmap_bytes(n))
        .saturating_add(LIVE_ARRAY_STRUCT_OVERHEAD_BYTES);

    timestamp_col
        .saturating_add(utf8_column_bytes(n, path_bytes, false))
        .saturating_add(utf8_column_bytes(n, mount_bytes, false))
        .saturating_add(utf8_column_bytes(n, kind_bytes, false))
        .saturating_add(size_col)
        .saturating_add(delta_col)
        .saturating_add(pid_col)
        .saturating_add(utf8_column_bytes(n, unit_bytes, false))
        .saturating_add(utf8_column_bytes(n, slice_bytes, false))
        .saturating_add(RECORD_BATCH_BASE_OVERHEAD_BYTES)
}

/// Converts `FsChurnEntry` records into an `FS_EVENTS_SCHEMA` Arrow `RecordBatch`.
pub fn fs_churn_to_record_batch(
    timestamp_ms: i64,
    rows: &[FsChurnEntry],
) -> Result<RecordBatch, String> {
    let n = rows.len();
    let mut path_bytes = 0_usize;
    let mut mount_bytes = 0_usize;
    let mut kind_bytes = 0_usize;
    let mut unit_bytes = 0_usize;
    let mut slice_bytes = 0_usize;
    for entry in rows {
        path_bytes = path_bytes.saturating_add(entry.path.len());
        mount_bytes = mount_bytes.saturating_add(entry.mount_point.len());
        kind_bytes = kind_bytes.saturating_add(if entry.unlinked_open_fds > 0 {
            "unlinked_open_fd".len()
        } else {
            "churn".len()
        });
        unit_bytes = unit_bytes.saturating_add(entry.owner_unit.as_deref().map_or(0, str::len));
        slice_bytes = slice_bytes.saturating_add(entry.owner_slice.as_deref().map_or(0, str::len));
    }

    let mut ts_b = Int64Builder::with_capacity(n);
    let mut path_b = StringBuilder::with_capacity(n, checked_utf8_capacity(path_bytes, "path")?);
    let mut mount_b =
        StringBuilder::with_capacity(n, checked_utf8_capacity(mount_bytes, "mount_point")?);
    let mut kind_b =
        StringBuilder::with_capacity(n, checked_utf8_capacity(kind_bytes, "event_kind")?);
    let mut size_b = UInt64Builder::with_capacity(n);
    let mut delta_b = Int64Builder::with_capacity(n);
    let mut pid_b = UInt32Builder::with_capacity(n);
    let mut unit_b =
        StringBuilder::with_capacity(n, checked_utf8_capacity(unit_bytes, "owner_unit")?);
    let mut slice_b =
        StringBuilder::with_capacity(n, checked_utf8_capacity(slice_bytes, "owner_slice")?);

    for entry in rows {
        ts_b.append_value(if entry.mtime_unix_ms > 0 {
            entry.mtime_unix_ms
        } else {
            timestamp_ms
        });
        path_b.append_value(&entry.path);
        mount_b.append_value(&entry.mount_point);
        kind_b.append_value(if entry.unlinked_open_fds > 0 {
            "unlinked_open_fd"
        } else {
            "churn"
        });
        size_b.append_value(entry.size_bytes);
        delta_b.append_value(entry.recent_delta_bytes);
        pid_b.append_option(entry.owner_pid);
        unit_b.append_value(entry.owner_unit.as_deref().unwrap_or(""));
        slice_b.append_value(entry.owner_slice.as_deref().unwrap_or(""));
    }

    RecordBatch::try_new(
        FS_EVENTS_SCHEMA.clone(),
        vec![
            Arc::new(ts_b.finish()),
            Arc::new(path_b.finish()),
            Arc::new(mount_b.finish()),
            Arc::new(kind_b.finish()),
            Arc::new(size_b.finish()),
            Arc::new(delta_b.finish()),
            Arc::new(pid_b.finish()),
            Arc::new(unit_b.finish()),
            Arc::new(slice_b.finish()),
        ],
    )
    .map_err(|e| e.to_string())
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

/// Estimates peak Arrow allocation bytes for [`units_to_record_batch`].
#[must_use]
pub fn estimate_units_batch_peak_bytes(rows: &[UnitRecord]) -> usize {
    let n = rows.len();
    let mut name_bytes = 0_usize;
    let mut manager_bytes = 0_usize;
    let mut slice_bytes = 0_usize;
    let mut active_bytes = 0_usize;
    let mut sub_bytes = 0_usize;
    let mut orphan_bytes = 0_usize;
    for r in rows {
        name_bytes = name_bytes.saturating_add(r.name.len());
        manager_bytes = manager_bytes.saturating_add(r.manager.len());
        slice_bytes = slice_bytes.saturating_add(r.slice.len());
        active_bytes = active_bytes.saturating_add(r.active_state.len());
        sub_bytes = sub_bytes.saturating_add(r.sub_state.len());
        orphan_bytes =
            orphan_bytes.saturating_add(r.orphaned_reason.as_deref().map_or(0, str::len));
    }
    let bool_col = null_bitmap_bytes(n).saturating_add(LIVE_ARRAY_STRUCT_OVERHEAD_BYTES);
    let pid_col = n
        .saturating_mul(4)
        .saturating_add(null_bitmap_bytes(n))
        .saturating_add(LIVE_ARRAY_STRUCT_OVERHEAD_BYTES);
    let u64_col = n
        .saturating_mul(8)
        .saturating_add(LIVE_ARRAY_STRUCT_OVERHEAD_BYTES);

    utf8_column_bytes(n, name_bytes, false)
        .saturating_add(utf8_column_bytes(n, manager_bytes, false))
        .saturating_add(utf8_column_bytes(n, slice_bytes, false))
        .saturating_add(utf8_column_bytes(n, active_bytes, false))
        .saturating_add(utf8_column_bytes(n, sub_bytes, false))
        .saturating_add(bool_col.saturating_mul(3))
        .saturating_add(utf8_column_bytes(n, orphan_bytes, true))
        .saturating_add(pid_col)
        .saturating_add(u64_col.saturating_mul(2))
        .saturating_add(RECORD_BATCH_BASE_OVERHEAD_BYTES)
}

/// Converts unit hygiene records into an Arrow `RecordBatch`.
pub fn units_to_record_batch(rows: &[UnitRecord]) -> Result<RecordBatch, String> {
    let n = rows.len();
    let mut name_bytes = 0_usize;
    let mut manager_bytes = 0_usize;
    let mut slice_bytes = 0_usize;
    let mut active_bytes = 0_usize;
    let mut sub_bytes = 0_usize;
    let mut orphan_bytes = 0_usize;
    for r in rows {
        name_bytes = name_bytes.saturating_add(r.name.len());
        manager_bytes = manager_bytes.saturating_add(r.manager.len());
        slice_bytes = slice_bytes.saturating_add(r.slice.len());
        active_bytes = active_bytes.saturating_add(r.active_state.len());
        sub_bytes = sub_bytes.saturating_add(r.sub_state.len());
        orphan_bytes =
            orphan_bytes.saturating_add(r.orphaned_reason.as_deref().map_or(0, str::len));
    }

    let mut name_b = StringBuilder::with_capacity(n, checked_utf8_capacity(name_bytes, "name")?);
    let mut manager_b =
        StringBuilder::with_capacity(n, checked_utf8_capacity(manager_bytes, "manager")?);
    let mut slice_b = StringBuilder::with_capacity(n, checked_utf8_capacity(slice_bytes, "slice")?);
    let mut active_b =
        StringBuilder::with_capacity(n, checked_utf8_capacity(active_bytes, "active_state")?);
    let mut sub_b = StringBuilder::with_capacity(n, checked_utf8_capacity(sub_bytes, "sub_state")?);
    let mut transient_b = BooleanBuilder::with_capacity(n);
    let mut collect_b = BooleanBuilder::with_capacity(n);
    let mut orphan_b =
        StringBuilder::with_capacity(n, checked_utf8_capacity(orphan_bytes, "orphaned_reason")?);
    let mut pid_b = UInt32Builder::with_capacity(n);
    let mut mem_b = UInt64Builder::with_capacity(n);
    let mut cpu_b = UInt64Builder::with_capacity(n);
    let mut pop_b = BooleanBuilder::with_capacity(n);

    for r in rows {
        name_b.append_value(&r.name);
        manager_b.append_value(&r.manager);
        slice_b.append_value(&r.slice);
        active_b.append_value(&r.active_state);
        sub_b.append_value(&r.sub_state);
        transient_b.append_value(r.transient);
        collect_b.append_value(r.collect_mode_enabled);
        orphan_b.append_option(r.orphaned_reason.as_deref());
        pid_b.append_option(r.main_pid);
        mem_b.append_value(r.memory_current_bytes);
        cpu_b.append_value(r.cpu_usage_usec);
        pop_b.append_value(r.populated);
    }

    let schema = UNITS_SCHEMA.clone();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(name_b.finish()),
            Arc::new(manager_b.finish()),
            Arc::new(slice_b.finish()),
            Arc::new(active_b.finish()),
            Arc::new(sub_b.finish()),
            Arc::new(transient_b.finish()),
            Arc::new(collect_b.finish()),
            Arc::new(orphan_b.finish()),
            Arc::new(pid_b.finish()),
            Arc::new(mem_b.finish()),
            Arc::new(cpu_b.finish()),
            Arc::new(pop_b.finish()),
        ],
    )
    .map_err(|e| e.to_string())
}

fn agent_touched_paths_joined_len(paths: &[String]) -> usize {
    if paths.is_empty() {
        0
    } else {
        paths
            .iter()
            .map(String::len)
            .fold(0_usize, usize::saturating_add)
            .saturating_add(paths.len() - 1)
    }
}

/// Estimates peak Arrow allocation bytes for [`agents_to_record_batch`].
#[must_use]
pub fn estimate_agents_batch_peak_bytes(rows: &[AgentSessionRecord]) -> usize {
    let n = rows.len();
    let mut session_bytes = 0_usize;
    let mut agent_bytes = 0_usize;
    let mut slice_bytes = 0_usize;
    let mut unit_bytes = 0_usize;
    let mut touched_bytes = 0_usize;
    for r in rows {
        session_bytes = session_bytes.saturating_add(r.session_id.len());
        agent_bytes = agent_bytes.saturating_add(r.agent_name.len());
        slice_bytes = slice_bytes.saturating_add(r.slice.len());
        unit_bytes = unit_bytes.saturating_add(r.unit.as_deref().map_or(0, str::len));
        touched_bytes =
            touched_bytes.saturating_add(agent_touched_paths_joined_len(&r.touched_paths));
    }
    let u64_col = n
        .saturating_mul(8)
        .saturating_add(LIVE_ARRAY_STRUCT_OVERHEAD_BYTES);

    utf8_column_bytes(n, session_bytes, false)
        .saturating_add(utf8_column_bytes(n, agent_bytes, false))
        .saturating_add(utf8_column_bytes(n, slice_bytes, false))
        .saturating_add(utf8_column_bytes(n, unit_bytes, true))
        .saturating_add(u64_col.saturating_mul(3))
        .saturating_add(utf8_column_bytes(n, touched_bytes, false))
        .saturating_add(RECORD_BATCH_BASE_OVERHEAD_BYTES)
}

/// Converts agent session summaries into an Arrow `RecordBatch`.
pub fn agents_to_record_batch(rows: &[AgentSessionRecord]) -> Result<RecordBatch, String> {
    let n = rows.len();
    let mut session_bytes = 0_usize;
    let mut agent_bytes = 0_usize;
    let mut slice_bytes = 0_usize;
    let mut unit_bytes = 0_usize;
    let mut touched_bytes = 0_usize;
    for r in rows {
        session_bytes = session_bytes.saturating_add(r.session_id.len());
        agent_bytes = agent_bytes.saturating_add(r.agent_name.len());
        slice_bytes = slice_bytes.saturating_add(r.slice.len());
        unit_bytes = unit_bytes.saturating_add(r.unit.as_deref().map_or(0, str::len));
        touched_bytes =
            touched_bytes.saturating_add(agent_touched_paths_joined_len(&r.touched_paths));
    }

    let mut session_b =
        StringBuilder::with_capacity(n, checked_utf8_capacity(session_bytes, "session_id")?);
    let mut agent_b =
        StringBuilder::with_capacity(n, checked_utf8_capacity(agent_bytes, "agent_name")?);
    let mut slice_b = StringBuilder::with_capacity(n, checked_utf8_capacity(slice_bytes, "slice")?);
    let mut unit_b = StringBuilder::with_capacity(n, checked_utf8_capacity(unit_bytes, "unit")?);
    let mut pid_cnt_b = UInt64Builder::with_capacity(n);
    let mut rss_b = UInt64Builder::with_capacity(n);
    let mut write_b = UInt64Builder::with_capacity(n);
    let mut touched_b =
        StringBuilder::with_capacity(n, checked_utf8_capacity(touched_bytes, "touched_paths")?);

    for r in rows {
        session_b.append_value(&r.session_id);
        agent_b.append_value(&r.agent_name);
        slice_b.append_value(&r.slice);
        unit_b.append_option(r.unit.as_deref());
        pid_cnt_b.append_value(r.pids.len() as u64);
        rss_b.append_value(r.rss_bytes);
        write_b.append_value(r.write_bytes);
        for (idx, path) in r.touched_paths.iter().enumerate() {
            if idx > 0 {
                touched_b.write_str(";").map_err(|e| e.to_string())?;
            }
            touched_b.write_str(path).map_err(|e| e.to_string())?;
        }
        touched_b.append_value("");
    }

    let schema = AGENTS_SCHEMA.clone();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(session_b.finish()),
            Arc::new(agent_b.finish()),
            Arc::new(slice_b.finish()),
            Arc::new(unit_b.finish()),
            Arc::new(pid_cnt_b.finish()),
            Arc::new(rss_b.finish()),
            Arc::new(write_b.finish()),
            Arc::new(touched_b.finish()),
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
            host: "workstation-01".into(),
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
