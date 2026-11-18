//! Unified DataFusion/Arrow query engine spanning live state, `HotMetricRing`,
//! and ZSTD date-partitioned Parquet partitions across `metrics`, `logs`,
//! `traces`, `units`, `fs`, and `agents`.

use arrow::array::{
    Array, ArrayData, ArrayRef, BinaryViewArray, BooleanArray, Float64Array, Int64Array,
    StringArray, StringViewArray, UInt32Array, UInt64Array, make_array,
};
use arrow::compute::cast;
use arrow::datatypes::{DataType, Field, FieldRef, Schema, SchemaRef};
use arrow::record_batch::{RecordBatch, RecordBatchOptions};
use datafusion::catalog::{Session, TableProvider};
use datafusion::common::DataFusionError;
use datafusion::common::tree_node::TreeNodeRecursion;
use datafusion::execution::disk_manager::{DiskManagerBuilder, DiskManagerMode};
use datafusion::execution::memory_pool::{
    GreedyMemoryPool, MemoryConsumer, MemoryPool, MemoryReservation,
};
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::execution::{SendableRecordBatchStream, TaskContext};
use datafusion::logical_expr::{ColumnarValue, Expr, TableType};
use datafusion::physical_expr::EquivalenceProperties;
use datafusion::physical_expr::expressions::Column;
use datafusion::physical_plan::execution_plan::{Boundedness, EmissionType};
use datafusion::physical_plan::projection::{ProjectionExec, ProjectionExpr};
use datafusion::physical_plan::stream::RecordBatchReceiverStreamBuilder;
use datafusion::physical_plan::{
    DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PhysicalExpr, PlanProperties,
    ReplaceChildrenOptions, collect,
};
use datafusion::prelude::{SessionConfig, SessionContext};
use fabric_core::arrow_schemas::{
    AGENTS_SCHEMA, FS_EVENTS_SCHEMA, LOGS_SCHEMA, METRICS_SCHEMA, TRACES_SCHEMA, UNITS_SCHEMA,
    agents_to_record_batch, estimate_agents_batch_peak_bytes, estimate_fs_churn_batch_peak_bytes,
    estimate_metrics_batch_peak_bytes, estimate_units_batch_peak_bytes, fs_churn_to_record_batch,
    metrics_to_record_batch, retained_record_batch_bytes, units_to_record_batch,
};
pub use fabric_core::query_spec::{
    QueryDomain, UnifiedQuerySpec, build_unified_sql, parse_duration_ms,
};
use fabric_core::{
    HostSnapshot, MetricPoint, SPINDLE_PROVENANCE_COLUMN, SpindleProvenance, StorageLayout,
};
use fabric_store::parquet_guard::{
    ColumnPageScanStats, PINNED_FOOTER_MIN_BYTES, checked_column_byte_range,
    column_peak_decode_fixed_row_bytes, estimate_arrow_schema_bytes,
    estimate_kv_metadata_clone_bytes, inspect_parquet_footer_bounds, read_parquet_footer_slice,
    scan_column_chunk_page_headers, scan_parquet_thrift_footer_slice,
    validate_non_nested_parquet_file_metadata, validate_parquet_metadata_invariants,
    validate_projected_row_group_pages, validate_provenance_compatible_parquet_schema,
};
use fabric_store::{
    MAX_SNAPSHOT_METADATA_BYTES, MAX_SNAPSHOT_PARQUET_FILES, OlapParquetStore, PinnedParquetFile,
    snapshot_partitions_retained_heap_bytes, snapshot_retained_heap_bytes,
    validated_storage_layout,
};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ParquetRecordBatchReaderBuilder};
use parquet::arrow::push_decoder::PushBuffers;
use parquet::basic::Type as ParquetPhysicalType;
use parquet::file::metadata::{ParquetMetaDataReader, RowGroupMetaData};
use parquet::file::reader::ChunkReader;
use std::fmt;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const UNIFIED_POOL_BYTES: usize = 64 * 1024 * 1024;
const MAX_UNIFIED_QUERY_RESULT_BYTES: usize = 32 * 1024 * 1024;
const UNIFIED_SPILL_BYTES: u64 = 256 * 1024 * 1024;
const MAX_SPINDLE_PROVENANCE_BYTES: usize = 4 * 1024;
static NEXT_RESULT_BUDGET_EXPR_ID: AtomicU64 = AtomicU64::new(1);

struct ResultBudgetState {
    inner: Mutex<(MemoryReservation, usize)>,
    max_bytes: usize,
}

impl fmt::Debug for ResultBudgetState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResultBudgetState")
            .field("max_bytes", &self.max_bytes)
            .finish()
    }
}

impl ResultBudgetState {
    fn new(reservation: MemoryReservation, max_bytes: usize) -> Self {
        Self {
            inner: Mutex::new((reservation, 0)),
            max_bytes,
        }
    }

    fn reserve_batch(&self, batch: &RecordBatch) -> datafusion::common::Result<()> {
        let need = estimate_retained_batch_and_json_bytes(batch);
        let mut guard = self.inner.lock().map_err(|_| {
            DataFusionError::Internal("unified query result budget mutex poisoned".to_owned())
        })?;
        let next_used = guard
            .1
            .checked_add(need)
            .filter(|&u| u <= self.max_bytes)
            .ok_or_else(|| {
                DataFusionError::ResourcesExhausted(format!(
                    "unified query result exceeds maximum retained budget ({} bytes)",
                    self.max_bytes
                ))
            })?;
        guard.0.try_grow(need)?;
        guard.1 = next_used;
        Ok(())
    }
}

#[derive(Debug)]
struct ResultBudgetExpr {
    id: u64,
    col_idx: usize,
    field: FieldRef,
    state: Arc<ResultBudgetState>,
}

impl PartialEq for ResultBudgetExpr {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.col_idx == other.col_idx
    }
}

impl Eq for ResultBudgetExpr {}

impl Hash for ResultBudgetExpr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.col_idx.hash(state);
    }
}

impl fmt::Display for ResultBudgetExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.field.name(), self.col_idx)
    }
}

impl PhysicalExpr for ResultBudgetExpr {
    fn data_type(&self, _input_schema: &Schema) -> datafusion::common::Result<DataType> {
        Ok(self.field.data_type().clone())
    }

    fn nullable(&self, _input_schema: &Schema) -> datafusion::common::Result<bool> {
        Ok(self.field.is_nullable())
    }

    fn return_field(&self, _input_schema: &Schema) -> datafusion::common::Result<FieldRef> {
        Ok(Arc::clone(&self.field))
    }

    fn evaluate(&self, batch: &RecordBatch) -> datafusion::common::Result<ColumnarValue> {
        let col = batch.columns().get(self.col_idx).ok_or_else(|| {
            DataFusionError::Internal(format!(
                "ResultBudgetExpr column index {} out of bounds for {}-column batch",
                self.col_idx,
                batch.num_columns()
            ))
        })?;
        self.state.reserve_batch(batch)?;
        Ok(ColumnarValue::Array(Arc::clone(col)))
    }

    fn children(&self) -> Vec<&Arc<dyn PhysicalExpr>> {
        Vec::new()
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn PhysicalExpr>>,
    ) -> datafusion::common::Result<Arc<dyn PhysicalExpr>> {
        if !children.is_empty() {
            return Err(DataFusionError::Internal(
                "ResultBudgetExpr does not accept children".to_owned(),
            ));
        }
        Ok(self)
    }

    fn fmt_sql(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.field.name())
    }

    fn is_volatile_node(&self) -> bool {
        true
    }
}

fn batch_json_cell_strings_bytes(batch: &RecordBatch) -> usize {
    let mut json_string_bytes = 0_usize;
    for col in batch.columns() {
        if let Some(sa) = col.as_any().downcast_ref::<StringArray>() {
            for row_idx in 0..batch.num_rows() {
                if !sa.is_null(row_idx) {
                    json_string_bytes = json_string_bytes.saturating_add(sa.value(row_idx).len());
                }
            }
        } else if let Some(sv) = col.as_any().downcast_ref::<StringViewArray>() {
            for row_idx in 0..batch.num_rows() {
                if !sv.is_null(row_idx) {
                    json_string_bytes = json_string_bytes.saturating_add(sv.value(row_idx).len());
                }
            }
        }
    }
    json_string_bytes
}

fn per_row_json_map_overhead(schema: &Schema) -> usize {
    96_usize
        .saturating_add(std::mem::size_of::<serde_json::Value>())
        .saturating_add(
            schema
                .fields()
                .iter()
                .map(|f| f.name().len().saturating_add(128))
                .fold(0_usize, usize::saturating_add),
        )
}

fn estimate_retained_batch_and_json_bytes(batch: &RecordBatch) -> usize {
    let arrow_bytes = batch.get_array_memory_size();
    if batch.num_rows() == 0 {
        return arrow_bytes;
    }
    let per_batch_overhead = 256_usize.saturating_add(batch.num_columns().saturating_mul(128));
    let per_row_overhead = per_row_json_map_overhead(batch.schema().as_ref());
    let json_string_bytes = batch_json_cell_strings_bytes(batch);
    arrow_bytes
        .saturating_add(per_batch_overhead)
        .saturating_add(batch.num_rows().saturating_mul(per_row_overhead))
        .saturating_add(json_string_bytes)
}

/// Output returned by `execute_unified_query` in both JSON and Arrow forms.
#[derive(Debug, Clone)]
pub struct UnifiedQueryResult {
    pub domain: String,
    pub row_count: usize,
    pub rows: Vec<serde_json::Value>,
    pub schema: SchemaRef,
    pub batches: Vec<RecordBatch>,
}

fn is_missing_file_error(err: &str) -> bool {
    err.contains("No such file or directory")
        || err.contains("os error 2")
        || err.contains("NotFound")
        || err.contains("unstable partition snapshot")
        || err.contains("changed concurrently during open")
}

/// Executes a bounded DataFusion/Arrow query over persisted Parquet + live ring/snapshot telemetry.
pub async fn execute_unified_query(
    data_dir: &Path,
    spec: &UnifiedQuerySpec,
    live_metrics: &[MetricPoint],
    live_snapshot: Option<&HostSnapshot>,
) -> Result<UnifiedQueryResult, String> {
    let mut last_err = None;
    for attempt in 0..3 {
        match execute_unified_query_once(data_dir, spec, live_metrics, live_snapshot).await {
            Ok(res) => return Ok(res),
            Err(err) if attempt < 2 && is_missing_file_error(&err) => {
                last_err = Some(err);
                tokio::task::yield_now().await;
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_err.unwrap_or_else(|| "unified query failed".to_owned()))
}

/// Checks exact Spindle row provenance through the same guarded Parquet reader
/// as ordinary queries. The caller must separately validate custody and the
/// committed projection frontier.
pub async fn probe_spindle_projection(
    layout: &StorageLayout,
    domain: QueryDomain,
    timestamp_ms: i64,
    key: &SpindleProvenance,
) -> Result<bool, String> {
    let (domain, schema) = match domain {
        QueryDomain::Metrics => ("metrics", METRICS_SCHEMA.clone()),
        QueryDomain::Logs => ("logs", LOGS_SCHEMA.clone()),
        QueryDomain::Traces => ("traces", TRACES_SCHEMA.clone()),
        _ => return Err("unsupported Spindle projection domain".to_owned()),
    };
    let olap = OlapParquetStore::new(layout.clone());
    let pool: Arc<dyn MemoryPool> = Arc::new(GreedyMemoryPool::new(UNIFIED_POOL_BYTES));
    let budget = Arc::new(SnapshotBudgetGuard::new(
        MemoryConsumer::new("SpindleProjectionSnapshot").register(&pool),
    ));
    let context = SessionContext::new_with_config_rt(
        SessionConfig::new().with_target_partitions(1),
        RuntimeEnvBuilder::new()
            .with_memory_pool(pool)
            .with_disk_manager_builder(
                DiskManagerBuilder::default().with_mode(DiskManagerMode::Disabled),
            )
            .build_arc()
            .map_err(|err| err.to_string())?,
    );

    for attempt in 0..3 {
        let result = probe_spindle_projection_once(
            &olap,
            domain,
            &schema,
            timestamp_ms,
            key,
            &budget,
            &context,
        )
        .await;
        match result {
            Err(err) if attempt < 2 && is_missing_file_error(&err) => {
                tokio::task::yield_now().await;
            }
            result => return result,
        }
    }
    unreachable!("the last attempt always returns")
}

async fn probe_spindle_projection_once(
    olap: &OlapParquetStore,
    domain: &str,
    schema: &SchemaRef,
    timestamp_ms: i64,
    key: &SpindleProvenance,
    budget: &Arc<SnapshotBudgetGuard>,
    context: &SessionContext,
) -> Result<bool, String> {
    // The collector replaces timestamps rounded down to zero before choosing
    // a partition. Their original Spindle payload cannot identify that day.
    let files = if timestamp_ms > 0 {
        budget.try_resize(MAX_SNAPSHOT_METADATA_BYTES)?;
        olap.snapshot_pinned_parquet_files_for_day(
            domain,
            fabric_store::olap::day_from_ms(timestamp_ms),
            MAX_SNAPSHOT_PARQUET_FILES,
            MAX_SNAPSHOT_METADATA_BYTES,
        )?
    } else {
        discover_bounded_snapshot(olap, domain, None, budget)?
    };
    if files.is_empty() {
        budget.free();
        return Ok(false);
    };
    let partitions = partition_pinned_files(files, 1, Some(budget))?;
    let plan = PinnedParquetExec::try_new_inner(
        Arc::clone(schema),
        partitions,
        None,
        None,
        Some(Arc::clone(budget)),
        true,
    )
    .map_err(|err| err.to_string())?;
    let mut stream = plan
        .execute(0, context.task_ctx())
        .map_err(|err| err.to_string())?;
    while let Some(batch) = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
        let batch = batch.map_err(|err| err.to_string())?;
        let column = batch
            .column_by_name(SPINDLE_PROVENANCE_COLUMN)
            .and_then(|column| column.as_any().downcast_ref::<StringArray>())
            .ok_or("missing Spindle row provenance in guarded batch")?;
        for raw in column.iter().flatten() {
            if raw.len() > MAX_SPINDLE_PROVENANCE_BYTES {
                return Err("Spindle row provenance exceeds its size limit".to_owned());
            }
            let provenance = serde_json::from_str::<SpindleProvenance>(raw)
                .map_err(|err| format!("invalid Spindle row provenance: {err}"))?;
            if provenance == *key {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

async fn execute_unified_query_once(
    data_dir: &Path,
    spec: &UnifiedQuerySpec,
    live_metrics: &[MetricPoint],
    live_snapshot: Option<&HostSnapshot>,
) -> Result<UnifiedQueryResult, String> {
    let layout = validated_storage_layout(data_dir)?;
    fabric_store::validate_non_tmp_path_on_disk(&layout.olap_dir()).map_err(|e| e.to_string())?;
    let spill_dir = fabric_store::ensure_durable_layout_subdir(&layout, &layout.spill_dir())?;
    let olap = OlapParquetStore::new(layout.clone());

    let pool: Arc<dyn MemoryPool> = Arc::new(GreedyMemoryPool::new(UNIFIED_POOL_BYTES));
    let mut live_input_res = MemoryConsumer::new("UnifiedQueryLiveInput").register(&pool);
    let snapshot_budget = Arc::new(SnapshotBudgetGuard::new(
        MemoryConsumer::new("UnifiedQuerySnapshot").register(&pool),
    ));
    let result_budget = Arc::new(ResultBudgetState::new(
        MemoryConsumer::new("UnifiedQueryResult").register(&pool),
        MAX_UNIFIED_QUERY_RESULT_BYTES,
    ));

    let env = RuntimeEnvBuilder::new()
        .with_memory_pool(pool)
        .with_temp_file_path(spill_dir)
        .with_max_temp_directory_size(UNIFIED_SPILL_BYTES)
        .build_arc()
        .map_err(|e| e.to_string())?;

    let cfg = SessionConfig::new()
        .with_target_partitions(2)
        .set_usize("datafusion.execution.meta_fetch_concurrency", 8);
    let ctx = SessionContext::new_with_config_rt(cfg, env);

    let default_order = match spec.domain {
        QueryDomain::Metrics => {
            register_metrics_table(
                &ctx,
                &olap,
                spec.since_ms,
                live_metrics,
                &mut live_input_res,
                &snapshot_budget,
            )
            .await?;
            Some("timestamp_ms DESC")
        }
        QueryDomain::Logs => {
            register_parquet_or_empty(
                &ctx,
                &olap,
                "logs",
                spec.since_ms,
                LOGS_SCHEMA.clone(),
                &snapshot_budget,
            )?;
            Some("timestamp_ms DESC")
        }
        QueryDomain::Traces => {
            register_parquet_or_empty(
                &ctx,
                &olap,
                "traces",
                spec.since_ms,
                TRACES_SCHEMA.clone(),
                &snapshot_budget,
            )?;
            Some("timestamp_ms DESC")
        }
        QueryDomain::Fs => {
            register_fs_table(
                &ctx,
                &olap,
                spec.since_ms,
                live_snapshot,
                &mut live_input_res,
                &snapshot_budget,
            )
            .await?;
            Some("size_bytes DESC, timestamp_ms DESC")
        }
        QueryDomain::Units => {
            let batch = match live_snapshot {
                Some(snap) if !snap.units.is_empty() => {
                    let peak_bytes = estimate_units_batch_peak_bytes(&snap.units);
                    live_input_res
                        .try_resize(peak_bytes)
                        .map_err(|e| e.to_string())?;
                    let batch = units_to_record_batch(&snap.units)?;
                    live_input_res
                        .try_resize(retained_record_batch_bytes(&batch))
                        .map_err(|e| e.to_string())?;
                    batch
                }
                _ => RecordBatch::new_empty(UNITS_SCHEMA.clone()),
            };
            ctx.register_batch("t", batch).map_err(|e| e.to_string())?;
            Some("memory_current_bytes DESC, name ASC")
        }
        QueryDomain::Agents => {
            let batch = match live_snapshot {
                Some(snap) if !snap.agents.is_empty() => {
                    let peak_bytes = estimate_agents_batch_peak_bytes(&snap.agents);
                    live_input_res
                        .try_resize(peak_bytes)
                        .map_err(|e| e.to_string())?;
                    let batch = agents_to_record_batch(&snap.agents)?;
                    live_input_res
                        .try_resize(retained_record_batch_bytes(&batch))
                        .map_err(|e| e.to_string())?;
                    batch
                }
                _ => RecordBatch::new_empty(AGENTS_SCHEMA.clone()),
            };
            ctx.register_batch("t", batch).map_err(|e| e.to_string())?;
            Some("rss_bytes DESC, session_id ASC")
        }
    };

    let sql = build_unified_sql(spec, default_order)?;
    let df = ctx
        .sql(&sql)
        .await
        .map_err(|e| format!("query error: {e}"))?;
    let schema = Arc::new(df.schema().as_arrow().clone());
    let plan = df.create_physical_plan().await.map_err(|e| e.to_string())?;
    let plan_schema = plan.schema();
    let guarded_plan: Arc<dyn ExecutionPlan> = if let Some(first_field) =
        plan_schema.fields().first()
    {
        let mut proj_exprs = Vec::with_capacity(plan_schema.fields().len());
        proj_exprs.push(ProjectionExpr {
            expr: Arc::new(ResultBudgetExpr {
                id: NEXT_RESULT_BUDGET_EXPR_ID.fetch_add(1, Ordering::Relaxed),
                col_idx: 0,
                field: Arc::clone(first_field),
                state: Arc::clone(&result_budget),
            }) as Arc<dyn PhysicalExpr>,
            alias: first_field.name().clone(),
        });
        for (idx, field) in plan_schema.fields().iter().enumerate().skip(1) {
            proj_exprs.push(ProjectionExpr {
                expr: Arc::new(Column::new(field.name(), idx)) as Arc<dyn PhysicalExpr>,
                alias: field.name().clone(),
            });
        }
        Arc::new(
            ProjectionExec::try_new_with_schema_metadata(proj_exprs, plan, plan_schema.as_ref())
                .map_err(|e| e.to_string())?,
        )
    } else {
        plan
    };
    let batches = collect(guarded_plan, ctx.task_ctx())
        .await
        .map_err(|e| e.to_string())?;
    drop(ctx);
    drop(live_input_res);
    drop(snapshot_budget);
    let rows = batches_to_json_rows_bounded(&batches, MAX_UNIFIED_QUERY_RESULT_BYTES)?;
    drop(result_budget);

    Ok(UnifiedQueryResult {
        domain: spec.domain.to_string(),
        row_count: rows.len(),
        rows,
        schema,
        batches,
    })
}

fn normalize_batch_schema(
    batch: RecordBatch,
    expected: &SchemaRef,
) -> datafusion::common::Result<RecordBatch> {
    if batch.schema().as_ref() == expected.as_ref() {
        return Ok(batch);
    }
    if batch.num_columns() != expected.fields().len() {
        return Err(DataFusionError::Execution(format!(
            "parquet batch column count {} does not match expected schema width {}",
            batch.num_columns(),
            expected.fields().len()
        )));
    }
    let cols: Vec<ArrayRef> = batch
        .columns()
        .iter()
        .zip(expected.fields().iter())
        .map(|(col, field)| {
            if col.data_type() == field.data_type() {
                Ok(Arc::clone(col))
            } else {
                cast(col.as_ref(), field.data_type()).map_err(DataFusionError::from)
            }
        })
        .collect::<datafusion::common::Result<Vec<_>>>()?;
    RecordBatch::try_new(Arc::clone(expected), cols).map_err(DataFusionError::from)
}

const PINNED_READER_RESERVATION_BYTES: usize = 16 * 1024;
const PINNED_READ_BATCH_ROWS: usize = 4096;
const PINNED_UNKNOWN_VARLEN_BATCH_ROWS: usize = 64;
const PINNED_UNKNOWN_VARLEN_TARGET_BYTES: usize = 64 * 1024;
const PINNED_TARGET_BATCH_BYTES: usize = 256 * 1024;

fn load_verified_arrow_reader_metadata(
    file: &std::fs::File,
    footer_res: &MemoryReservation,
) -> datafusion::common::Result<(u64, usize, ArrowReaderMetadata)> {
    let (file_len, footer_len) =
        inspect_parquet_footer_bounds(file).map_err(DataFusionError::Execution)?;
    footer_res.try_resize(footer_len.saturating_add(PINNED_FOOTER_MIN_BYTES))?;
    let footer_bytes = read_parquet_footer_slice(file, file_len, footer_len)
        .map_err(DataFusionError::Execution)?;
    let scan =
        scan_parquet_thrift_footer_slice(&footer_bytes).map_err(DataFusionError::Execution)?;
    let decoded_ipc_bytes = scan.decoded_arrow_ipc_bytes();
    footer_res.try_resize(
        footer_len
            .saturating_add(decoded_ipc_bytes)
            .saturating_add(PINNED_FOOTER_MIN_BYTES),
    )?;
    let est_arrow_ipc_bytes = scan
        .validate_arrow_schema_ipc()
        .map_err(DataFusionError::Execution)?;
    footer_res.try_resize(
        footer_len
            .saturating_add(scan.est_pq_meta_bytes())
            .saturating_add(PINNED_FOOTER_MIN_BYTES),
    )?;
    let pq_meta = ParquetMetaDataReader::decode_metadata(&footer_bytes)
        .map_err(|e| DataFusionError::Execution(e.to_string()))?;
    drop(footer_bytes);
    validate_non_nested_parquet_file_metadata(&pq_meta).map_err(DataFusionError::Execution)?;
    let kv_clone_bytes = estimate_kv_metadata_clone_bytes(&pq_meta);
    footer_res.try_resize(
        pq_meta
            .memory_size()
            .saturating_add(kv_clone_bytes)
            .saturating_add(decoded_ipc_bytes)
            .saturating_add(est_arrow_ipc_bytes.saturating_mul(2))
            .saturating_add(PINNED_FOOTER_MIN_BYTES),
    )?;
    let reader_meta = ArrowReaderMetadata::try_new(Arc::new(pq_meta), Default::default())
        .map_err(|e| DataFusionError::Execution(e.to_string()))?;
    let retained_footer_bytes = reader_meta
        .metadata()
        .memory_size()
        .saturating_add(estimate_arrow_schema_bytes(reader_meta.schema()))
        .saturating_add(PINNED_FOOTER_MIN_BYTES);
    footer_res.try_resize(retained_footer_bytes)?;
    Ok((file_len, footer_len, reader_meta))
}

fn estimate_row_group_decode_plan(
    rg: &RowGroupMetaData,
    parquet_arrow_schema: &Schema,
    data_section_end: u64,
    mask: &ProjectionMask,
    remaining_limit: Option<usize>,
    output_cols: usize,
    page_scans: Option<&[Option<ColumnPageScanStats>]>,
) -> datafusion::common::Result<(usize, usize, usize)> {
    let raw_rg_rows = rg.num_rows();
    if raw_rg_rows < 0 {
        return Err(DataFusionError::Execution(format!(
            "invalid negative row-group row count {raw_rg_rows}"
        )));
    }
    let rg_rows = usize::try_from(raw_rg_rows).unwrap_or(usize::MAX);
    let effective_rows = remaining_limit.map_or(rg_rows, |rem| rg_rows.min(rem));
    let mut compressed_chunks_total = 0usize;
    let mut scan_resident_total = 0usize;
    let mut varlen_uncomp_total = 0usize;
    let mut reported_unenc_total = 0usize;
    let mut fixed_row_bytes = 0usize;
    let mut included_leaves = 0usize;

    for col_idx in 0..rg.num_columns() {
        if !mask.leaf_included(col_idx) {
            continue;
        }
        included_leaves = included_leaves.saturating_add(1);
        let col = rg.column(col_idx);
        let num_values = col.num_values();
        if num_values < 0 || num_values != raw_rg_rows {
            return Err(DataFusionError::Execution(format!(
                "parquet column chunk {col_idx} num_values {num_values} mismatches row group rows {raw_rg_rows}"
            )));
        }
        let raw_comp = col.compressed_size();
        let raw_uncomp = col.uncompressed_size();
        if raw_comp < 0 || raw_uncomp < 0 {
            return Err(DataFusionError::Execution(format!(
                "parquet column chunk {col_idx} negative byte size (comp={raw_comp}, uncomp={raw_uncomp})"
            )));
        }
        let (_col_start, col_len) = checked_column_byte_range(col, col_idx, data_section_end)
            .map_err(DataFusionError::Execution)?;
        compressed_chunks_total =
            compressed_chunks_total.saturating_add(usize::try_from(col_len).unwrap_or(usize::MAX));
        let unenc_opt = match col.unencoded_byte_array_data_bytes() {
            Some(raw_unenc) if raw_unenc < 0 => {
                return Err(DataFusionError::Execution(format!(
                    "parquet column chunk {col_idx} negative unencoded byte count {raw_unenc}"
                )));
            }
            Some(raw_unenc) => Some(usize::try_from(raw_unenc).unwrap_or(usize::MAX)),
            None => None,
        };
        let footer_uncomp = usize::try_from(raw_uncomp).unwrap_or(usize::MAX);
        let scan_opt = page_scans.and_then(|scans| scans.get(col_idx).and_then(|s| *s));
        let uncomp = scan_opt.map_or(footer_uncomp, |scan| {
            footer_uncomp.max(scan.effective_payload_bytes())
        });
        scan_resident_total =
            scan_resident_total.saturating_add(scan_opt.map_or(0, |s| s.resident_bytes()));
        let arrow_type = parquet_arrow_schema
            .fields()
            .get(col_idx)
            .map(|f| f.data_type());
        let peak_fixed = column_peak_decode_fixed_row_bytes(
            col.column_type(),
            col.column_descr().type_length(),
            col.column_descr().max_def_level(),
            arrow_type,
        );
        fixed_row_bytes = fixed_row_bytes.saturating_add(peak_fixed);
        if col.column_type() == ParquetPhysicalType::BYTE_ARRAY {
            varlen_uncomp_total = varlen_uncomp_total.saturating_add(uncomp);
            if let Some(unenc) = unenc_opt {
                reported_unenc_total = reported_unenc_total.saturating_add(unenc);
            }
        }
    }

    let max_rows_cap = PINNED_UNKNOWN_VARLEN_TARGET_BYTES
        .checked_div(varlen_uncomp_total)
        .map_or(PINNED_READ_BATCH_ROWS, |rows| {
            rows.clamp(1, PINNED_UNKNOWN_VARLEN_BATCH_ROWS)
        });
    let est_row_bytes = fixed_row_bytes.saturating_add(varlen_uncomp_total).max(1);
    let batch_rows = (PINNED_TARGET_BATCH_BYTES / est_row_bytes)
        .clamp(1, max_rows_cap)
        .min(effective_rows.max(1));
    let decoded_batch_est = if rg_rows == 0 {
        0
    } else {
        let varlen_decoded_est = if batch_rows >= rg_rows {
            batch_rows
                .saturating_mul(varlen_uncomp_total)
                .max(reported_unenc_total)
        } else {
            batch_rows.saturating_mul(varlen_uncomp_total)
        };
        batch_rows
            .saturating_mul(fixed_row_bytes)
            .saturating_add(varlen_decoded_est)
    };
    let cols = output_cols.max(included_leaves).max(1);
    let arrow_overhead = cols
        .saturating_mul(256)
        .saturating_add(batch_rows.saturating_mul(cols).saturating_mul(24));
    let rg_resident_bytes = PINNED_READER_RESERVATION_BYTES
        .saturating_add(compressed_chunks_total)
        .saturating_add(scan_resident_total);
    let rg_decode_budget = rg_resident_bytes
        .saturating_add(decoded_batch_est)
        .saturating_add(arrow_overhead);
    Ok((batch_rows, rg_resident_bytes, rg_decode_budget))
}

fn logical_view_expanded_bytes(col: &ArrayRef) -> Option<usize> {
    let logical_bytes = if let Some(sv) = col.as_any().downcast_ref::<StringViewArray>() {
        (0..sv.len())
            .filter(|&i| !sv.is_null(i))
            .map(|i| sv.value(i).len())
            .fold(0usize, usize::saturating_add)
    } else {
        let bv = col.as_any().downcast_ref::<BinaryViewArray>()?;
        (0..bv.len())
            .filter(|&i| !bv.is_null(i))
            .map(|i| bv.value(i).len())
            .fold(0usize, usize::saturating_add)
    };
    Some(
        logical_bytes
            .saturating_add(col.len().saturating_add(1).saturating_mul(16))
            .saturating_add(64)
            .saturating_add(col.get_array_memory_size()),
    )
}

fn max_dictionary_values_bytes(data: &ArrayData) -> Option<usize> {
    let mut max_vals = match data.data_type() {
        DataType::Dictionary(_, _) => data.child_data().first().map(|c| {
            let arr = make_array(c.clone());
            let mut size = arr.get_array_memory_size();
            if let Some(sv) = arr.as_any().downcast_ref::<StringViewArray>() {
                size = size.max(
                    sv.iter()
                        .flatten()
                        .map(|s| s.len().saturating_add(32))
                        .max()
                        .unwrap_or(0),
                );
            } else if let Some(bv) = arr.as_any().downcast_ref::<BinaryViewArray>() {
                size = size.max(
                    bv.iter()
                        .flatten()
                        .map(|s| s.len().saturating_add(32))
                        .max()
                        .unwrap_or(0),
                );
            }
            size
        }),
        _ => None,
    };
    for child in data.child_data() {
        if let Some(child_max) = max_dictionary_values_bytes(child) {
            max_vals = Some(max_vals.map_or(child_max, |m| m.max(child_max)));
        }
    }
    max_vals
}

fn estimate_array_cast_bytes(col: &ArrayRef, target_type: &DataType) -> usize {
    if col.data_type() == target_type {
        return 0;
    }
    let mut worst = col
        .get_array_memory_size()
        .saturating_mul(2)
        .saturating_add(col.len().saturating_mul(32));
    if let Some(view_expanded) = logical_view_expanded_bytes(col) {
        worst = worst.max(view_expanded);
    }
    if let Some(dict_vals_bytes) = max_dictionary_values_bytes(&col.to_data()) {
        worst = worst.max(
            col.len()
                .saturating_mul(dict_vals_bytes.max(32))
                .saturating_add(col.len().saturating_mul(16))
                .saturating_add(col.get_array_memory_size()),
        );
    }
    worst
}

fn estimate_projection_cast_bytes(
    raw: &RecordBatch,
    unique_roots: &[usize],
    projection_indices: &[usize],
    projected_schema: &SchemaRef,
) -> datafusion::common::Result<usize> {
    if raw.num_columns() != unique_roots.len() {
        return Err(DataFusionError::Execution(format!(
            "projected parquet batch column count {} does not match expected mask width {}",
            raw.num_columns(),
            unique_roots.len()
        )));
    }
    if unique_roots == projection_indices && raw.schema().as_ref() == projected_schema.as_ref() {
        return Ok(0);
    }
    let mut extra = 0usize;
    for (out_pos, orig_root) in projection_indices.iter().enumerate() {
        let raw_pos = unique_roots.binary_search(orig_root).map_err(|_| {
            DataFusionError::Internal(format!(
                "projected root column {orig_root} missing from Parquet projection mask"
            ))
        })?;
        let col = raw.column(raw_pos);
        let target_type = projected_schema.field(out_pos).data_type();
        extra = extra.saturating_add(estimate_array_cast_bytes(col, target_type));
    }
    Ok(extra)
}

fn estimate_normalize_cast_bytes(
    batch: &RecordBatch,
    expected: &SchemaRef,
) -> datafusion::common::Result<usize> {
    if batch.schema().as_ref() == expected.as_ref() {
        return Ok(0);
    }
    if batch.num_columns() != expected.fields().len() {
        return Err(DataFusionError::Execution(format!(
            "parquet batch column count {} does not match expected schema width {}",
            batch.num_columns(),
            expected.fields().len()
        )));
    }
    let mut extra = 0usize;
    for (col, field) in batch.columns().iter().zip(expected.fields().iter()) {
        extra = extra.saturating_add(estimate_array_cast_bytes(col, field.data_type()));
    }
    Ok(extra)
}

fn build_projected_batch(
    raw: RecordBatch,
    unique_roots: &[usize],
    projection_indices: &[usize],
    projected_schema: &SchemaRef,
) -> datafusion::common::Result<RecordBatch> {
    if raw.num_columns() != unique_roots.len() {
        return Err(DataFusionError::Execution(format!(
            "projected parquet batch column count {} does not match expected mask width {}",
            raw.num_columns(),
            unique_roots.len()
        )));
    }
    if unique_roots == projection_indices && raw.schema().as_ref() == projected_schema.as_ref() {
        return Ok(raw);
    }
    let mut cols = Vec::with_capacity(projection_indices.len());
    for (out_pos, orig_root) in projection_indices.iter().enumerate() {
        let raw_pos = unique_roots.binary_search(orig_root).map_err(|_| {
            DataFusionError::Internal(format!(
                "projected root column {orig_root} missing from Parquet projection mask"
            ))
        })?;
        let col = raw.column(raw_pos);
        let target_type = projected_schema.field(out_pos).data_type();
        if col.data_type() == target_type {
            cols.push(Arc::clone(col));
        } else {
            cols.push(cast(col.as_ref(), target_type).map_err(DataFusionError::from)?);
        }
    }
    RecordBatch::try_new(Arc::clone(projected_schema), cols).map_err(DataFusionError::from)
}

#[derive(Debug)]
struct SnapshotBudgetGuard {
    reservation: Mutex<MemoryReservation>,
}

impl SnapshotBudgetGuard {
    fn new(reservation: MemoryReservation) -> Self {
        Self {
            reservation: Mutex::new(reservation),
        }
    }

    fn try_resize(&self, bytes: usize) -> Result<(), String> {
        let guard = self
            .reservation
            .lock()
            .map_err(|_| "UnifiedQuerySnapshot budget mutex poisoned".to_owned())?;
        guard.try_resize(bytes).map_err(|e| e.to_string())
    }

    fn free(&self) {
        if let Ok(guard) = self.reservation.lock() {
            guard.free();
        }
    }
}

#[derive(Debug)]
struct PinnedParquetTable {
    schema: SchemaRef,
    partitions: Arc<[Arc<[PinnedParquetFile]>]>,
    snapshot_budget: Option<Arc<SnapshotBudgetGuard>>,
}

impl TableProvider for PinnedParquetTable {
    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    fn scan<'life0, 'life1, 'life2, 'life3, 'async_trait>(
        &'life0 self,
        _state: &'life1 dyn Session,
        projection: Option<&'life2 Vec<usize>>,
        _filters: &'life3 [Expr],
        limit: Option<usize>,
    ) -> Pin<
        Box<
            dyn Future<Output = datafusion::common::Result<Arc<dyn ExecutionPlan>>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        'life3: 'async_trait,
        Self: 'async_trait,
    {
        let plan = PinnedParquetExec::try_new_with_budget(
            Arc::clone(&self.schema),
            Arc::clone(&self.partitions),
            projection.cloned(),
            limit,
            self.snapshot_budget.clone(),
        );
        Box::pin(async move { plan.map(|exec| Arc::new(exec) as Arc<dyn ExecutionPlan>) })
    }
}

#[derive(Debug, Clone)]
struct PinnedParquetExec {
    full_schema: SchemaRef,
    projected_schema: SchemaRef,
    projection: Option<Vec<usize>>,
    partitions: Arc<[Arc<[PinnedParquetFile]>]>,
    limit: Option<usize>,
    properties: Arc<PlanProperties>,
    snapshot_budget: Option<Arc<SnapshotBudgetGuard>>,
    spindle_probe: bool,
}

impl PinnedParquetExec {
    #[cfg(test)]
    fn try_new(
        full_schema: SchemaRef,
        partitions: Arc<[Arc<[PinnedParquetFile]>]>,
        projection: Option<Vec<usize>>,
        limit: Option<usize>,
    ) -> datafusion::common::Result<Self> {
        Self::try_new_with_budget(full_schema, partitions, projection, limit, None)
    }

    fn try_new_with_budget(
        full_schema: SchemaRef,
        partitions: Arc<[Arc<[PinnedParquetFile]>]>,
        projection: Option<Vec<usize>>,
        limit: Option<usize>,
        snapshot_budget: Option<Arc<SnapshotBudgetGuard>>,
    ) -> datafusion::common::Result<Self> {
        Self::try_new_inner(
            full_schema,
            partitions,
            projection,
            limit,
            snapshot_budget,
            false,
        )
    }

    fn try_new_inner(
        full_schema: SchemaRef,
        partitions: Arc<[Arc<[PinnedParquetFile]>]>,
        projection: Option<Vec<usize>>,
        limit: Option<usize>,
        snapshot_budget: Option<Arc<SnapshotBudgetGuard>>,
        spindle_probe: bool,
    ) -> datafusion::common::Result<Self> {
        let projected_schema = match projection.as_ref() {
            Some(indices) => Arc::new(full_schema.project(indices)?),
            None if spindle_probe => {
                let mut fields = full_schema.fields().to_vec();
                fields.push(Arc::new(Field::new(
                    SPINDLE_PROVENANCE_COLUMN,
                    DataType::Utf8,
                    true,
                )));
                Arc::new(Schema::new_with_metadata(
                    fields,
                    full_schema.metadata().clone(),
                ))
            }
            None => Arc::clone(&full_schema),
        };
        let properties = Arc::new(PlanProperties::new(
            EquivalenceProperties::new(Arc::clone(&projected_schema)),
            Partitioning::UnknownPartitioning(partitions.len()),
            EmissionType::Incremental,
            Boundedness::Bounded,
        ));
        Ok(Self {
            full_schema,
            projected_schema,
            projection,
            partitions,
            limit,
            properties,
            snapshot_budget,
            spindle_probe,
        })
    }
}

impl DisplayAs for PinnedParquetExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::Verbose => {
                write!(f, "PinnedParquetExec: partitions={}", self.partitions.len())?;
                if let Some(proj) = &self.projection {
                    write!(f, ", projection={proj:?}")?;
                }
                if let Some(limit) = self.limit {
                    write!(f, ", limit={limit}")?;
                }
                Ok(())
            }
            DisplayFormatType::TreeRender => write!(f, ""),
        }
    }
}

impl ExecutionPlan for PinnedParquetExec {
    fn name(&self) -> &'static str {
        "PinnedParquetExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![]
    }

    fn apply_expressions(
        &self,
        _f: &mut dyn FnMut(&Arc<dyn PhysicalExpr>) -> datafusion::common::Result<TreeNodeRecursion>,
    ) -> datafusion::common::Result<TreeNodeRecursion> {
        Ok(TreeNodeRecursion::Continue)
    }

    fn replace_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
        _options: ReplaceChildrenOptions,
    ) -> datafusion::common::Result<Arc<dyn ExecutionPlan>> {
        if !children.is_empty() {
            return Err(DataFusionError::Internal(
                "PinnedParquetExec does not accept children".to_owned(),
            ));
        }
        Ok(self)
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> datafusion::common::Result<Arc<dyn ExecutionPlan>> {
        if !children.is_empty() {
            return Err(DataFusionError::Internal(
                "PinnedParquetExec does not accept children".to_owned(),
            ));
        }
        Ok(self)
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> datafusion::common::Result<SendableRecordBatchStream> {
        let Some(files) = self.partitions.get(partition).map(Arc::clone) else {
            return Err(DataFusionError::Internal(format!(
                "PinnedParquetExec partition {partition} out of range ({})",
                self.partitions.len()
            )));
        };
        let full_schema = Arc::clone(&self.full_schema);
        let projected_schema = Arc::clone(&self.projected_schema);
        let projection = self.projection.clone();
        let limit = self.limit;
        let snapshot_budget = self.snapshot_budget.clone();
        let spindle_probe = self.spindle_probe;

        let (raw_tx, mut raw_rx) = tokio::sync::mpsc::channel::<
            datafusion::common::Result<(RecordBatch, MemoryReservation)>,
        >(1);
        let mut builder = RecordBatchReceiverStreamBuilder::new(Arc::clone(&projected_schema), 1);
        let out_tx = builder.tx();
        let footer_res = MemoryConsumer::new(format!("PinnedParquetFooter[{partition}]"))
            .register(context.memory_pool());
        let decode_res = MemoryConsumer::new(format!("PinnedParquetDecode[{partition}]"))
            .register(context.memory_pool());

        builder.spawn(async move {
            let mut in_channel_res: Option<MemoryReservation> = None;
            while let Some(item) = raw_rx.recv().await {
                match item {
                    Ok((batch, batch_res)) => {
                        let Ok(permit) = out_tx.reserve().await else {
                            return Ok(());
                        };
                        in_channel_res = Some(batch_res);
                        permit.send(Ok(batch));
                    }
                    Err(err) => {
                        let _ = out_tx.send(Err(err)).await;
                        return Ok(());
                    }
                }
            }
            if in_channel_res.is_some() {
                // Keep final batch reservation held until the receiver dequeues it
                // from the capacity-1 channel or drops the stream.
                let _ = out_tx.reserve().await;
            }
            drop(in_channel_res);
            Ok(())
        });

        builder.spawn_blocking(move || {
            let _snapshot_budget = snapshot_budget;
            let mut remaining_limit = limit;

            for pinned in files.iter() {
                if remaining_limit == Some(0) || raw_tx.is_closed() {
                    break;
                }
                let file = pinned.open_verified().map_err(DataFusionError::Execution)?;
                let (file_len, footer_len, reader_meta) =
                    load_verified_arrow_reader_metadata(&file, &footer_res)?;
                let has_provenance = validate_provenance_compatible_parquet_schema(
                    &reader_meta,
                    &full_schema,
                )
                .map_err(DataFusionError::Execution)?;
                validate_parquet_metadata_invariants(&reader_meta, file_len, footer_len)
                    .map_err(DataFusionError::Execution)?;
                if spindle_probe && !has_provenance {
                    drop(reader_meta);
                    footer_res.free();
                    continue;
                }
                let data_section_end = file_len
                    .checked_sub(8_u64.saturating_add(footer_len as u64))
                    .ok_or_else(|| {
                        DataFusionError::Execution(
                            "invalid parquet footer offset bounds".to_owned(),
                        )
                    })?;

                if let Some(indices) = projection.as_ref()
                    && indices.is_empty()
                {
                    let raw_rows = reader_meta.metadata().file_metadata().num_rows();
                    let mut file_rows = usize::try_from(raw_rows).map_err(|_| {
                        DataFusionError::Execution(format!(
                            "parquet file num_rows {raw_rows} invalid for empty projection"
                        ))
                    })?;
                    if let Some(rem) = remaining_limit.as_mut() {
                        file_rows = file_rows.min(*rem);
                        *rem = rem.saturating_sub(file_rows);
                    }
                    while file_rows > 0 {
                        if raw_tx.is_closed() {
                            decode_res.free();
                            footer_res.free();
                            return Ok(());
                        }
                        let chunk = file_rows.min(PINNED_READ_BATCH_ROWS);
                        file_rows -= chunk;
                        let batch = RecordBatch::try_new_with_options(
                            Arc::clone(&projected_schema),
                            Vec::new(),
                            &RecordBatchOptions::new().with_row_count(Some(chunk)),
                        )?;
                        let batch_bytes = batch.get_array_memory_size();
                        decode_res.try_resize(batch_bytes)?;
                        let batch_res = decode_res.split(batch_bytes);
                        if raw_tx.blocking_send(Ok((batch, batch_res))).is_err() {
                            decode_res.free();
                            footer_res.free();
                            return Ok(());
                        }
                    }
                    drop(reader_meta);
                    footer_res.free();
                    decode_res.free();
                    continue;
                }

                let (unique_roots, mask) = match projection.as_ref() {
                    Some(indices) => {
                        let mut unique_roots = indices.clone();
                        unique_roots.sort_unstable();
                        unique_roots.dedup();
                        let mask = ProjectionMask::roots(
                            reader_meta.metadata().file_metadata().schema_descr(),
                            unique_roots.iter().copied(),
                        );
                        (Some(unique_roots), mask)
                    }
                    None if has_provenance && !spindle_probe => (
                        None,
                        ProjectionMask::roots(
                            reader_meta.metadata().file_metadata().schema_descr(),
                            0..full_schema.fields().len(),
                        ),
                    ),
                    None => (None, ProjectionMask::all()),
                };

                let num_row_groups = reader_meta.metadata().num_row_groups();
                for rg_idx in 0..num_row_groups {
                    if remaining_limit == Some(0) || raw_tx.is_closed() {
                        break;
                    }
                    let rg = reader_meta.metadata().row_group(rg_idx);
                    let rg_rows = usize::try_from(rg.num_rows()).map_err(|_| {
                        DataFusionError::Execution(format!(
                            "row group {rg_idx} invalid num_rows {}",
                            rg.num_rows()
                        ))
                    })?;
                    if rg_rows == 0 {
                        continue;
                    }
                    let mut rg_rows_remaining =
                        remaining_limit.map_or(rg_rows, |rem| rg_rows.min(rem));
                    let (_, _, pre_rg_budget) = estimate_row_group_decode_plan(
                        rg,
                        reader_meta.schema(),
                        data_section_end,
                        &mask,
                        remaining_limit,
                        projected_schema.fields().len(),
                        None,
                    )?;
                    decode_res.try_resize(pre_rg_budget)?;
                    let cur_len = file
                        .metadata()
                        .map_err(|e| DataFusionError::Execution(e.to_string()))?
                        .len();
                    if cur_len != file_len {
                        return Err(DataFusionError::Execution(format!(
                            "parquet file {} changed length concurrently during query ({cur_len} != {file_len})",
                            pinned.display_path().display()
                        )));
                    }

                    let mut rg_buffers = PushBuffers::new(file_len);
                    let mut col_scans = vec![None; rg.num_columns()];
                    for (col_idx, slot) in col_scans.iter_mut().enumerate() {
                        if !mask.leaf_included(col_idx) {
                            continue;
                        }
                        let col = rg.column(col_idx);
                        let (col_start, col_len) =
                            checked_column_byte_range(col, col_idx, data_section_end)
                                .map_err(DataFusionError::Execution)?;
                        let col_len_usize = usize::try_from(col_len).map_err(|_| {
                            DataFusionError::Execution(format!(
                                "column {col_idx} compressed length overflows usize"
                            ))
                        })?;
                        let col_bytes =
                            <std::fs::File as ChunkReader>::get_bytes(&file, col_start, col_len_usize)
                                .map_err(|e| DataFusionError::Execution(e.to_string()))?;
                        if col_bytes.len() != col_len_usize {
                            return Err(DataFusionError::Execution(format!(
                                "short read buffering parquet column {col_idx} in row group {rg_idx}"
                            )));
                        }
                        let scan = scan_column_chunk_page_headers(
                            &mut std::io::Cursor::new(&col_bytes),
                            0,
                            col_len,
                            col.column_type(),
                            col.column_descr().type_length(),
                            col.column_descr().max_def_level(),
                            rg_rows,
                        )
                        .map_err(DataFusionError::Execution)?;
                        rg_buffers.push_range(col_start..(col_start + col_len), col_bytes);
                        *slot = Some(scan);
                    }
                    let (batch_rows, rg_resident_bytes, rg_decode_budget) =
                        estimate_row_group_decode_plan(
                            rg,
                            reader_meta.schema(),
                            data_section_end,
                            &mask,
                            remaining_limit,
                            projected_schema.fields().len(),
                            Some(&col_scans),
                        )?;
                    decode_res.try_resize(rg_decode_budget)?;
                    let rg_arc = Arc::new(rg_buffers);
                    validate_projected_row_group_pages(&rg_arc, &reader_meta, rg_idx, &mask)
                        .map_err(DataFusionError::Execution)?;
                    let immutable_rg = Arc::try_unwrap(rg_arc).unwrap_or_else(|a| (*a).clone());
                    let mut pb = ParquetRecordBatchReaderBuilder::new_with_metadata(
                        immutable_rg,
                        reader_meta.clone(),
                    )
                    .with_row_groups(vec![rg_idx])
                    .with_projection(mask.clone())
                    .with_batch_size(batch_rows);
                    if let Some(rem) = remaining_limit {
                        pb = pb.with_limit(rem);
                    }
                    let mut reader = pb
                        .build()
                        .map_err(|e| DataFusionError::Execution(e.to_string()))?;
                    loop {
                        if rg_rows_remaining == 0
                            || remaining_limit == Some(0)
                            || raw_tx.is_closed()
                        {
                            break;
                        }
                        decode_res.try_resize(rg_decode_budget)?;
                        let Some(batch_res) = reader.next() else {
                            break;
                        };
                        let raw =
                            batch_res.map_err(|e| DataFusionError::Execution(e.to_string()))?;
                        let raw_bytes = raw.get_array_memory_size();
                        let extra_cast_bytes = match (projection.as_ref(), unique_roots.as_ref()) {
                            (Some(indices), Some(roots)) => estimate_projection_cast_bytes(
                                &raw,
                                roots,
                                indices,
                                &projected_schema,
                            )?,
                            _ => estimate_normalize_cast_bytes(&raw, &projected_schema)?,
                        };
                        decode_res.try_resize(
                            rg_resident_bytes
                                .saturating_add(raw_bytes)
                                .saturating_add(extra_cast_bytes),
                        )?;
                        let out_batch = match (projection.as_ref(), unique_roots.as_ref()) {
                            (Some(indices), Some(roots)) => {
                                build_projected_batch(raw, roots, indices, &projected_schema)?
                            }
                            _ => normalize_batch_schema(raw, &projected_schema)?,
                        };
                        let produced_rows = out_batch.num_rows();
                        rg_rows_remaining = rg_rows_remaining.saturating_sub(produced_rows);
                        if let Some(rem) = remaining_limit.as_mut() {
                            *rem = rem.saturating_sub(produced_rows);
                        }
                        let out_bytes = out_batch.get_array_memory_size();
                        decode_res.try_resize(rg_resident_bytes.saturating_add(out_bytes))?;
                        let batch_res = decode_res.split(out_bytes);
                        if raw_tx.blocking_send(Ok((out_batch, batch_res))).is_err() {
                            decode_res.free();
                            footer_res.free();
                            return Ok(());
                        }
                    }
                }
                drop(reader_meta);
                footer_res.free();
                decode_res.free();
            }
            decode_res.free();
            footer_res.free();
            Ok(())
        });
        Ok(builder.build())
    }
}

fn discover_bounded_snapshot(
    olap: &OlapParquetStore,
    domain: &str,
    since_ms: Option<i64>,
    snapshot_budget: &Arc<SnapshotBudgetGuard>,
) -> Result<Vec<PinnedParquetFile>, String> {
    snapshot_budget.try_resize(MAX_SNAPSHOT_METADATA_BYTES)?;
    match olap.snapshot_pinned_parquet_files_bounded(
        domain,
        since_ms,
        MAX_SNAPSHOT_PARQUET_FILES,
        MAX_SNAPSHOT_METADATA_BYTES,
    ) {
        Ok(files) => Ok(files),
        Err(err) => {
            snapshot_budget.free();
            Err(err)
        }
    }
}

#[cfg(test)]
fn register_pinned_parquet_table(
    ctx: &SessionContext,
    table_name: &str,
    schema: SchemaRef,
    files: Vec<PinnedParquetFile>,
) -> Result<(), String> {
    register_pinned_parquet_table_with_budget(ctx, table_name, schema, files, None)
}

fn register_pinned_parquet_table_with_budget(
    ctx: &SessionContext,
    table_name: &str,
    schema: SchemaRef,
    files: Vec<PinnedParquetFile>,
    snapshot_budget: Option<&Arc<SnapshotBudgetGuard>>,
) -> Result<(), String> {
    let partitions = partition_pinned_files(files, 2, snapshot_budget)?;
    let table = PinnedParquetTable {
        schema,
        partitions,
        snapshot_budget: snapshot_budget.map(Arc::clone),
    };
    ctx.register_table(table_name, Arc::new(table))
        .map_err(|e| e.to_string())?;
    Ok(())
}

type PinnedParquetPartitions = Arc<[Arc<[PinnedParquetFile]>]>;

fn partition_pinned_files(
    files: Vec<PinnedParquetFile>,
    target_partitions: usize,
    snapshot_budget: Option<&Arc<SnapshotBudgetGuard>>,
) -> Result<PinnedParquetPartitions, String> {
    let mid = files.len().div_ceil(target_partitions).max(1);
    let num_chunks = files.len().div_ceil(mid);
    if let Some(budget) = snapshot_budget {
        let retained_files_bytes = snapshot_retained_heap_bytes(&files);
        let conversion_peak_bytes = retained_files_bytes
            .saturating_add(
                files
                    .capacity()
                    .saturating_mul(std::mem::size_of::<PinnedParquetFile>()),
            )
            .saturating_add(
                files
                    .len()
                    .saturating_mul(std::mem::size_of::<PinnedParquetFile>()),
            )
            .saturating_add(
                num_chunks.saturating_mul(std::mem::size_of::<Arc<[PinnedParquetFile]>>() + 16),
            )
            .saturating_add(16);
        if let Err(err) = budget.try_resize(conversion_peak_bytes.max(MAX_SNAPSHOT_METADATA_BYTES))
        {
            budget.free();
            return Err(err);
        }
    }
    let mut partitions_vec = Vec::with_capacity(num_chunks);
    let mut iter = files.into_iter();
    while iter.len() > 0 {
        let chunk: Arc<[PinnedParquetFile]> = iter.by_ref().take(mid).collect();
        partitions_vec.push(chunk);
    }
    let partitions: Arc<[Arc<[PinnedParquetFile]>]> = Arc::from(partitions_vec);
    drop(iter);
    if let Some(budget) = snapshot_budget {
        let retained_bytes = snapshot_partitions_retained_heap_bytes(&partitions);
        if let Err(err) = budget.try_resize(retained_bytes) {
            budget.free();
            return Err(err);
        }
    }
    Ok(partitions)
}

fn register_parquet_or_empty(
    ctx: &SessionContext,
    olap: &OlapParquetStore,
    domain: &str,
    since_ms: Option<i64>,
    schema: SchemaRef,
    snapshot_budget: &Arc<SnapshotBudgetGuard>,
) -> Result<(), String> {
    let files = discover_bounded_snapshot(olap, domain, since_ms, snapshot_budget)?;
    if files.is_empty() {
        snapshot_budget.free();
        ctx.register_batch("t", RecordBatch::new_empty(schema))
            .map_err(|e| e.to_string())?;
    } else {
        register_pinned_parquet_table_with_budget(ctx, "t", schema, files, Some(snapshot_budget))?;
    }
    Ok(())
}

async fn register_parquet_with_optional_live(
    ctx: &SessionContext,
    olap: &OlapParquetStore,
    domain: &str,
    since_ms: Option<i64>,
    schema: SchemaRef,
    live_batch: Option<RecordBatch>,
    snapshot_budget: &Arc<SnapshotBudgetGuard>,
) -> Result<(), String> {
    let files = discover_bounded_snapshot(olap, domain, since_ms, snapshot_budget)?;
    match (files.is_empty(), live_batch) {
        (true, None) => {
            snapshot_budget.free();
            ctx.register_batch("t", RecordBatch::new_empty(schema))
                .map_err(|e| e.to_string())?;
        }
        (true, Some(batch)) => {
            snapshot_budget.free();
            ctx.register_batch("t", batch).map_err(|e| e.to_string())?;
        }
        (false, None) => {
            register_pinned_parquet_table_with_budget(
                ctx,
                "t",
                schema,
                files,
                Some(snapshot_budget),
            )?;
        }
        (false, Some(batch)) => {
            register_pinned_parquet_table_with_budget(
                ctx,
                "t_pq",
                schema,
                files,
                Some(snapshot_budget),
            )?;
            ctx.register_batch("t_live", batch)
                .map_err(|e| e.to_string())?;
            let union_df = ctx
                .sql("SELECT * FROM t_pq UNION SELECT * FROM t_live")
                .await
                .map_err(|e| e.to_string())?;
            ctx.register_table("t", union_df.into_view())
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

async fn register_metrics_table(
    ctx: &SessionContext,
    olap: &OlapParquetStore,
    since_ms: Option<i64>,
    live_metrics: &[MetricPoint],
    live_input_res: &mut MemoryReservation,
    snapshot_budget: &Arc<SnapshotBudgetGuard>,
) -> Result<(), String> {
    let live_batch = if live_metrics.is_empty() {
        None
    } else {
        let peak_bytes = estimate_metrics_batch_peak_bytes(live_metrics);
        live_input_res
            .try_resize(peak_bytes)
            .map_err(|e| e.to_string())?;
        let batch = metrics_to_record_batch(live_metrics)?;
        live_input_res
            .try_resize(retained_record_batch_bytes(&batch))
            .map_err(|e| e.to_string())?;
        Some(batch)
    };
    register_parquet_with_optional_live(
        ctx,
        olap,
        "metrics",
        since_ms,
        METRICS_SCHEMA.clone(),
        live_batch,
        snapshot_budget,
    )
    .await
}

async fn register_fs_table(
    ctx: &SessionContext,
    olap: &OlapParquetStore,
    since_ms: Option<i64>,
    live_snapshot: Option<&HostSnapshot>,
    live_input_res: &mut MemoryReservation,
    snapshot_budget: &Arc<SnapshotBudgetGuard>,
) -> Result<(), String> {
    let live_batch = match live_snapshot.filter(|s| !s.fs_churn.is_empty()) {
        Some(s) => {
            let peak_bytes = estimate_fs_churn_batch_peak_bytes(&s.fs_churn);
            live_input_res
                .try_resize(peak_bytes)
                .map_err(|e| e.to_string())?;
            let batch = fs_churn_to_record_batch(s.timestamp_unix_ms, &s.fs_churn)?;
            live_input_res
                .try_resize(retained_record_batch_bytes(&batch))
                .map_err(|e| e.to_string())?;
            Some(batch)
        }
        None => None,
    };
    register_parquet_with_optional_live(
        ctx,
        olap,
        "fs_events",
        since_ms,
        FS_EVENTS_SCHEMA.clone(),
        live_batch,
        snapshot_budget,
    )
    .await
}

#[cfg(test)]
fn batches_to_json_rows(batches: &[RecordBatch]) -> Result<Vec<serde_json::Value>, String> {
    batches_to_json_rows_bounded(batches, MAX_UNIFIED_QUERY_RESULT_BYTES)
}

fn batches_to_json_rows_bounded(
    batches: &[RecordBatch],
    max_bytes: usize,
) -> Result<Vec<serde_json::Value>, String> {
    let mut used_bytes = 0_usize;
    let mut total_rows = 0_usize;
    for batch in batches {
        let arrow_bytes = batch.get_array_memory_size();
        let batch_overhead = if batch.num_rows() == 0 {
            0
        } else {
            256_usize.saturating_add(batch.num_columns().saturating_mul(128))
        };
        used_bytes = used_bytes
            .checked_add(arrow_bytes)
            .and_then(|u| u.checked_add(batch_overhead))
            .filter(|&u| u <= max_bytes)
            .ok_or_else(|| {
                format!("unified query result exceeds maximum retained budget ({max_bytes} bytes)")
            })?;
        total_rows = total_rows.checked_add(batch.num_rows()).ok_or_else(|| {
            format!("unified query result row count overflows ({max_bytes} bytes)")
        })?;
    }
    let vec_header = total_rows.saturating_mul(std::mem::size_of::<serde_json::Value>());
    used_bytes = used_bytes
        .checked_add(vec_header)
        .filter(|&u| u <= max_bytes)
        .ok_or_else(|| {
            format!("unified query result exceeds maximum retained budget ({max_bytes} bytes)")
        })?;

    let mut rows = Vec::with_capacity(total_rows);
    for batch in batches {
        let row_base = per_row_json_map_overhead(batch.schema().as_ref());
        for row_idx in 0..batch.num_rows() {
            used_bytes = used_bytes
                .checked_add(row_base)
                .filter(|&u| u <= max_bytes)
                .ok_or_else(|| {
                    format!(
                        "unified query result exceeds maximum retained budget ({max_bytes} bytes)"
                    )
                })?;
            let mut obj = serde_json::Map::with_capacity(batch.num_columns());
            for (field, array) in batch.schema().fields().iter().zip(batch.columns()) {
                let val = if array.is_null(row_idx) {
                    serde_json::Value::Null
                } else {
                    match array.data_type() {
                        DataType::Utf8 => {
                            let s = array
                                .as_any()
                                .downcast_ref::<StringArray>()
                                .ok_or("Utf8 downcast failed")?
                                .value(row_idx);
                            used_bytes = used_bytes
                                .checked_add(s.len())
                                .filter(|&u| u <= max_bytes)
                                .ok_or_else(|| {
                                    format!(
                                        "unified query result exceeds maximum retained budget ({max_bytes} bytes)"
                                    )
                                })?;
                            serde_json::Value::String(s.to_owned())
                        }
                        DataType::Utf8View => {
                            let s = array
                                .as_any()
                                .downcast_ref::<StringViewArray>()
                                .ok_or("Utf8View downcast failed")?
                                .value(row_idx);
                            used_bytes = used_bytes
                                .checked_add(s.len())
                                .filter(|&u| u <= max_bytes)
                                .ok_or_else(|| {
                                    format!(
                                        "unified query result exceeds maximum retained budget ({max_bytes} bytes)"
                                    )
                                })?;
                            serde_json::Value::String(s.to_owned())
                        }
                        DataType::Int64 => serde_json::json!(
                            array
                                .as_any()
                                .downcast_ref::<Int64Array>()
                                .ok_or("Int64 downcast failed")?
                                .value(row_idx)
                        ),
                        DataType::UInt64 => serde_json::json!(
                            array
                                .as_any()
                                .downcast_ref::<UInt64Array>()
                                .ok_or("UInt64 downcast failed")?
                                .value(row_idx)
                        ),
                        DataType::UInt32 => serde_json::json!(
                            array
                                .as_any()
                                .downcast_ref::<UInt32Array>()
                                .ok_or("UInt32 downcast failed")?
                                .value(row_idx)
                        ),
                        DataType::Float64 => serde_json::json!(
                            array
                                .as_any()
                                .downcast_ref::<Float64Array>()
                                .ok_or("Float64 downcast failed")?
                                .value(row_idx)
                        ),
                        DataType::Boolean => serde_json::Value::Bool(
                            array
                                .as_any()
                                .downcast_ref::<BooleanArray>()
                                .ok_or("Boolean downcast failed")?
                                .value(row_idx),
                        ),
                        other => return Err(format!("unsupported Arrow column type: {other}")),
                    }
                };
                obj.insert(field.name().clone(), val);
            }
            rows.push(serde_json::Value::Object(obj));
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{Field, Schema};
    use std::fs;
    use std::io::{Seek, SeekFrom};
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    fn non_tmp_dir() -> TempDir {
        if let Some(env_dir) = std::env::var_os("TMPDIR") {
            let p = std::path::PathBuf::from(env_dir);
            if !p.starts_with("/tmp")
                && !p.starts_with("/dev/shm")
                && fs::create_dir_all(&p).is_ok()
                && let Ok(td) = tempfile::tempdir_in(&p)
            {
                return td;
            }
        }
        if fs::create_dir_all("/data/tmp").is_ok()
            && let Ok(td) = tempfile::tempdir_in("/data/tmp")
        {
            return td;
        }
        tempfile::tempdir_in("/var/tmp").unwrap()
    }

    #[tokio::test]
    async fn spindle_receipt_probe_requires_exact_provenance_and_keeps_public_schema() {
        let dir = non_tmp_dir();
        let missing_store = dir.path().join("uninitialized");
        let layout = validated_storage_layout(&missing_store).unwrap();
        let key = SpindleProvenance {
            origin: "spindle-a".into(),
            record_id: 17,
            producer: "batch-client".into(),
            generation: "generation-a".into(),
            sequence: 3,
        };
        let timestamp_ms = 1_700_000_000_000;
        assert!(
            !probe_spindle_projection(&layout, QueryDomain::Metrics, timestamp_ms, &key)
                .await
                .unwrap()
        );
        assert!(!missing_store.exists());

        let catalog = fabric_store::CatalogStore::open(dir.path()).unwrap();
        let layout = catalog.layout();
        let olap = OlapParquetStore::new(layout.clone());
        let point = MetricPoint {
            timestamp_ms,
            name: "receipt.sensor".into(),
            value: 12.5,
            host: "host".into(),
            slice: "spindle".into(),
            unit: "test".into(),
            pid: None,
            labels_json: serde_json::to_string(&key).unwrap(),
        };
        olap.write_metrics(std::slice::from_ref(&point)).unwrap();
        assert!(
            !probe_spindle_projection(layout, QueryDomain::Metrics, timestamp_ms, &key)
                .await
                .unwrap()
        );
        let log_key = SpindleProvenance {
            record_id: 18,
            sequence: 4,
            ..key.clone()
        };
        let trace_key = SpindleProvenance {
            record_id: 19,
            sequence: 5,
            ..key.clone()
        };
        let mut flush = olap
            .plan_spindle_flush(
                &[(Some(key.clone()), point)],
                &[(
                    Some(log_key.clone()),
                    fabric_core::LogRecord {
                        timestamp_ms,
                        message: "test receipt log".into(),
                        ..Default::default()
                    },
                )],
                &[(
                    Some(trace_key.clone()),
                    fabric_core::TraceSpanRecord {
                        timestamp_ms,
                        span_id: "test receipt span".into(),
                        ..Default::default()
                    },
                )],
            )
            .unwrap();
        olap.execute_flush_plan(&mut flush).unwrap();
        for (domain, receipt) in [
            (QueryDomain::Metrics, &key),
            (QueryDomain::Logs, &log_key),
            (QueryDomain::Traces, &trace_key),
        ] {
            assert!(
                probe_spindle_projection(layout, domain, timestamp_ms, receipt)
                    .await
                    .unwrap()
            );
        }
        let mut other_origin = key.clone();
        other_origin.origin = "spindle-b".into();
        assert!(
            !probe_spindle_projection(layout, QueryDomain::Metrics, timestamp_ms, &other_origin)
                .await
                .unwrap()
        );
        assert!(
            !probe_spindle_projection(
                layout,
                QueryDomain::Metrics,
                timestamp_ms + 86_400_000,
                &key,
            )
            .await
            .unwrap()
        );
        assert!(
            probe_spindle_projection(layout, QueryDomain::Metrics, 0, &key)
                .await
                .unwrap()
        );
        let query = UnifiedQuerySpec {
            limit: 10,
            ..Default::default()
        };
        let output = execute_unified_query(layout.data_dir(), &query, &[], None)
            .await
            .unwrap();
        assert_eq!(output.row_count, 2);
        assert_eq!(output.schema.fields(), METRICS_SCHEMA.fields());
        assert!(output.rows.iter().all(|row| {
            row["name"] == "receipt.sensor" && row.get(SPINDLE_PROVENANCE_COLUMN).is_none()
        }));
    }

    #[tokio::test]
    async fn pinned_parquet_stream_ignores_post_snapshot_ancestor_symlink_and_rejects_fifo() {
        let dir = non_tmp_dir();
        let outside = non_tmp_dir();
        let catalog = fabric_store::CatalogStore::open(dir.path()).unwrap();
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());

        olap.write_metrics(&[MetricPoint {
            timestamp_ms: 1_700_000_000_000_i64,
            name: "pinned.metric".into(),
            value: 12.5,
            host: "h".into(),
            slice: "spindle".into(),
            unit: "svc".into(),
            pid: None,
            labels_json: "{}".into(),
        }])
        .unwrap();

        let pinned = olap.snapshot_pinned_parquet_files("metrics", None).unwrap();
        assert_eq!(pinned.len(), 1);

        let domain_dir = layout.olap_domain_dir("metrics");
        let moved = dir.path().join("metrics-moved");
        fs::rename(&domain_dir, &moved).unwrap();
        symlink(outside.path(), &domain_dir).unwrap();

        let ctx = SessionContext::new();
        register_pinned_parquet_table(&ctx, "t", METRICS_SCHEMA.clone(), pinned).unwrap();
        let batches = ctx
            .sql("SELECT name, value FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        let rows = batches_to_json_rows(&batches).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "pinned.metric");
        assert_eq!(rows[0]["value"], 12.5);

        let fifo_dir = non_tmp_dir();
        let fifo_catalog = fabric_store::CatalogStore::open(fifo_dir.path()).unwrap();
        let fifo_olap = OlapParquetStore::new(fifo_catalog.layout().clone());
        let pq_path = fifo_olap
            .write_metrics(&[MetricPoint {
                timestamp_ms: 1_700_000_000_000_i64,
                name: "fifo.metric".into(),
                value: 1.0,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: None,
                labels_json: "{}".into(),
            }])
            .unwrap()[0]
            .clone();
        let fifo_snap = fifo_olap
            .snapshot_pinned_parquet_files("metrics", None)
            .unwrap();
        assert_eq!(fifo_snap.len(), 1);

        fs::remove_file(&pq_path).unwrap();
        let status = std::process::Command::new("mkfifo")
            .arg(&pq_path)
            .status()
            .expect("spawn mkfifo");
        assert!(status.success(), "mkfifo succeeded");

        let fifo_ctx = SessionContext::new();
        register_pinned_parquet_table(&fifo_ctx, "t", METRICS_SCHEMA.clone(), fifo_snap).unwrap();
        let stream_err = fifo_ctx
            .sql("SELECT name, value FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("pinned stream must reject FIFO without blocking");
        assert!(
            stream_err.to_string().contains("non-regular"),
            "unexpected stream err: {stream_err}"
        );

        let query_err =
            execute_unified_query(fifo_dir.path(), &UnifiedQuerySpec::default(), &[], None)
                .await
                .expect_err("execute_unified_query must reject FIFO .parquet without blocking");
        assert!(
            query_err.contains("non-regular"),
            "unexpected query err: {query_err}"
        );
    }

    #[tokio::test]
    async fn pinned_parquet_exec_pushes_down_projection_and_enforces_memory_pool() {
        let dir = non_tmp_dir();
        let catalog = fabric_store::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let wide_labels = format!("{{\"blob\":\"{}\"}}", "x".repeat(96 * 1024));
        olap.write_metrics(&[MetricPoint {
            timestamp_ms: 1_700_000_000_000_i64,
            name: "wide.payload.metric".into(),
            value: 7.25,
            host: "h".into(),
            slice: "spindle".into(),
            unit: "svc".into(),
            pid: None,
            labels_json: wide_labels,
        }])
        .unwrap();

        let pinned = olap.snapshot_pinned_parquet_files("metrics", None).unwrap();
        assert_eq!(pinned.len(), 1);

        let env = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::new(GreedyMemoryPool::new(128 * 1024)))
            .build_arc()
            .unwrap();
        let ctx = SessionContext::new_with_config_rt(
            SessionConfig::new().with_target_partitions(1),
            Arc::clone(&env),
        );
        register_pinned_parquet_table(&ctx, "t", METRICS_SCHEMA.clone(), pinned).unwrap();

        let count_batches = ctx
            .sql("SELECT COUNT(*) AS cnt FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        let count_rows = batches_to_json_rows(&count_batches).unwrap();
        assert_eq!(count_rows.len(), 1);
        assert_eq!(count_rows[0]["cnt"], 1);

        let narrow_batches = ctx
            .sql("SELECT name, value FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        let narrow_rows = batches_to_json_rows(&narrow_batches).unwrap();
        assert_eq!(narrow_rows.len(), 1);
        assert_eq!(narrow_rows[0]["name"], "wide.payload.metric");
        assert_eq!(narrow_rows[0]["value"], 7.25);

        let oom_err = ctx
            .sql("SELECT labels_json FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("selecting wide labels_json must trip the 128 KiB GreedyMemoryPool");
        let err_text = oom_err.to_string();
        assert!(
            err_text.contains("Resources exhausted")
                || err_text.contains("allocate")
                || err_text.contains("memory"),
            "unexpected memory pool err: {err_text}"
        );

        // Corrupt the compressed column chunk bytes on disk while leaving the Parquet footer intact,
        // proving `GreedyMemoryPool` rejects the oversized column BEFORE reading/decompressing its pages.
        {
            use std::io::Write;
            let pq_files = olap.list_parquet_files("metrics", None).unwrap();
            let pq_path = &pq_files[0];
            let meta_reader =
                ParquetRecordBatchReaderBuilder::try_new(fs::File::open(pq_path).unwrap()).unwrap();
            let (col_start, col_len) = meta_reader.metadata().row_group(0).column(7).byte_range();
            drop(meta_reader);
            let mut f = fs::OpenOptions::new().write(true).open(pq_path).unwrap();
            f.seek(SeekFrom::Start(col_start)).unwrap();
            let corrupt = vec![0xFF_u8; usize::try_from(col_len).unwrap_or(16).min(16)];
            f.write_all(&corrupt).unwrap();
            f.sync_all().unwrap();
        }
        let corrupt_pinned = olap.snapshot_pinned_parquet_files("metrics", None).unwrap();
        let corrupt_ctx =
            SessionContext::new_with_config_rt(SessionConfig::new().with_target_partitions(1), env);
        register_pinned_parquet_table(&corrupt_ctx, "t", METRICS_SCHEMA.clone(), corrupt_pinned)
            .unwrap();
        let predecode_oom = corrupt_ctx
            .sql("SELECT labels_json FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("pre-decode pool check must reject before touching corrupted data pages");
        assert!(
            predecode_oom.to_string().contains("Resources exhausted"),
            "expected pre-decode memory rejection before ZSTD/page read, got: {predecode_oom}"
        );

        // Verify the final batch sitting in the capacity-1 output channel keeps its reservation
        // held in `GreedyMemoryPool` after `spawn_blocking` exits until drained by the consumer.
        let ch_dir = non_tmp_dir();
        let ch_catalog = fabric_store::CatalogStore::open(ch_dir.path()).unwrap();
        let ch_olap = OlapParquetStore::new(ch_catalog.layout().clone());
        let mid_labels = format!("{{\"blob\":\"{}\"}}", "y".repeat(48 * 1024));
        ch_olap
            .write_metrics(&[MetricPoint {
                timestamp_ms: 1_700_000_000_000_i64,
                name: "channel.hold.metric".into(),
                value: 3.5,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: None,
                labels_json: mid_labels,
            }])
            .unwrap();
        let ch_pinned = ch_olap
            .snapshot_pinned_parquet_files("metrics", None)
            .unwrap();
        let pool: Arc<dyn datafusion::execution::memory_pool::MemoryPool> =
            Arc::new(GreedyMemoryPool::new(512 * 1024));
        let ch_env = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::clone(&pool))
            .build_arc()
            .unwrap();
        let ch_ctx = SessionContext::new_with_config_rt(
            SessionConfig::new().with_target_partitions(1),
            ch_env,
        );
        let exec = PinnedParquetExec::try_new(
            METRICS_SCHEMA.clone(),
            Arc::from([Arc::from(ch_pinned)]),
            None,
            None,
        )
        .unwrap();
        let stream = exec.execute(0, ch_ctx.task_ctx()).unwrap();
        for _ in 0..80 {
            let cur = pool.reserved();
            if (48 * 1024..80 * 1024).contains(&cur) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            pool.reserved() >= 48 * 1024,
            "final queued batch must remain reserved in channel before consumer drain (reserved={})",
            pool.reserved()
        );
        let collected = datafusion::physical_plan::common::collect(stream)
            .await
            .unwrap();
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].num_rows(), 1);
        assert_eq!(
            pool.reserved(),
            0,
            "all reservations must release once the final batch is drained"
        );

        // Dictionary-cast estimation and dictionary-repeated pre-decode OOM guard without stats.
        {
            use arrow::array::{DictionaryArray, Int32Array};
            use arrow::datatypes::Int32Type;
            use parquet::arrow::ArrowWriter;
            use parquet::file::properties::{EnabledStatistics, WriterProperties};
            use std::io::Write;

            let dict_keys = Int32Array::from(vec![0; 64]);
            let dict_vals = StringArray::from(vec!["d".repeat(16 * 1024)]);
            let dict_arr: ArrayRef = Arc::new(
                DictionaryArray::<Int32Type>::try_new(dict_keys, Arc::new(dict_vals)).unwrap(),
            );
            assert!(
                estimate_array_cast_bytes(&dict_arr, &DataType::Utf8) >= 64 * 16 * 1024,
                "dictionary cast estimate must account for per-row expanded dictionary values"
            );

            let dict_dir = non_tmp_dir();
            let dict_catalog = fabric_store::CatalogStore::open(dict_dir.path()).unwrap();
            let dict_olap = OlapParquetStore::new(dict_catalog.layout().clone());
            let repeated_labels = "k".repeat(96 * 1024);
            let pts: Vec<MetricPoint> = (0..64_i64)
                .map(|i| MetricPoint {
                    timestamp_ms: 1_700_000_000_000_i64 + i,
                    name: "wide.dict.metric".into(),
                    value: 1.5,
                    host: "h".into(),
                    slice: "spindle".into(),
                    unit: "svc".into(),
                    pid: None,
                    labels_json: repeated_labels.clone(),
                })
                .collect();
            let batch = metrics_to_record_batch(&pts).unwrap();
            let part_dir = dict_catalog
                .layout()
                .olap_domain_dir("metrics")
                .join("dt=2023-11-14");
            fs::create_dir_all(&part_dir).unwrap();
            let pq_path = part_dir.join("part-dict-nostats.parquet");
            let props = WriterProperties::builder()
                .set_statistics_enabled(EnabledStatistics::None)
                .set_dictionary_enabled(true)
                .build();
            let pq_file = fs::File::create(&pq_path).unwrap();
            let mut writer = ArrowWriter::try_new(pq_file, batch.schema(), Some(props)).unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();

            let meta_reader =
                ParquetRecordBatchReaderBuilder::try_new(fs::File::open(&pq_path).unwrap())
                    .unwrap();
            let (col_start, col_len) = meta_reader.metadata().row_group(0).column(7).byte_range();
            drop(meta_reader);
            let mut f = fs::OpenOptions::new().write(true).open(&pq_path).unwrap();
            f.seek(SeekFrom::Start(col_start)).unwrap();
            let corrupt = vec![0xFF_u8; usize::try_from(col_len).unwrap_or(16).min(16)];
            f.write_all(&corrupt).unwrap();
            f.sync_all().unwrap();

            let dict_pinned = dict_olap
                .snapshot_pinned_parquet_files("metrics", None)
                .unwrap();
            assert_eq!(dict_pinned.len(), 1);
            let dict_env = RuntimeEnvBuilder::new()
                .with_memory_pool(Arc::new(GreedyMemoryPool::new(128 * 1024)))
                .build_arc()
                .unwrap();
            let dict_ctx = SessionContext::new_with_config_rt(
                SessionConfig::new().with_target_partitions(1),
                dict_env,
            );
            register_pinned_parquet_table(&dict_ctx, "t", METRICS_SCHEMA.clone(), dict_pinned)
                .unwrap();

            let narrow = dict_ctx
                .sql("SELECT name, value FROM t")
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            assert_eq!(batches_to_json_rows(&narrow).unwrap().len(), 64);

            let dict_oom = dict_ctx
                .sql("SELECT labels_json FROM t")
                .await
                .unwrap()
                .collect()
                .await
                .expect_err(
                    "dictionary-repeated EnabledStatistics::None wide column must reject pre-decode",
                );
            assert!(
                dict_oom.to_string().contains("Resources exhausted"),
                "expected pre-decode memory rejection for dictionary column without stats, got: {dict_oom}"
            );

            // Deduplicated StringView / Utf8View cast estimation and pre-cast pool rejection.
            use arrow::array::builder::StringViewBuilder;
            let mut svb = StringViewBuilder::new().with_deduplicate_strings();
            let long_view = "v".repeat(16 * 1024);
            for _ in 0..64 {
                svb.append_value(&long_view);
            }
            let view_arr: ArrayRef = Arc::new(svb.finish());
            let view_est = estimate_array_cast_bytes(&view_arr, &DataType::Utf8);
            assert!(
                view_est >= 64 * 16 * 1024,
                "Utf8View cast estimate must cover full logical expansion of shared out-of-line slices"
            );
            let view_schema = Arc::new(arrow::datatypes::Schema::new(vec![
                arrow::datatypes::Field::new("labels_json", DataType::Utf8View, false),
            ]));
            let expected_utf8_schema = Arc::new(arrow::datatypes::Schema::new(vec![
                arrow::datatypes::Field::new("labels_json", DataType::Utf8, false),
            ]));
            let view_batch =
                RecordBatch::try_new(view_schema, vec![Arc::clone(&view_arr)]).unwrap();
            let norm_est =
                estimate_normalize_cast_bytes(&view_batch, &expected_utf8_schema).unwrap();
            assert_eq!(norm_est, view_est);
            let view_pool: Arc<dyn datafusion::execution::memory_pool::MemoryPool> =
                Arc::new(GreedyMemoryPool::new(512 * 1024));
            let view_res = MemoryConsumer::new("ViewCastCheck").register(&view_pool);
            assert!(
                view_res.try_resize(norm_est).is_err(),
                "512 KiB pool must reject ~1 MiB Utf8View -> Utf8 cast before normalizing"
            );
            let normalized_view =
                normalize_batch_schema(view_batch, &expected_utf8_schema).unwrap();
            assert_eq!(normalized_view.num_rows(), 64);

            // Repeated/nested Parquet column (same root width 8, List column inside) must reject
            // before row-group page decoding.
            use arrow::array::{Int64Builder, ListBuilder};
            let list_schema = Arc::new(Schema::new(vec![
                Field::new("timestamp_ms", DataType::Int64, false),
                Field::new("name", DataType::Utf8, false),
                Field::new("value", DataType::Float64, false),
                Field::new("host", DataType::Utf8, false),
                Field::new("slice", DataType::Utf8, false),
                Field::new("unit", DataType::Utf8, false),
                Field::new("pid", DataType::UInt32, true),
                Field::new(
                    "labels_json",
                    DataType::List(Arc::new(Field::new("item", DataType::Int64, true))),
                    false,
                ),
            ]));
            let mut lb = ListBuilder::new(Int64Builder::new());
            for _ in 0..1000 {
                lb.values().append_value(42);
            }
            lb.append(true);
            let nested_batch = RecordBatch::try_new(
                Arc::clone(&list_schema),
                vec![
                    Arc::new(Int64Array::from(vec![1_700_000_000_000_i64])),
                    Arc::new(StringArray::from(vec!["nested.metric"])),
                    Arc::new(Float64Array::from(vec![1.0_f64])),
                    Arc::new(StringArray::from(vec!["h"])),
                    Arc::new(StringArray::from(vec!["spindle"])),
                    Arc::new(StringArray::from(vec!["svc"])),
                    Arc::new(UInt32Array::from(vec![None])),
                    Arc::new(lb.finish()),
                ],
            )
            .unwrap();
            let nested_dir = non_tmp_dir();
            let nested_catalog = fabric_store::CatalogStore::open(nested_dir.path()).unwrap();
            let nested_olap = OlapParquetStore::new(nested_catalog.layout().clone());
            let nested_part = nested_catalog
                .layout()
                .olap_domain_dir("metrics")
                .join("dt=2023-11-14");
            fs::create_dir_all(&nested_part).unwrap();
            let nested_pq = nested_part.join("part-nested-list.parquet");
            let mut nested_writer = ArrowWriter::try_new(
                fs::File::create(&nested_pq).unwrap(),
                nested_batch.schema(),
                None,
            )
            .unwrap();
            nested_writer.write(&nested_batch).unwrap();
            nested_writer.close().unwrap();

            let nested_pinned = nested_olap
                .snapshot_pinned_parquet_files("metrics", None)
                .unwrap();
            let nested_env = RuntimeEnvBuilder::new()
                .with_memory_pool(Arc::new(GreedyMemoryPool::new(512 * 1024)))
                .build_arc()
                .unwrap();
            let nested_ctx = SessionContext::new_with_config_rt(
                SessionConfig::new().with_target_partitions(1),
                nested_env,
            );
            register_pinned_parquet_table(&nested_ctx, "t", METRICS_SCHEMA.clone(), nested_pinned)
                .unwrap();
            let nested_err = nested_ctx
                .sql("SELECT name, labels_json FROM t")
                .await
                .unwrap()
                .collect()
                .await
                .expect_err("nested/repeated parquet schema must reject before row-group decode");
            assert!(
                nested_err.to_string().contains("repeated parquet")
                    || nested_err.to_string().contains("incompatible parquet")
                    || nested_err.to_string().contains("flat schema"),
                "unexpected nested schema err: {nested_err}"
            );
        }
    }

    fn rewrite_parquet_footer_with_meta(
        pq_path: &Path,
        mut transform: impl FnMut(
            parquet::file::metadata::ParquetMetaData,
        ) -> parquet::file::metadata::ParquetMetaData,
    ) {
        use parquet::file::metadata::ParquetMetaDataWriter;
        let orig = fs::read(pq_path).unwrap();
        let len = orig.len();
        let footer_len = u32::from_le_bytes(orig[len - 8..len - 4].try_into().unwrap()) as usize;
        let footer_start = len - 8 - footer_len;
        let meta = ParquetMetaDataReader::decode_metadata(&orig[footer_start..len - 8]).unwrap();
        let updated = transform(meta);
        let mut new_footer_and_tail = Vec::new();
        ParquetMetaDataWriter::new(&mut new_footer_and_tail, &updated)
            .finish()
            .unwrap();
        let mut out = orig[..footer_start].to_vec();
        out.extend_from_slice(&new_footer_and_tail);
        fs::write(pq_path, out).unwrap();
    }

    #[tokio::test]
    async fn malformed_thrift_list_footer_and_retained_arrow_schema_memory_are_bounded() {
        use parquet::arrow::ArrowWriter;
        use parquet::file::properties::WriterProperties;
        use std::collections::HashMap;

        // 1. Tiny malformed Thrift footer (< 64B) declaring 100M SchemaElements must reject cleanly
        // without pre-allocating inside ParquetMetaDataReader.
        let bad_dir = non_tmp_dir();
        let bad_catalog = fabric_store::CatalogStore::open(bad_dir.path()).unwrap();
        let bad_olap = OlapParquetStore::new(bad_catalog.layout().clone());
        let bad_part = bad_catalog
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        fs::create_dir_all(&bad_part).unwrap();
        let bad_pq = bad_part.join("part-thrift-list-bomb.parquet");
        // Struct { field 2 = List<Struct> length 100_000_000 (varint 0x80 0xc2 0xd7 0x2f), Stop }
        let bomb_footer = [0x29_u8, 0xfc, 0x80, 0xc2, 0xd7, 0x2f, 0x00];
        let mut bomb_bytes = b"PAR1".to_vec();
        bomb_bytes.extend_from_slice(&bomb_footer);
        bomb_bytes.extend_from_slice(&(u32::try_from(bomb_footer.len()).unwrap()).to_le_bytes());
        bomb_bytes.extend_from_slice(b"PAR1");
        fs::write(&bad_pq, bomb_bytes).unwrap();
        let bad_pinned = bad_olap
            .snapshot_pinned_parquet_files("metrics", None)
            .unwrap();
        let bad_ctx = SessionContext::new();
        register_pinned_parquet_table(&bad_ctx, "t", METRICS_SCHEMA.clone(), bad_pinned).unwrap();
        let bomb_err = bad_ctx
            .sql("SELECT name FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("malformed Thrift list count exceeding footer bytes must reject");
        assert!(
            bomb_err.to_string().contains("Thrift") || bomb_err.to_string().contains("footer"),
            "unexpected bomb err: {bomb_err}"
        );

        // 2. Large custom Schema metadata retained by ArrowReaderMetadata must remain accounted
        // in footer_res after load.
        let meta_dir = non_tmp_dir();
        let meta_catalog = fabric_store::CatalogStore::open(meta_dir.path()).unwrap();
        let meta_olap = OlapParquetStore::new(meta_catalog.layout().clone());
        let meta_part = meta_catalog
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        fs::create_dir_all(&meta_part).unwrap();
        let meta_pq = meta_part.join("part-large-arrow-meta.parquet");
        let pts: Vec<MetricPoint> = (0..4_i64)
            .map(|i| MetricPoint {
                timestamp_ms: 1_700_000_000_000_i64 + i,
                name: format!("meta.{i}"),
                value: i as f64,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: None,
                labels_json: "{}".into(),
            })
            .collect();
        let base_batch = metrics_to_record_batch(&pts).unwrap();
        let mut custom_meta = HashMap::new();
        custom_meta.insert("big_meta".to_owned(), "m".repeat(160 * 1024));
        let schema_with_meta = Arc::new(Schema::new_with_metadata(
            METRICS_SCHEMA.fields().clone(),
            custom_meta,
        ));
        let meta_batch =
            RecordBatch::try_new(Arc::clone(&schema_with_meta), base_batch.columns().to_vec())
                .unwrap();
        let props = WriterProperties::builder()
            .set_max_row_group_row_count(Some(1))
            .build();
        let mut writer = ArrowWriter::try_new(
            fs::File::create(&meta_pq).unwrap(),
            schema_with_meta,
            Some(props),
        )
        .unwrap();
        for i in 0..4 {
            writer.write(&meta_batch.slice(i, 1)).unwrap();
        }
        writer.close().unwrap();

        let meta_pinned = meta_olap
            .snapshot_pinned_parquet_files("metrics", None)
            .unwrap();

        // First verify a 128 KiB pool rejects the large Arrow schema metadata cleanly.
        let small_env = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::new(GreedyMemoryPool::new(128 * 1024)))
            .build_arc()
            .unwrap();
        let small_ctx = SessionContext::new_with_config_rt(
            SessionConfig::new().with_target_partitions(1),
            small_env,
        );
        register_pinned_parquet_table(&small_ctx, "t", METRICS_SCHEMA.clone(), meta_pinned.clone())
            .unwrap();
        let small_err = small_ctx
            .sql("SELECT name FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("128 KiB pool must reject large retained ARROW:schema metadata");
        assert!(
            small_err.to_string().contains("Resources exhausted"),
            "unexpected small-pool schema metadata err: {small_err}"
        );

        // Verify an 832 KiB pool also rejects the ~952 KiB transient try_new peak
        // (ParquetMetaData + KV clone + decoded IPC base64 + dual Arrow Schema peak) pre-try_new.
        let transient_env = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::new(GreedyMemoryPool::new(832 * 1024)))
            .build_arc()
            .unwrap();
        let transient_ctx = SessionContext::new_with_config_rt(
            SessionConfig::new().with_target_partitions(1),
            transient_env,
        );
        register_pinned_parquet_table(
            &transient_ctx,
            "t",
            METRICS_SCHEMA.clone(),
            meta_pinned.clone(),
        )
        .unwrap();
        let transient_err = transient_ctx
            .sql("SELECT name FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("832 KiB pool must reject ~952 KiB pre-try_new dual IPC + KV clone peak");
        assert!(
            transient_err.to_string().contains("Resources exhausted"),
            "unexpected transient-peak schema metadata err: {transient_err}"
        );

        // Next verify against an 1152 KiB pool (accommodating the ~952 KiB try_new peak) that
        // while the stream is open and holding ArrowReaderMetadata, footer_res retains both
        // ParquetMetaData and Arrow Schema bytes and releases transient buffers after try_new.
        let pool: Arc<dyn datafusion::execution::memory_pool::MemoryPool> =
            Arc::new(GreedyMemoryPool::new(1152 * 1024));
        let hold_env = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::clone(&pool))
            .build_arc()
            .unwrap();
        let hold_ctx = SessionContext::new_with_config_rt(
            SessionConfig::new().with_target_partitions(1),
            hold_env,
        );
        let exec = PinnedParquetExec::try_new(
            METRICS_SCHEMA.clone(),
            Arc::from([Arc::from(meta_pinned)]),
            Some(vec![1]),
            None,
        )
        .unwrap();
        let stream = exec.execute(0, hold_ctx.task_ctx()).unwrap();
        for _ in 0..80 {
            if pool.reserved() >= 350 * 1024 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            pool.reserved() >= 350 * 1024,
            "footer_res must retain Arrow schema metadata alongside ParquetMetaData (reserved={})",
            pool.reserved()
        );
        let concurrent = MemoryConsumer::new("ConcurrentConsumer").register(&pool);
        assert!(
            concurrent.try_resize(850 * 1024).is_err(),
            "retained Arrow schema reservation must block an extra 850 KiB allocation in 1152 KiB pool"
        );
        drop(concurrent);
        let drained = datafusion::physical_plan::common::collect(stream)
            .await
            .unwrap();
        let total_rows: usize = drained.iter().map(RecordBatch::num_rows).sum();
        assert_eq!(total_rows, 4);
        assert_eq!(pool.reserved(), 0);
    }

    #[tokio::test]
    async fn liar_footer_page_header_and_metadata_invariants_reject_pre_decode() {
        use parquet::arrow::ArrowWriter;
        use parquet::basic::{Compression, ZstdLevel};
        use parquet::file::metadata::{FileMetaData, ParquetMetaDataBuilder};
        use parquet::file::properties::{EnabledStatistics, WriterProperties};

        // 1. Craft a valid ZSTD Parquet file whose page header advertises ~2 MiB uncompressed,
        // then tamper only the footer to claim total_uncompressed_size = 128 bytes.
        let dir = non_tmp_dir();
        let catalog = fabric_store::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        fs::create_dir_all(&part_dir).unwrap();
        let pq_path = part_dir.join("part-liar-page.parquet");
        let huge_compressible = "a".repeat(2 * 1024 * 1024);
        let batch = metrics_to_record_batch(&[MetricPoint {
            timestamp_ms: 1_700_000_000_000_i64,
            name: "liar.page".into(),
            value: 1.0,
            host: "h".into(),
            slice: "spindle".into(),
            unit: "svc".into(),
            pid: None,
            labels_json: huge_compressible,
        }])
        .unwrap();
        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(3).unwrap()))
            .set_dictionary_enabled(false)
            .set_statistics_enabled(EnabledStatistics::None)
            .build();
        let mut writer = ArrowWriter::try_new(
            fs::File::create(&pq_path).unwrap(),
            batch.schema(),
            Some(props),
        )
        .unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();

        rewrite_parquet_footer_with_meta(&pq_path, |meta| {
            let mut b = meta.into_builder();
            let mut rgs = b.take_row_groups();
            let mut rg_b = rgs.remove(0).into_builder();
            let mut cols = rg_b.take_columns();
            let col7 = cols.remove(7);
            let tampered_col7 = col7
                .into_builder()
                .set_total_uncompressed_size(128)
                .set_unencoded_byte_array_data_bytes(None)
                .build()
                .unwrap();
            cols.push(tampered_col7);
            rg_b = rg_b.set_column_metadata(cols);
            b = b.add_row_group(rg_b.build().unwrap());
            b.build()
        });

        let pinned = olap.snapshot_pinned_parquet_files("metrics", None).unwrap();
        let pool_env = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::new(GreedyMemoryPool::new(512 * 1024)))
            .build_arc()
            .unwrap();
        let ctx = SessionContext::new_with_config_rt(
            SessionConfig::new().with_target_partitions(1),
            pool_env,
        );
        register_pinned_parquet_table(&ctx, "t", METRICS_SCHEMA.clone(), pinned).unwrap();
        let page_oom = ctx
            .sql("SELECT labels_json FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err(
                "page-header uncompressed size > footer claim must reject before ZSTD decompress",
            );
        assert!(
            page_oom.to_string().contains("Resources exhausted"),
            "expected pre-decompression page-header Resources exhausted error, got: {page_oom}"
        );

        // 2. Swapped same-type columns (`host` and `unit`) must fail flat schema validation.
        let mut swapped_fields = METRICS_SCHEMA.fields().to_vec();
        swapped_fields.swap(3, 5);
        let swapped_schema = Arc::new(Schema::new(swapped_fields));
        let swapped_batch = metrics_to_record_batch(&[MetricPoint {
            timestamp_ms: 1_700_000_000_000_i64,
            name: "swapped".into(),
            value: 1.0,
            host: "h".into(),
            slice: "spindle".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        }])
        .unwrap();
        let mut swapped_cols = swapped_batch.columns().to_vec();
        swapped_cols.swap(3, 5);
        let swapped_rec = RecordBatch::try_new(Arc::clone(&swapped_schema), swapped_cols).unwrap();
        let swap_dir = non_tmp_dir();
        let swap_catalog = fabric_store::CatalogStore::open(swap_dir.path()).unwrap();
        let swap_olap = OlapParquetStore::new(swap_catalog.layout().clone());
        let swap_part = swap_catalog
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        fs::create_dir_all(&swap_part).unwrap();
        let swap_pq = swap_part.join("part-swapped-cols.parquet");
        let mut swap_writer =
            ArrowWriter::try_new(fs::File::create(&swap_pq).unwrap(), swapped_schema, None)
                .unwrap();
        swap_writer.write(&swapped_rec).unwrap();
        swap_writer.close().unwrap();
        let swap_pinned = swap_olap
            .snapshot_pinned_parquet_files("metrics", None)
            .unwrap();
        let swap_ctx = SessionContext::new();
        register_pinned_parquet_table(&swap_ctx, "t", METRICS_SCHEMA.clone(), swap_pinned).unwrap();
        let swap_err = swap_ctx
            .sql("SELECT host, unit FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("reordered same-type columns must be rejected");
        assert!(
            swap_err
                .to_string()
                .contains("does not match expected column"),
            "unexpected swapped column err: {swap_err}"
        );

        // 3. Mismatched file_metadata.num_rows on COUNT(*), negative rg.num_rows, and negative
        // data_page_offset must fail cleanly without clamping to 0 or panicking in byte_range().
        let inv_dir = non_tmp_dir();
        let inv_catalog = fabric_store::CatalogStore::open(inv_dir.path()).unwrap();
        let inv_olap = OlapParquetStore::new(inv_catalog.layout().clone());
        let inv_pq = inv_olap
            .write_metrics(&[MetricPoint {
                timestamp_ms: 1_700_000_000_000_i64,
                name: "inv.check".into(),
                value: 2.0,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: None,
                labels_json: "{}".into(),
            }])
            .unwrap()
            .pop()
            .unwrap();
        let clean_bytes = fs::read(&inv_pq).unwrap();

        // Case 3a: Mismatched file_metadata.num_rows = 999 while rg has 1 row.
        {
            let len = clean_bytes.len();
            let orig_footer_len =
                u32::from_le_bytes(clean_bytes[len - 8..len - 4].try_into().unwrap()) as usize;
            let footer_start = len - 8 - orig_footer_len;
            let mut new_footer = clean_bytes[footer_start..len - 8].to_vec();
            assert_eq!(new_footer.pop(), Some(0));
            // Field 3 (I64, full field_id 3 -> zig-zag 6), value 999 (zig-zag 1998 = 0xce 0x0f), Stop (0x00).
            new_footer.extend_from_slice(&[0x06, 0x06, 0xce, 0x0f, 0x00]);
            assert_eq!(
                ParquetMetaDataReader::decode_metadata(&new_footer)
                    .unwrap()
                    .file_metadata()
                    .num_rows(),
                999
            );
            let mut tampered = clean_bytes[..footer_start].to_vec();
            let new_len_u32 = u32::try_from(new_footer.len()).unwrap();
            tampered.extend_from_slice(&new_footer);
            tampered.extend_from_slice(&new_len_u32.to_le_bytes());
            tampered.extend_from_slice(b"PAR1");
            fs::write(&inv_pq, tampered).unwrap();
        }
        let count_ctx = SessionContext::new();
        register_pinned_parquet_table(
            &count_ctx,
            "t",
            METRICS_SCHEMA.clone(),
            inv_olap
                .snapshot_pinned_parquet_files("metrics", None)
                .unwrap(),
        )
        .unwrap();
        let count_err = count_ctx
            .sql("SELECT COUNT(*) FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("COUNT(*) must reject file num_rows mismatch against row-group sum");
        assert!(
            count_err.to_string().contains("num_rows"),
            "unexpected mismatch row count err: {count_err}"
        );

        // Case 3b: Negative rg.num_rows = -1 cleanly rejected without clamping to 0.
        fs::write(&inv_pq, &clean_bytes).unwrap();
        rewrite_parquet_footer_with_meta(&inv_pq, |meta| {
            let fmd = meta.file_metadata().clone();
            let rgs: Vec<RowGroupMetaData> = meta
                .row_groups()
                .iter()
                .cloned()
                .map(|rg| rg.into_builder().set_num_rows(-1).build().unwrap())
                .collect();
            let neg_fmd = FileMetaData::new(
                fmd.version(),
                -1,
                fmd.created_by().map(str::to_owned),
                fmd.key_value_metadata().cloned(),
                fmd.schema_descr_ptr(),
                fmd.column_orders().cloned(),
            );
            ParquetMetaDataBuilder::new(neg_fmd)
                .set_row_groups(rgs)
                .build()
        });
        let neg_rg_ctx = SessionContext::new();
        register_pinned_parquet_table(
            &neg_rg_ctx,
            "t",
            METRICS_SCHEMA.clone(),
            inv_olap
                .snapshot_pinned_parquet_files("metrics", None)
                .unwrap(),
        )
        .unwrap();
        let neg_rg_err = neg_rg_ctx
            .sql("SELECT name FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("negative row-group num_rows must fail cleanly");
        assert!(
            neg_rg_err.to_string().contains("negative"),
            "unexpected negative rg err: {neg_rg_err}"
        );

        // Case 3c: Negative col.data_page_offset = -5 returns Err without panicking in byte_range().
        fs::write(&inv_pq, &clean_bytes).unwrap();
        rewrite_parquet_footer_with_meta(&inv_pq, |meta| {
            let mut b = meta.into_builder();
            let mut rgs = b.take_row_groups();
            let mut rg_b = rgs.remove(0).into_builder();
            let mut cols = rg_b.take_columns();
            let bad_col = cols
                .remove(0)
                .into_builder()
                .set_data_page_offset(-5)
                .set_dictionary_page_offset(None)
                .build()
                .unwrap();
            cols.insert(0, bad_col);
            rg_b = rg_b.set_column_metadata(cols);
            b = b.add_row_group(rg_b.build().unwrap());
            b.build()
        });
        let neg_off_ctx = SessionContext::new();
        register_pinned_parquet_table(
            &neg_off_ctx,
            "t",
            METRICS_SCHEMA.clone(),
            inv_olap
                .snapshot_pinned_parquet_files("metrics", None)
                .unwrap(),
        )
        .unwrap();
        let neg_off_err = neg_off_ctx
            .sql("SELECT name FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("negative data_page_offset must fail cleanly without byte_range panic");
        assert!(
            neg_off_err.to_string().contains("data_page_offset"),
            "unexpected negative data_page_offset err: {neg_off_err}"
        );
    }

    fn test_push_vlq(buf: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            buf.push((v as u8 & 0x7f) | 0x80);
            v >>= 7;
        }
        buf.push(v as u8);
    }

    fn test_push_zig_zag(buf: &mut Vec<u8>, v: i64) {
        let enc = ((v << 1) ^ (v >> 63)) as u64;
        test_push_vlq(buf, enc);
    }

    fn wrap_thrift_footer_parquet(footer: &[u8]) -> Vec<u8> {
        let mut out = b"PAR1".to_vec();
        out.extend_from_slice(footer);
        out.extend_from_slice(&u32::try_from(footer.len()).unwrap().to_le_bytes());
        out.extend_from_slice(b"PAR1");
        out
    }

    #[tokio::test]
    async fn schema_aware_thrift_deep_schema_dict_bomb_and_wire_differential_reject_pre_decode() {
        use parquet::arrow::ArrowWriter;
        use parquet::file::metadata::ParquetMetaDataWriter;
        use parquet::file::properties::{EnabledStatistics, WriterProperties};

        // 1. Deep SchemaElement.num_children chain (depth 40 at Thrift wire depth 2).
        let mut deep_footer = vec![0x15_u8, 0x02, 0x19, 0xfc];
        test_push_vlq(&mut deep_footer, 40);
        for _ in 0..39 {
            // 4: name = "g", 5: num_children = 1, Stop
            deep_footer.extend_from_slice(&[0x48, 0x01, b'g', 0x15, 0x02, 0x00]);
        }
        // leaf 39: 1: type = INT32(1), 3: repetition = REQUIRED(0), 4: name = "v", Stop
        deep_footer.extend_from_slice(&[0x15, 0x02, 0x25, 0x00, 0x18, 0x01, b'v', 0x00]);
        // 3: num_rows = 0, 4: row_groups = [], Stop
        deep_footer.extend_from_slice(&[0x16, 0x00, 0x19, 0x0c, 0x00]);
        let deep_bytes = wrap_thrift_footer_parquet(&deep_footer);

        let dir = non_tmp_dir();
        let catalog = fabric_store::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        fs::create_dir_all(&part_dir).unwrap();
        let test_pq = part_dir.join("part-guard-r16.parquet");
        fs::write(&test_pq, deep_bytes).unwrap();
        let ctx1 = SessionContext::new();
        register_pinned_parquet_table(
            &ctx1,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let deep_err = ctx1
            .sql("SELECT name FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("deep SchemaElement.num_children chain must reject pre-decode");
        assert!(
            deep_err.to_string().contains("depth"),
            "unexpected deep schema err: {deep_err}"
        );

        // 2. Wire-type / field-ID differential on FileMetaData.row_groups (field 4 as Byte 0x13).
        let diff_footer = [
            0x15_u8, 0x02, // 1: version = 1
            0x19, 0x1c, // 2: schema list<struct> len=1
            0x48, 0x01, b'r', 0x00, // SchemaElement { 4: name="r" }
            0x16, 0x00, // 3: num_rows = 0
            0x13, 0xfc, 0x80, 0xc2, 0xd7, 0x2f, // 4: declared Byte(3) hiding 100M list
            0x00,
        ];
        fs::write(&test_pq, wrap_thrift_footer_parquet(&diff_footer)).unwrap();
        let ctx2 = SessionContext::new();
        register_pinned_parquet_table(
            &ctx2,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let diff_err = ctx2
            .sql("SELECT name FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("wire-type mismatch on FileMetaData.row_groups must reject pre-decode");
        assert!(
            diff_err.to_string().contains("wire type"),
            "unexpected wire-type differential err: {diff_err}"
        );

        // Build clean single-row METRICS file for page-header & dictionary-bomb tests.
        fs::remove_file(&test_pq).unwrap();
        let clean_batch = metrics_to_record_batch(&[MetricPoint {
            timestamp_ms: 1_700_000_000_000_i64,
            name: "guard.base".into(),
            value: 42.0,
            host: "h".into(),
            slice: "spindle".into(),
            unit: "svc".into(),
            pid: None,
            labels_json: "{}".into(),
        }])
        .unwrap();
        let props = WriterProperties::builder()
            .set_dictionary_enabled(false)
            .set_statistics_enabled(EnabledStatistics::None)
            .build();
        let mut writer = ArrowWriter::try_new(
            fs::File::create(&test_pq).unwrap(),
            clean_batch.schema(),
            Some(props),
        )
        .unwrap();
        writer.write(&clean_batch).unwrap();
        writer.close().unwrap();
        let clean_bytes = fs::read(&test_pq).unwrap();

        // 3. PageHeader wire-type differential on field 5 (data_page_header as BINARY 0x28).
        {
            let mut bad_page_hdr = vec![0x15_u8, 0x00, 0x15, 0x20, 0x15, 0x08, 0x28, 0x04];
            bad_page_hdr.extend_from_slice(b"evil");
            bad_page_hdr.push(0x00);
            let meta = ParquetMetaDataReader::new()
                .parse_and_finish(&fs::File::open(&test_pq).unwrap())
                .unwrap();
            let orig_len = clean_bytes.len();
            let old_footer_len = u32::from_le_bytes([
                clean_bytes[orig_len - 8],
                clean_bytes[orig_len - 7],
                clean_bytes[orig_len - 6],
                clean_bytes[orig_len - 5],
            ]) as usize;
            let data_end = orig_len - 8 - old_footer_len;
            let new_off = data_end as i64;
            let mut out = clean_bytes[..data_end].to_vec();
            out.extend_from_slice(&bad_page_hdr);
            out.extend_from_slice(&[0_u8; 4]);
            let mut b = meta.into_builder();
            let mut rgs = b.take_row_groups();
            let mut rg_b = rgs.remove(0).into_builder();
            let mut cols = rg_b.take_columns();
            let tampered = cols
                .remove(1)
                .into_builder()
                .set_data_page_offset(new_off)
                .set_dictionary_page_offset(None)
                .set_total_compressed_size((bad_page_hdr.len() + 4) as i64)
                .build()
                .unwrap();
            cols.insert(1, tampered);
            rg_b = rg_b.set_column_metadata(cols);
            b = b.add_row_group(rg_b.build().unwrap());
            let mut f = fs::File::create(&test_pq).unwrap();
            std::io::Write::write_all(&mut f, &out).unwrap();
            ParquetMetaDataWriter::new(&mut f, &b.build())
                .finish()
                .unwrap();
        }
        let ctx3 = SessionContext::new();
        register_pinned_parquet_table(
            &ctx3,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let page_diff_err = ctx3
            .sql("SELECT name FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("wire-type mismatch on PageHeader.data_page_header must reject");
        assert!(
            page_diff_err.to_string().contains("wire type"),
            "unexpected page header diff err: {page_diff_err}"
        );

        // 4. Tiny DICTIONARY_PAGE advertising num_values = 50_000_000 on INT64 `value` column.
        fs::write(&test_pq, &clean_bytes).unwrap();
        {
            let meta = ParquetMetaDataReader::new()
                .parse_and_finish(&fs::File::open(&test_pq).unwrap())
                .unwrap();
            let orig_len = clean_bytes.len();
            let old_footer_len = u32::from_le_bytes([
                clean_bytes[orig_len - 8],
                clean_bytes[orig_len - 7],
                clean_bytes[orig_len - 6],
                clean_bytes[orig_len - 5],
            ]) as usize;
            let data_end = orig_len - 8 - old_footer_len;
            let (orig_col2_start, orig_col2_len) = meta.row_group(0).column(2).byte_range();
            let orig_col2_bytes =
                &clean_bytes[orig_col2_start as usize..(orig_col2_start + orig_col2_len) as usize];

            // Dictionary PageHeader: type=2, uncomp=16, comp=16, dict_hdr{num_values=50_000_000, enc=0}
            let mut dict_hdr = vec![0x15_u8, 0x04, 0x15, 0x20, 0x15, 0x20, 0x4c, 0x15];
            test_push_zig_zag(&mut dict_hdr, 50_000_000);
            dict_hdr.extend_from_slice(&[0x15, 0x00, 0x00, 0x00]);
            let dict_off = data_end as i64;
            let data_off = (data_end + dict_hdr.len() + 16) as i64;
            let total_comp = (dict_hdr.len() + 16 + orig_col2_bytes.len()) as i64;

            let mut out = clean_bytes[..data_end].to_vec();
            out.extend_from_slice(&dict_hdr);
            out.extend_from_slice(&[0_u8; 16]);
            out.extend_from_slice(orig_col2_bytes);

            let mut b = meta.into_builder();
            let mut rgs = b.take_row_groups();
            let mut rg_b = rgs.remove(0).into_builder();
            let mut cols = rg_b.take_columns();
            let tampered2 = cols
                .remove(2)
                .into_builder()
                .set_dictionary_page_offset(Some(dict_off))
                .set_data_page_offset(data_off)
                .set_total_compressed_size(total_comp)
                .build()
                .unwrap();
            cols.insert(2, tampered2);
            rg_b = rg_b.set_column_metadata(cols);
            b = b.add_row_group(rg_b.build().unwrap());
            let mut f = fs::File::create(&test_pq).unwrap();
            std::io::Write::write_all(&mut f, &out).unwrap();
            ParquetMetaDataWriter::new(&mut f, &b.build())
                .finish()
                .unwrap();
        }
        let ctx4 = SessionContext::new();
        register_pinned_parquet_table(
            &ctx4,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let dict_bomb_err = ctx4
            .sql("SELECT value FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("tiny dictionary page claiming 50M values must reject before DictDecoder");
        assert!(
            dict_bomb_err.to_string().contains("Resources exhausted"),
            "unexpected dict bomb err: {dict_bomb_err}"
        );
    }

    #[tokio::test]
    async fn sixty_five_k_row_group_queries_succeed_under_unified_pool_and_olap_splits_row_groups()
    {
        use parquet::arrow::ArrowWriter;
        use parquet::basic::{Compression, ZstdLevel};
        use parquet::file::properties::WriterProperties;

        // 1. Valid single 65,536-row row group with 512 B distinct labels (~33.5 MiB uncompressed)
        // succeeds both `LIMIT 1` projection and `COUNT(*)` inside UNIFIED_POOL_BYTES (64 MiB).
        let dir = non_tmp_dir();
        let catalog = fabric_store::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        fs::create_dir_all(&part_dir).unwrap();
        let pq_path = part_dir.join("part-65k-512b.parquet");
        let pts: Vec<MetricPoint> = (0..65_536_i64)
            .map(|i| MetricPoint {
                timestamp_ms: 1_700_000_000_000_i64 + i,
                name: "big.rg.metric".into(),
                value: i as f64,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "u".into(),
                pid: None,
                labels_json: format!("{{\"k\":\"{i:0503}\"}}"),
            })
            .collect();
        let big_batch = metrics_to_record_batch(&pts).unwrap();
        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(1).unwrap()))
            .set_max_row_group_row_count(Some(65_536))
            .build();
        let mut writer = ArrowWriter::try_new(
            fs::File::create(&pq_path).unwrap(),
            big_batch.schema(),
            Some(props),
        )
        .unwrap();
        writer.write(&big_batch).unwrap();
        writer.close().unwrap();

        let pinned = olap.snapshot_pinned_parquet_files("metrics", None).unwrap();
        let env = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::new(GreedyMemoryPool::new(UNIFIED_POOL_BYTES)))
            .build_arc()
            .unwrap();
        let ctx =
            SessionContext::new_with_config_rt(SessionConfig::new().with_target_partitions(1), env);
        register_pinned_parquet_table(&ctx, "t", METRICS_SCHEMA.clone(), pinned).unwrap();

        let lim = ctx
            .sql("SELECT labels_json FROM t LIMIT 1")
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        assert_eq!(batches_to_json_rows(&lim).unwrap().len(), 1);

        let cnt = ctx
            .sql("SELECT COUNT(*) AS cnt FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        assert_eq!(batches_to_json_rows(&cnt).unwrap()[0]["cnt"], 65_536);

        // 2. OlapParquetStore splits > 16,384-row writes into <= 16,384-row groups.
        let olap_dir = non_tmp_dir();
        let olap_cat = fabric_store::CatalogStore::open(olap_dir.path()).unwrap();
        let olap2 = OlapParquetStore::new(olap_cat.layout().clone());
        let written = olap2.write_metrics(&pts[..17_000]).unwrap().pop().unwrap();
        let written_meta = ParquetMetaDataReader::new()
            .parse_and_finish(&fs::File::open(&written).unwrap())
            .unwrap();
        assert!(
            written_meta.num_row_groups() >= 2,
            "OlapParquetStore must split 17_000-row write into >= 2 row groups"
        );
        for rg in written_meta.row_groups() {
            assert!(
                rg.num_rows() <= 16_384,
                "row group rows {} exceeds 16_384 cap",
                rg.num_rows()
            );
        }
    }

    fn test_encode_base64(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0];
            let b1 = chunk.get(1).copied().unwrap_or(0);
            let b2 = chunk.get(2).copied().unwrap_or(0);
            out.push(TABLE[(b0 >> 2) as usize] as char);
            out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
            if chunk.len() > 1 {
                out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
            } else {
                out.push('=');
            }
            if chunk.len() > 2 {
                out.push(TABLE[(b2 & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
        out
    }

    fn craft_ipc_omitted_fields_b64() -> String {
        let mut fb = Vec::with_capacity(40);
        fb.extend_from_slice(&16_u32.to_le_bytes()); // root offset -> Message table
        // Message vtable @ 4: vtable_len=10, table_len=12, version=0, header_type=4, header=8 (+2 pad)
        for v in [10_u16, 12, 0, 4, 8, 0] {
            fb.extend_from_slice(&v.to_le_bytes());
        }
        // Message table @ 16: soffset=12, header_type=1(Schema), uoffset(Schema@36 - 24 = 12)
        fb.extend_from_slice(&12_i32.to_le_bytes());
        fb.extend_from_slice(&[1_u8, 0, 0, 0]);
        fb.extend_from_slice(&12_u32.to_le_bytes());
        // Schema vtable @ 28 (empty, no fields slot): vtable_len=4, table_len=4 (+4 pad)
        for v in [4_u16, 4, 0, 0] {
            fb.extend_from_slice(&v.to_le_bytes());
        }
        // Schema table @ 36: soffset(36 - 28 = 8)
        fb.extend_from_slice(&8_i32.to_le_bytes());
        assert!(arrow::ipc::root_as_message(&fb).is_ok());
        test_encode_base64(&fb)
    }

    fn craft_ipc_aliased_kv_b64(repeats: usize, val_len: usize) -> String {
        let vec_data_off = 60_usize;
        let kv_vt_off = vec_data_off + repeats * 4;
        let kv_tbl_off = kv_vt_off + 8;
        let key_str_off = kv_tbl_off + 12;
        let val_str_off = key_str_off + 8;
        let mut fb = Vec::with_capacity(val_str_off + 8 + val_len);

        fb.extend_from_slice(&16_u32.to_le_bytes());
        for v in [10_u16, 12, 0, 4, 8, 0] {
            fb.extend_from_slice(&v.to_le_bytes());
        }
        fb.extend_from_slice(&12_i32.to_le_bytes());
        fb.extend_from_slice(&[1_u8, 0, 0, 0]);
        fb.extend_from_slice(&16_u32.to_le_bytes());
        // Schema vtable @ 28: endianness=0, fields=4, custom_metadata=8 (+2 pad)
        for v in [10_u16, 12, 0, 4, 8, 0] {
            fb.extend_from_slice(&v.to_le_bytes());
        }
        // Schema table @ 40: soffset=12, fields->52 (offset 8), custom_metadata->56 (offset 8)
        fb.extend_from_slice(&12_i32.to_le_bytes());
        fb.extend_from_slice(&8_u32.to_le_bytes());
        fb.extend_from_slice(&8_u32.to_le_bytes());
        // Empty fields vector @ 52
        fb.extend_from_slice(&0_u32.to_le_bytes());
        // Shared custom_metadata vector @ 56: all entries alias single KeyValue table
        fb.extend_from_slice(&(repeats as u32).to_le_bytes());
        for i in 0..repeats {
            let elem_pos = vec_data_off + i * 4;
            let rel = (kv_tbl_off - elem_pos) as u32;
            fb.extend_from_slice(&rel.to_le_bytes());
        }
        // KeyValue vtable @ kv_vt_off: vtable_len=8, table_len=12, VT_KEY=4, VT_VALUE=8
        for v in [8_u16, 12, 4, 8] {
            fb.extend_from_slice(&v.to_le_bytes());
        }
        // KeyValue table @ kv_tbl_off
        fb.extend_from_slice(&8_i32.to_le_bytes());
        fb.extend_from_slice(&((key_str_off - (kv_tbl_off + 4)) as u32).to_le_bytes());
        fb.extend_from_slice(&((val_str_off - (kv_tbl_off + 8)) as u32).to_le_bytes());
        // Key string "k" @ key_str_off (padded to 8 bytes)
        fb.extend_from_slice(&1_u32.to_le_bytes());
        fb.extend_from_slice(&[b'k', 0, 0, 0]);
        // Value string @ val_str_off
        fb.extend_from_slice(&(val_len as u32).to_le_bytes());
        fb.resize(fb.len() + val_len, b'x');
        fb.push(0);
        while fb.len() % 4 != 0 {
            fb.push(0);
        }
        assert!(arrow::ipc::root_as_message(&fb).is_ok());
        test_encode_base64(&fb)
    }

    #[tokio::test]
    async fn delta_encodings_v1_bitpacked_levels_and_malformed_ipc_arrow_schema_reject_pre_decode()
    {
        use parquet::arrow::ArrowWriter;
        use parquet::file::metadata::{KeyValue, ParquetMetaDataWriter};
        use parquet::file::properties::{EnabledStatistics, WriterProperties};

        let dir = non_tmp_dir();
        let catalog = fabric_store::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        fs::create_dir_all(&part_dir).unwrap();
        let pq_path = part_dir.join("part-r17-attack.parquet");

        let base_batch = metrics_to_record_batch(&[MetricPoint {
            timestamp_ms: 1_700_000_000_000_i64,
            name: "r17.base".into(),
            value: 1.0,
            host: "h".into(),
            slice: "spindle".into(),
            unit: "svc".into(),
            pid: None,
            labels_json: "{}".into(),
        }])
        .unwrap();
        let props = WriterProperties::builder()
            .set_dictionary_enabled(false)
            .set_statistics_enabled(EnabledStatistics::None)
            .build();
        let mut writer = ArrowWriter::try_new(
            fs::File::create(&pq_path).unwrap(),
            base_batch.schema(),
            Some(props),
        )
        .unwrap();
        writer.write(&base_batch).unwrap();
        writer.close().unwrap();
        let clean_bytes = fs::read(&pq_path).unwrap();
        let orig_len = clean_bytes.len();
        let old_footer_len = u32::from_le_bytes([
            clean_bytes[orig_len - 8],
            clean_bytes[orig_len - 7],
            clean_bytes[orig_len - 6],
            clean_bytes[orig_len - 5],
        ]) as usize;
        let data_end = orig_len - 8 - old_footer_len;

        // 1. DELTA_LENGTH_BYTE_ARRAY (encoding=6) 1-row page advertising huge delta header count.
        {
            // DataPageHeader: num_values=1 (zig 2), encoding=6 (zig 12), def=3 (RLE), rep=3 (RLE)
            let mut delta_page = vec![
                0x15_u8, 0x00, 0x15, 0x20, 0x15, 0x20, 0x2c, 0x15, 0x02, 0x15, 0x0c, 0x15, 0x06,
                0x15, 0x06, 0x00, 0x00,
            ];
            let hdr_len = delta_page.len();
            delta_page.extend_from_slice(&[0x01, 0x01, 0x80, 0x80, 0xf2, 0xee, 0x01]);
            delta_page.resize(hdr_len + 16, 0);

            let meta = ParquetMetaDataReader::new()
                .parse_and_finish(&fs::File::open(&pq_path).unwrap())
                .unwrap();
            let new_off = data_end as i64;
            let mut b = meta.into_builder();
            let mut rgs = b.take_row_groups();
            let mut rg_b = rgs.remove(0).into_builder();
            let mut cols = rg_b.take_columns();
            let col1 = cols
                .remove(1)
                .into_builder()
                .set_data_page_offset(new_off)
                .set_dictionary_page_offset(None)
                .set_total_compressed_size(delta_page.len() as i64)
                .build()
                .unwrap();
            cols.insert(1, col1);
            rg_b = rg_b.set_column_metadata(cols);
            b = b.add_row_group(rg_b.build().unwrap());

            let mut out = clean_bytes[..data_end].to_vec();
            out.extend_from_slice(&delta_page);
            let mut f = fs::File::create(&pq_path).unwrap();
            std::io::Write::write_all(&mut f, &out).unwrap();
            ParquetMetaDataWriter::new(&mut f, &b.build())
                .finish()
                .unwrap();
        }
        let store_delta_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&pq_path).unwrap(),
                Some(&METRICS_SCHEMA),
                128,
            )
            .expect_err("DELTA_LENGTH_BYTE_ARRAY page must reject pre-decode in store guard");
        assert!(
            store_delta_err.contains("delta") || store_delta_err.contains("encoding"),
            "unexpected store delta err: {store_delta_err}"
        );
        let q_ctx1 = SessionContext::new();
        register_pinned_parquet_table(
            &q_ctx1,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let query_delta_err = q_ctx1
            .sql("SELECT name FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("DELTA_LENGTH_BYTE_ARRAY page must reject pre-decode in query");
        assert!(
            query_delta_err.to_string().contains("delta")
                || query_delta_err.to_string().contains("encoding"),
            "unexpected query delta err: {query_delta_err}"
        );

        // 2. V1 BIT_PACKED definition_level_encoding=4 with empty payload on nullable `pid` (col 6).
        fs::write(&pq_path, &clean_bytes).unwrap();
        {
            // DataPageHeader: num_values=1, enc=0 (PLAIN), def=4 (zig 8 BIT_PACKED), rep=3 (RLE)
            let bp_page = [
                0x15_u8, 0x00, 0x15, 0x00, 0x15, 0x00, 0x2c, 0x15, 0x02, 0x15, 0x00, 0x15, 0x08,
                0x15, 0x06, 0x00, 0x00,
            ];
            let meta = ParquetMetaDataReader::new()
                .parse_and_finish(&fs::File::open(&pq_path).unwrap())
                .unwrap();
            let new_off = data_end as i64;
            let mut b = meta.into_builder();
            let mut rgs = b.take_row_groups();
            let mut rg_b = rgs.remove(0).into_builder();
            let mut cols = rg_b.take_columns();
            let col6 = cols
                .remove(6)
                .into_builder()
                .set_data_page_offset(new_off)
                .set_dictionary_page_offset(None)
                .set_total_compressed_size(bp_page.len() as i64)
                .set_total_uncompressed_size(0)
                .build()
                .unwrap();
            cols.insert(6, col6);
            rg_b = rg_b.set_column_metadata(cols);
            b = b.add_row_group(rg_b.build().unwrap());

            let mut out = clean_bytes[..data_end].to_vec();
            out.extend_from_slice(&bp_page);
            let mut f = fs::File::create(&pq_path).unwrap();
            std::io::Write::write_all(&mut f, &out).unwrap();
            ParquetMetaDataWriter::new(&mut f, &b.build())
                .finish()
                .unwrap();
        }
        let store_bp_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&pq_path).unwrap(),
                Some(&METRICS_SCHEMA),
                128,
            )
            .expect_err("V1 BIT_PACKED level encoding must reject pre-decode in store guard");
        assert!(
            store_bp_err.contains("V1 level encoding"),
            "unexpected store BIT_PACKED err: {store_bp_err}"
        );
        let q_ctx2 = SessionContext::new();
        register_pinned_parquet_table(
            &q_ctx2,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let query_bp_err = q_ctx2
            .sql("SELECT pid FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("V1 BIT_PACKED level encoding must reject pre-decode in query");
        assert!(
            query_bp_err.to_string().contains("V1 level encoding"),
            "unexpected query BIT_PACKED err: {query_bp_err}"
        );

        let rewrite_arrow_schema_kv = |pq_path: &Path, val: String| {
            rewrite_parquet_footer_with_meta(pq_path, |meta| {
                let fm = meta.file_metadata();
                let new_fm = parquet::file::metadata::FileMetaData::new(
                    fm.version(),
                    fm.num_rows(),
                    fm.created_by().map(str::to_owned),
                    Some(vec![KeyValue::new("ARROW:schema".to_owned(), val.clone())]),
                    fm.schema_descr_ptr(),
                    fm.column_orders().map(|o| o.to_vec()),
                );
                parquet::file::metadata::ParquetMetaData::new(new_fm, meta.row_groups().to_vec())
            });
        };

        // 3. Valid FlatBuffer ARROW:schema omitting `fields` must reject before fb_to_schema unwrap().
        fs::write(&pq_path, &clean_bytes).unwrap();
        rewrite_arrow_schema_kv(&pq_path, craft_ipc_omitted_fields_b64());
        let store_nofields_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&pq_path).unwrap(),
                Some(&METRICS_SCHEMA),
                128,
            )
            .expect_err("ARROW:schema omitting fields must reject pre-decode without panic");
        assert!(
            store_nofields_err.contains("omits required fields"),
            "unexpected store omitted-fields err: {store_nofields_err}"
        );
        let q_ctx3 = SessionContext::new();
        register_pinned_parquet_table(
            &q_ctx3,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let query_nofields_err = q_ctx3
            .sql("SELECT name FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("ARROW:schema omitting fields must reject pre-decode in query");
        assert!(
            query_nofields_err
                .to_string()
                .contains("omits required fields"),
            "unexpected query omitted-fields err: {query_nofields_err}"
        );

        // 4a. Shared-DAG FlatBuffer ARROW:schema (~13 KiB wire -> ~3.1 MiB expanded > 2 MiB hard cap).
        fs::write(&pq_path, &clean_bytes).unwrap();
        rewrite_arrow_schema_kv(&pq_path, craft_ipc_aliased_kv_b64(2_048, 1_500));
        let store_dag_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&pq_path).unwrap(),
                Some(&METRICS_SCHEMA),
                128,
            )
            .expect_err("DAG-amplified ARROW:schema exceeding 2 MiB must reject");
        assert!(
            store_dag_err.contains("Resources exhausted"),
            "unexpected store DAG err: {store_dag_err}"
        );

        // 4b. Moderate DAG expansion (~1.05 MiB < 2 MiB) from tiny ~6 KiB wire must reject pre-try_new
        // under a tight 256 KiB query pool.
        fs::write(&pq_path, &clean_bytes).unwrap();
        rewrite_arrow_schema_kv(&pq_path, craft_ipc_aliased_kv_b64(700, 1_500));
        let tight_env = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::new(GreedyMemoryPool::new(256 * 1024)))
            .build_arc()
            .unwrap();
        let tight_ctx = SessionContext::new_with_config_rt(
            SessionConfig::new().with_target_partitions(1),
            tight_env,
        );
        register_pinned_parquet_table(
            &tight_ctx,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let tight_dag_err = tight_ctx
            .sql("SELECT name FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("DAG-amplified ARROW:schema must exceed tight pool pre-try_new");
        assert!(
            tight_dag_err.to_string().contains("Resources exhausted"),
            "unexpected query DAG err: {tight_dag_err}"
        );
    }

    #[tokio::test]
    async fn round18_dict_rle_panics_bool_dict_ipc_dag_preflight_and_toctou_reject_cleanly() {
        use arrow::array::BooleanArray;
        use arrow::datatypes::Field;
        use arrow::ipc::writer::{DictionaryTracker, IpcDataGenerator, IpcWriteOptions};
        use parquet::arrow::ArrowWriter;
        use parquet::basic::{Compression, Encoding};
        use parquet::file::metadata::{KeyValue, ParquetMetaDataWriter};
        use parquet::file::properties::{EnabledStatistics, WriterProperties};

        let write_varint_u32 = |out: &mut Vec<u8>, mut v: u32| {
            loop {
                let mut byte = (v & 0x7f) as u8;
                v >>= 7;
                if v != 0 {
                    byte |= 0x80;
                }
                out.push(byte);
                if v == 0 {
                    break;
                }
            }
        };
        let encode_thrift_zigzag = |v: i32| -> u32 { ((v << 1) ^ (v >> 31)) as u32 };
        let craft_dict_page = |dict_num_vals: i32, payload: &[u8]| -> Vec<u8> {
            let mut hdr = Vec::new();
            // type=2(DICTIONARY_PAGE), uncomp, comp, dictionary_page_header(num_values, encoding=0 PLAIN)
            hdr.extend_from_slice(&[0x15, encode_thrift_zigzag(2) as u8]);
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.extend_from_slice(&[0x4c, 0x15]);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(dict_num_vals));
            hdr.extend_from_slice(&[0x15, 0x00, 0x00, 0x00]);
            hdr.extend_from_slice(payload);
            hdr
        };
        let craft_v1_data_page = |num_vals: i32, val_enc: i32, payload: &[u8]| -> Vec<u8> {
            let mut hdr = Vec::new();
            hdr.extend_from_slice(&[0x15, 0x00]); // type = DATA_PAGE(0)
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.extend_from_slice(&[0x2c, 0x15]); // field 5 DataPageHeader struct
            write_varint_u32(&mut hdr, encode_thrift_zigzag(num_vals));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(val_enc));
            hdr.extend_from_slice(&[0x15, 0x06, 0x15, 0x06, 0x00, 0x00]); // def=3(RLE), rep=3(RLE)
            hdr.extend_from_slice(payload);
            hdr
        };

        let rewrite_col_chunk = |base_path: &Path,
                                 col_idx: usize,
                                 dict_off_rel: Option<i64>,
                                 data_off_rel: i64,
                                 uncomp_total: i64,
                                 encs: Vec<Encoding>,
                                 chunk_bytes: &[u8]| {
            let orig = fs::read(base_path).unwrap();
            let orig_len = orig.len();
            let old_footer_len =
                u32::from_le_bytes(orig[orig_len - 8..orig_len - 4].try_into().unwrap()) as usize;
            let data_end = orig_len - 8 - old_footer_len;
            let meta =
                ParquetMetaDataReader::decode_metadata(&orig[data_end..orig_len - 8]).unwrap();
            let base_off = data_end as i64;
            let mut b = meta.into_builder();
            let mut rgs = b.take_row_groups();
            let mut rg_b = rgs.remove(0).into_builder();
            let mut cols = rg_b.take_columns();
            let col = cols
                .remove(col_idx)
                .into_builder()
                .set_encodings(encs)
                .set_compression(Compression::UNCOMPRESSED)
                .set_dictionary_page_offset(dict_off_rel.map(|r| base_off + r))
                .set_data_page_offset(base_off + data_off_rel)
                .set_total_compressed_size(chunk_bytes.len() as i64)
                .set_total_uncompressed_size(uncomp_total)
                .build()
                .unwrap();
            cols.insert(col_idx, col);
            rg_b = rg_b.set_column_metadata(cols);
            b = b.add_row_group(rg_b.build().unwrap());

            let mut out = orig[..data_end].to_vec();
            out.extend_from_slice(chunk_bytes);
            let mut f = fs::File::create(base_path).unwrap();
            std::io::Write::write_all(&mut f, &out).unwrap();
            ParquetMetaDataWriter::new(&mut f, &b.build())
                .finish()
                .unwrap();
        };

        let dir = non_tmp_dir();
        let catalog = fabric_store::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        fs::create_dir_all(&part_dir).unwrap();
        let clean_path = part_dir.join("part-clean.parquet");
        let base_batch = metrics_to_record_batch(&[MetricPoint {
            timestamp_ms: 1_700_000_000_000_i64,
            name: "r18.base".into(),
            value: 1.0,
            host: "h".into(),
            slice: "spindle".into(),
            unit: "svc".into(),
            pid: Some(42),
            labels_json: "{}".into(),
        }])
        .unwrap();
        let props = WriterProperties::builder()
            .set_dictionary_enabled(false)
            .set_statistics_enabled(EnabledStatistics::None)
            .build();
        let mut w = ArrowWriter::try_new(
            fs::File::create(&clean_path).unwrap(),
            base_batch.schema(),
            Some(props),
        )
        .unwrap();
        w.write(&base_batch).unwrap();
        w.close().unwrap();
        let clean_bytes = fs::read(&clean_path).unwrap();
        fs::remove_file(&clean_path).unwrap();

        let pq_path = part_dir.join("part-r18-target.parquet");

        // (a1) Required BYTE_ARRAY column `name` with valid DICTIONARY_PAGE and 0-byte RLE_DICTIONARY DataPage.
        fs::write(&pq_path, &clean_bytes).unwrap();
        let mut name_dict_payload = 1_u32.to_le_bytes().to_vec();
        name_dict_payload.push(b'm');
        let dict_part = craft_dict_page(1, &name_dict_payload);
        let data_part = craft_v1_data_page(1, 8, &[]);
        let mut chunk_a1 = dict_part.clone();
        chunk_a1.extend_from_slice(&data_part);
        rewrite_col_chunk(
            &pq_path,
            1,
            Some(0),
            dict_part.len() as i64,
            name_dict_payload.len() as i64,
            vec![Encoding::PLAIN, Encoding::RLE_DICTIONARY],
            &chunk_a1,
        );
        assert!(
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&pq_path).unwrap(),
                Some(&METRICS_SCHEMA),
                128,
            )
            .is_err()
        );
        let ctx_a1 = SessionContext::new();
        register_pinned_parquet_table(
            &ctx_a1,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        assert!(
            ctx_a1
                .sql("SELECT name FROM t")
                .await
                .unwrap()
                .collect()
                .await
                .is_err()
        );

        // (a2) Optional INT64 column `pid` where RLE def levels consume entire DataPage leaving 0 value bytes.
        fs::write(&pq_path, &clean_bytes).unwrap();
        let pid_dict = craft_dict_page(1, &42_i64.to_le_bytes());
        let pid_lvl_only_data = craft_v1_data_page(1, 8, &[2, 0, 0, 0, 0x02, 0x01]);
        let mut chunk_a2 = pid_dict.clone();
        chunk_a2.extend_from_slice(&pid_lvl_only_data);
        rewrite_col_chunk(
            &pq_path,
            6,
            Some(0),
            pid_dict.len() as i64,
            14,
            vec![Encoding::PLAIN, Encoding::RLE_DICTIONARY],
            &chunk_a2,
        );
        let err_a2 = fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
            fs::File::open(&pq_path).unwrap(),
            Some(&METRICS_SCHEMA),
            128,
        )
        .expect_err("empty post-level dictionary slice must reject before DictDecoder/DictIndexDecoder panic");
        assert!(
            err_a2.contains("empty dictionary data page value payload"),
            "{err_a2}"
        );
        let ctx_a2 = SessionContext::new();
        register_pinned_parquet_table(
            &ctx_a2,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        assert!(
            ctx_a2
                .sql("SELECT pid FROM t")
                .await
                .unwrap()
                .collect()
                .await
                .is_err()
        );

        // (b) RLE_DICTIONARY data pages with bit_width = 33 and 11-byte 0x80 VLQ indicator.
        for (tag, bad_dict_data, expected_msg) in [
            ("bw33", vec![33_u8, 0x00], "bit_width 33"),
            (
                "vlq11",
                vec![
                    1_u8, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01,
                ],
                "overlong VLQ",
            ),
        ] {
            fs::write(&pq_path, &clean_bytes).unwrap();
            let dpart = craft_dict_page(1, &name_dict_payload);
            let bad_dpage = craft_v1_data_page(1, 8, &bad_dict_data);
            let mut chunk_b = dpart.clone();
            chunk_b.extend_from_slice(&bad_dpage);
            rewrite_col_chunk(
                &pq_path,
                1,
                Some(0),
                dpart.len() as i64,
                (name_dict_payload.len() + bad_dict_data.len()) as i64,
                vec![Encoding::PLAIN, Encoding::RLE_DICTIONARY],
                &chunk_b,
            );
            let store_err =
                fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                    fs::File::open(&pq_path).unwrap(),
                    Some(&METRICS_SCHEMA),
                    128,
                )
                .expect_err("malformed RLE dict payload must reject pre-decode");
            assert!(
                store_err.contains(expected_msg),
                "{tag}: unexpected store err: {store_err}"
            );
            let ctx_b = SessionContext::new();
            register_pinned_parquet_table(
                &ctx_b,
                "t",
                METRICS_SCHEMA.clone(),
                olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
            )
            .unwrap();
            let q_err = ctx_b
                .sql("SELECT name FROM t")
                .await
                .unwrap()
                .collect()
                .await
                .expect_err("malformed RLE dict payload must reject in PinnedParquetExec");
            assert!(
                q_err.to_string().contains(expected_msg),
                "{tag}: unexpected query err: {q_err}"
            );
        }

        // (c) BOOLEAN column paired with ARROW:schema Dictionary(Int32, Boolean) must reject cleanly.
        let bool_dir = non_tmp_dir();
        let bool_pq = bool_dir.path().join("bool-dict-ipc.parquet");
        let bool_schema = Arc::new(Schema::new(vec![Field::new(
            "flag",
            DataType::Boolean,
            false,
        )]));
        let bool_batch = RecordBatch::try_new(
            Arc::clone(&bool_schema),
            vec![Arc::new(BooleanArray::from(vec![true]))],
        )
        .unwrap();
        let mut bw =
            ArrowWriter::try_new(fs::File::create(&bool_pq).unwrap(), bool_schema, None).unwrap();
        bw.write(&bool_batch).unwrap();
        bw.close().unwrap();
        let dict_bool_schema = Schema::new(vec![Field::new(
            "flag",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Boolean)),
            false,
        )]);
        let ipc_opts = IpcWriteOptions::default();
        let mut dict_tracker = DictionaryTracker::new(true);
        let enc_msg = IpcDataGenerator::default().schema_to_bytes_with_dictionary_tracker(
            &dict_bool_schema,
            &mut dict_tracker,
            &ipc_opts,
        );
        let mut ipc_prefixed = vec![255_u8; 4];
        ipc_prefixed.extend_from_slice(&(enc_msg.ipc_message.len() as u32).to_le_bytes());
        ipc_prefixed.extend_from_slice(&enc_msg.ipc_message);
        let dict_bool_b64 = test_encode_base64(&ipc_prefixed);
        rewrite_parquet_footer_with_meta(&bool_pq, |meta| {
            let fm = meta.file_metadata();
            let new_fm = parquet::file::metadata::FileMetaData::new(
                fm.version(),
                fm.num_rows(),
                fm.created_by().map(str::to_owned),
                Some(vec![KeyValue::new(
                    "ARROW:schema".to_owned(),
                    dict_bool_b64.clone(),
                )]),
                fm.schema_descr_ptr(),
                fm.column_orders().map(|o| o.to_vec()),
            );
            parquet::file::metadata::ParquetMetaData::new(new_fm, meta.row_groups().to_vec())
        });
        let dict_bool_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&bool_pq).unwrap(),
                None,
                128,
            )
            .expect_err("Dictionary(Int32, Boolean) ARROW:schema must reject pre-decode");
        assert!(
            dict_bool_err.contains("dictionary-typed ARROW:schema"),
            "unexpected bool-dict err: {dict_bool_err}"
        );

        // (d) Exponential 22-level shared-child Field.children DAG must reject in preflight BEFORE
        //     arrow::ipc::root_as_message runs into TooManyTables.
        let craft_shared_children_dag_b64 = |depth: usize| -> (Vec<u8>, String) {
            let mut fb = Vec::new();
            fb.extend_from_slice(&16_u32.to_le_bytes()); // root -> Message @ 16
            // Message vtable @ 4
            for v in [10_u16, 12, 0, 4, 8, 0] {
                fb.extend_from_slice(&v.to_le_bytes());
            }
            // Message @ 16
            fb.extend_from_slice(&12_i32.to_le_bytes());
            fb.extend_from_slice(&[1_u8, 0, 0, 0]); // Schema
            fb.extend_from_slice(&12_u32.to_le_bytes()); // Schema @ 36
            // Schema vtable @ 28: VT_FIELDS(6) -> offset 4
            for v in [8_u16, 8, 0, 4] {
                fb.extend_from_slice(&v.to_le_bytes());
            }
            // Schema @ 36: soffset=8, fields->44
            fb.extend_from_slice(&8_i32.to_le_bytes());
            fb.extend_from_slice(&4_u32.to_le_bytes());
            // Schema.fields vector @ 44: len=1, elem[0] -> 68
            fb.extend_from_slice(&1_u32.to_le_bytes());
            fb.extend_from_slice(&20_u32.to_le_bytes());
            // Shared internal Field vtable @ 52 (16 bytes): VT_CHILDREN(14) -> offset 4
            for v in [16_u16, 8, 0, 0, 0, 0, 0, 4] {
                fb.extend_from_slice(&v.to_le_bytes());
            }
            // Nodes i = 0..depth at base = 68 + i * 20
            for i in 0..depth {
                let base = 68 + i * 20;
                let next_tbl = 68 + (i + 1) * 20;
                let leaf_adjust = usize::from(i + 1 == depth) * 4;
                let target = next_tbl + leaf_adjust;
                fb.extend_from_slice(&((base - 52) as i32).to_le_bytes());
                fb.extend_from_slice(&4_u32.to_le_bytes()); // children vec @ base + 8
                fb.extend_from_slice(&2_u32.to_le_bytes());
                fb.extend_from_slice(&((target - (base + 12)) as u32).to_le_bytes());
                fb.extend_from_slice(&((target - (base + 16)) as u32).to_le_bytes());
            }
            // Leaf vtable @ 68 + depth * 20 (4 bytes) + leaf Field table @ +4 (4 bytes)
            for v in [4_u16, 4] {
                fb.extend_from_slice(&v.to_le_bytes());
            }
            fb.extend_from_slice(&4_i32.to_le_bytes());
            let b64 = test_encode_base64(&fb);
            (fb, b64)
        };
        let (dag_fb, dag_b64) = craft_shared_children_dag_b64(22);
        let fb_raw_err = format!("{:?}", arrow::ipc::root_as_message(&dag_fb).unwrap_err());
        assert!(
            fb_raw_err.contains("TooManyTables"),
            "expected test DAG to overwhelm default verifier, got: {fb_raw_err}"
        );
        fs::write(&pq_path, &clean_bytes).unwrap();
        rewrite_parquet_footer_with_meta(&pq_path, |meta| {
            let fm = meta.file_metadata();
            let new_fm = parquet::file::metadata::FileMetaData::new(
                fm.version(),
                fm.num_rows(),
                fm.created_by().map(str::to_owned),
                Some(vec![KeyValue::new(
                    "ARROW:schema".to_owned(),
                    dag_b64.clone(),
                )]),
                fm.schema_descr_ptr(),
                fm.column_orders().map(|o| o.to_vec()),
            );
            parquet::file::metadata::ParquetMetaData::new(new_fm, meta.row_groups().to_vec())
        });
        let dag_err = fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
            fs::File::open(&pq_path).unwrap(),
            Some(&METRICS_SCHEMA),
            128,
        )
        .expect_err("shared-children FlatBuffer DAG must reject in pre-verifier preflight");
        assert!(
            dag_err.contains("nested ARROW:schema field children rejected"),
            "expected pre-verifier children rejection before TooManyTables, got: {dag_err}"
        );

        // (e) Dictionary decompression residency: padded 256 KiB UNCOMPRESSED dictionary page on INT64
        //     holds comp (256 KiB) + uncomp (256 KiB) + PushBuffers (256 KiB) and must trip a 600 KiB pool.
        fs::write(&pq_path, &clean_bytes).unwrap();
        let mut padded_i64_dict = vec![0_u8; 256 * 1024];
        padded_i64_dict[..8].copy_from_slice(&1_700_000_000_000_i64.to_le_bytes());
        let ts_dict = craft_dict_page(1, &padded_i64_dict);
        let ts_data = craft_v1_data_page(1, 8, &[0_u8, 0x02]);
        let mut ts_chunk = ts_dict.clone();
        ts_chunk.extend_from_slice(&ts_data);
        rewrite_col_chunk(
            &pq_path,
            0,
            Some(0),
            ts_dict.len() as i64,
            (padded_i64_dict.len() + 2) as i64,
            vec![Encoding::PLAIN, Encoding::RLE_DICTIONARY],
            &ts_chunk,
        );
        let dict_res_env = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::new(GreedyMemoryPool::new(600 * 1024)))
            .build_arc()
            .unwrap();
        let dict_res_ctx = SessionContext::new_with_config_rt(
            SessionConfig::new().with_target_partitions(1),
            dict_res_env,
        );
        register_pinned_parquet_table(
            &dict_res_ctx,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let dict_res_err = dict_res_ctx
            .sql("SELECT timestamp_ms FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("256 KiB dict comp + uncomp + chunk buffer must reject under 600 KiB pool");
        assert!(
            dict_res_err.to_string().contains("Resources exhausted"),
            "unexpected dict residency err: {dict_res_err}"
        );

        // (f) In-place TOCTOU immunity:
        // (f1) Once open_verified_parquet_batch_reader_from_file returns, mutating RG 0 on disk in-place
        //      does not affect the immutable PushBuffers snapshot already validated & buffered.
        fs::write(&pq_path, &clean_bytes).unwrap();
        let mut rg0_reader =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&pq_path).unwrap(),
                Some(&METRICS_SCHEMA),
                128,
            )
            .unwrap();
        {
            use std::io::Write;
            let meta_snap = ParquetMetaDataReader::new()
                .parse_and_finish(&fs::File::open(&pq_path).unwrap())
                .unwrap();
            let (c1_start, c1_len) = meta_snap.row_group(0).column(1).byte_range();
            let mut mut_f = fs::OpenOptions::new().write(true).open(&pq_path).unwrap();
            mut_f.seek(SeekFrom::Start(c1_start)).unwrap();
            mut_f
                .write_all(&vec![0xFF_u8; usize::try_from(c1_len).unwrap().min(64)])
                .unwrap();
            mut_f.sync_all().unwrap();
        }
        let decoded_from_snapshot: Vec<RecordBatch> =
            rg0_reader.by_ref().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(
            decoded_from_snapshot
                .iter()
                .map(RecordBatch::num_rows)
                .sum::<usize>(),
            1
        );

        // (f2) 2-row-group file: after draining RG 0 cleanly, mutating RG 1 in-place to
        //      DELTA_LENGTH_BYTE_ARRAY (6) before RG 1 is snapshotted rejects pre-decode on both
        //      VerifiedParquetBatchReader and PinnedParquetExec without reaching parquet's decoder.
        let two_rg_batch = metrics_to_record_batch(&[
            MetricPoint {
                timestamp_ms: 1_700_000_000_000_i64,
                name: "rg0".into(),
                value: 1.0,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: Some(1),
                labels_json: "{}".into(),
            },
            MetricPoint {
                timestamp_ms: 1_700_000_000_001_i64,
                name: "rg1".into(),
                value: 2.0,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: Some(2),
                labels_json: "{}".into(),
            },
        ])
        .unwrap();
        let two_rg_props = WriterProperties::builder()
            .set_dictionary_enabled(false)
            .set_compression(Compression::UNCOMPRESSED)
            .set_max_row_group_row_count(Some(1))
            .set_statistics_enabled(EnabledStatistics::None)
            .build();
        let mut tw = ArrowWriter::try_new(
            fs::File::create(&pq_path).unwrap(),
            two_rg_batch.schema(),
            Some(two_rg_props),
        )
        .unwrap();
        tw.write(&two_rg_batch.slice(0, 1)).unwrap();
        tw.write(&two_rg_batch.slice(1, 1)).unwrap();
        tw.close().unwrap();

        let two_rg_clean = fs::read(&pq_path).unwrap();
        let two_meta = ParquetMetaDataReader::new()
            .parse_and_finish(&fs::File::open(&pq_path).unwrap())
            .unwrap();
        assert_eq!(two_meta.num_row_groups(), 2);
        let (rg1_c1_start, rg1_c1_len) = two_meta.row_group(1).column(1).byte_range();

        let mut two_reader =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&pq_path).unwrap(),
                Some(&METRICS_SCHEMA),
                1,
            )
            .unwrap();
        let first_rg = two_reader.next().unwrap().unwrap();
        assert_eq!(first_rg.num_rows(), 1);
        // Flip RG 1's DataPageHeader encoding byte (0x00 PLAIN -> 0x0c = zigzag(6) DELTA_LENGTH_BYTE_ARRAY)
        // inside DataPageHeader while holding file length constant (true in-place TOCTOU overwrite).
        {
            use std::io::Write;
            let raw_rg1 =
                &two_rg_clean[rg1_c1_start as usize..(rg1_c1_start + rg1_c1_len) as usize];
            let needle = [0x2c_u8, 0x15, 0x02, 0x15, 0x00, 0x15, 0x06, 0x15, 0x06];
            let enc_rel = raw_rg1
                .windows(needle.len())
                .position(|w| w == needle)
                .expect("expected V1 PLAIN DataPageHeader signature in uncompressed RG 1")
                + 4;
            let mut f = fs::OpenOptions::new().write(true).open(&pq_path).unwrap();
            f.seek(SeekFrom::Start(rg1_c1_start + enc_rel as u64))
                .unwrap();
            f.write_all(&[0x0c_u8]).unwrap();
            f.sync_all().unwrap();
        }
        let rg1_err = two_reader.next().unwrap().expect_err(
            "in-place mutation of RG 1 to DELTA_LENGTH_BYTE_ARRAY must reject pre-decode",
        );
        assert!(
            rg1_err.to_string().contains("encoding 6"),
            "unexpected TOCTOU RG 1 err: {rg1_err}"
        );

        // Also verify PinnedParquetExec snapshots each row group into immutable PushBuffers and
        // detects in-place mutation of a subsequent unread row group pre-decode.
        let eight_rg_pts: Vec<MetricPoint> = (0..8_i64)
            .map(|i| MetricPoint {
                timestamp_ms: 1_700_000_000_000_i64 + i,
                name: format!("rg{i}.{}", "n".repeat(32 * 1024)),
                value: i as f64,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: Some(i as u32),
                labels_json: "{}".into(),
            })
            .collect();
        let eight_rg_batch = metrics_to_record_batch(&eight_rg_pts).unwrap();
        let eight_rg_props = WriterProperties::builder()
            .set_dictionary_enabled(false)
            .set_compression(Compression::UNCOMPRESSED)
            .set_max_row_group_row_count(Some(1))
            .set_statistics_enabled(EnabledStatistics::None)
            .build();
        let mut fw = ArrowWriter::try_new(
            fs::File::create(&pq_path).unwrap(),
            eight_rg_batch.schema(),
            Some(eight_rg_props),
        )
        .unwrap();
        for i in 0..8 {
            fw.write(&eight_rg_batch.slice(i, 1)).unwrap();
        }
        fw.close().unwrap();
        let eight_rg_clean = fs::read(&pq_path).unwrap();
        let eight_meta = ParquetMetaDataReader::new()
            .parse_and_finish(&fs::File::open(&pq_path).unwrap())
            .unwrap();
        assert_eq!(eight_meta.num_row_groups(), 8);
        let (rg7_c1_start, rg7_c1_len) = eight_meta.row_group(7).column(1).byte_range();
        let toctou_pool: Arc<dyn datafusion::execution::memory_pool::MemoryPool> =
            Arc::new(GreedyMemoryPool::new(4 * 1024 * 1024));
        let toctou_env = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::clone(&toctou_pool))
            .build_arc()
            .unwrap();
        let toctou_ctx = SessionContext::new_with_config_rt(
            SessionConfig::new().with_target_partitions(1),
            toctou_env,
        );
        let exec_toctou = PinnedParquetExec::try_new(
            METRICS_SCHEMA.clone(),
            Arc::from([Arc::from(
                olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
            )]),
            Some(vec![1]),
            None,
        )
        .unwrap();
        let stream = exec_toctou.execute(0, toctou_ctx.task_ctx()).unwrap();
        for _ in 0..80 {
            if toctou_pool.reserved() >= 64 * 1024 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            toctou_pool.reserved() >= 64 * 1024,
            "expected early row groups to queue before mutating rg7 (reserved={})",
            toctou_pool.reserved()
        );
        {
            use std::io::Write;
            let raw_rg7 =
                &eight_rg_clean[rg7_c1_start as usize..(rg7_c1_start + rg7_c1_len) as usize];
            let needle = [0x2c_u8, 0x15, 0x02, 0x15, 0x00, 0x15, 0x06, 0x15, 0x06];
            let enc_rel = raw_rg7
                .windows(needle.len())
                .position(|w| w == needle)
                .unwrap()
                + 4;
            let mut f = fs::OpenOptions::new().write(true).open(&pq_path).unwrap();
            f.seek(SeekFrom::Start(rg7_c1_start + enc_rel as u64))
                .unwrap();
            f.write_all(&[0x0c_u8]).unwrap();
            f.sync_all().unwrap();
        }
        let next_res = datafusion::physical_plan::common::collect(stream)
            .await
            .expect_err("PinnedParquetExec must reject mutated RG 7 pre-decode");
        assert!(
            next_res.to_string().contains("encoding 6"),
            "unexpected PinnedParquetExec TOCTOU err: {next_res}"
        );
    }

    #[tokio::test]
    async fn round19_zero_rle_def_levels_dict_oob_gzip_codec_dict_ipc_and_dual_schema_peak_reject_cleanly()
     {
        use arrow::array::FixedSizeBinaryArray;
        use arrow::datatypes::Field;
        use arrow::ipc::writer::{DictionaryTracker, IpcDataGenerator, IpcWriteOptions};
        use parquet::arrow::ArrowWriter;
        use parquet::basic::{Compression, Encoding, GzipLevel};
        use parquet::file::metadata::{KeyValue, ParquetMetaDataWriter};
        use parquet::file::properties::{EnabledStatistics, WriterProperties};

        let write_varint_u32 = |out: &mut Vec<u8>, mut v: u32| {
            loop {
                let mut byte = (v & 0x7f) as u8;
                v >>= 7;
                if v != 0 {
                    byte |= 0x80;
                }
                out.push(byte);
                if v == 0 {
                    break;
                }
            }
        };
        let encode_thrift_zigzag = |v: i32| -> u32 { ((v << 1) ^ (v >> 31)) as u32 };
        let craft_dict_page = |dict_num_vals: i32, payload: &[u8]| -> Vec<u8> {
            let mut hdr = Vec::new();
            hdr.extend_from_slice(&[0x15, encode_thrift_zigzag(2) as u8]);
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.extend_from_slice(&[0x4c, 0x15]);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(dict_num_vals));
            hdr.extend_from_slice(&[0x15, 0x00, 0x00, 0x00]);
            hdr.extend_from_slice(payload);
            hdr
        };
        let craft_v1_data_page = |num_vals: i32, val_enc: i32, payload: &[u8]| -> Vec<u8> {
            let mut hdr = Vec::new();
            hdr.extend_from_slice(&[0x15, 0x00]);
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.extend_from_slice(&[0x2c, 0x15]);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(num_vals));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(val_enc));
            hdr.extend_from_slice(&[0x15, 0x06, 0x15, 0x06, 0x00, 0x00]);
            hdr.extend_from_slice(payload);
            hdr
        };
        let rewrite_col_chunk = |base_path: &Path,
                                 col_idx: usize,
                                 dict_off_rel: Option<i64>,
                                 data_off_rel: i64,
                                 uncomp_total: i64,
                                 encs: Vec<Encoding>,
                                 chunk_bytes: &[u8]| {
            let orig = fs::read(base_path).unwrap();
            let orig_len = orig.len();
            let old_footer_len =
                u32::from_le_bytes(orig[orig_len - 8..orig_len - 4].try_into().unwrap()) as usize;
            let data_end = orig_len - 8 - old_footer_len;
            let meta =
                ParquetMetaDataReader::decode_metadata(&orig[data_end..orig_len - 8]).unwrap();
            let base_off = data_end as i64;
            let mut b = meta.into_builder();
            let mut rgs = b.take_row_groups();
            let mut rg_b = rgs.remove(0).into_builder();
            let mut cols = rg_b.take_columns();
            let col = cols
                .remove(col_idx)
                .into_builder()
                .set_encodings(encs)
                .set_compression(Compression::UNCOMPRESSED)
                .set_dictionary_page_offset(dict_off_rel.map(|r| base_off + r))
                .set_data_page_offset(base_off + data_off_rel)
                .set_total_compressed_size(chunk_bytes.len() as i64)
                .set_total_uncompressed_size(uncomp_total)
                .build()
                .unwrap();
            cols.insert(col_idx, col);
            rg_b = rg_b.set_column_metadata(cols);
            b = b.add_row_group(rg_b.build().unwrap());

            let mut out = orig[..data_end].to_vec();
            out.extend_from_slice(chunk_bytes);
            let mut f = fs::File::create(base_path).unwrap();
            std::io::Write::write_all(&mut f, &out).unwrap();
            ParquetMetaDataWriter::new(&mut f, &b.build())
                .finish()
                .unwrap();
        };

        let dir = non_tmp_dir();
        let catalog = fabric_store::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        fs::create_dir_all(&part_dir).unwrap();
        let clean_path = part_dir.join("part-clean-r19.parquet");
        let base_batch = metrics_to_record_batch(&[MetricPoint {
            timestamp_ms: 1_700_000_000_000_i64,
            name: "r19.base".into(),
            value: 1.0,
            host: "h".into(),
            slice: "spindle".into(),
            unit: "svc".into(),
            pid: Some(42),
            labels_json: "{}".into(),
        }])
        .unwrap();
        let props = WriterProperties::builder()
            .set_dictionary_enabled(false)
            .set_statistics_enabled(EnabledStatistics::None)
            .build();
        let mut w = ArrowWriter::try_new(
            fs::File::create(&clean_path).unwrap(),
            base_batch.schema(),
            Some(props),
        )
        .unwrap();
        w.write(&base_batch).unwrap();
        w.close().unwrap();
        let clean_bytes = fs::read(&clean_path).unwrap();
        fs::remove_file(&clean_path).unwrap();

        let pq_path = part_dir.join("part-r19-target.parquet");

        // (a) Zero-indicator RLE definition levels `[0x00, 0x00, 0x03]` must reject pre-decode
        // instead of letting PackedDecoder skip past `00 00` and execute the unvalidated `03` run.
        fs::write(&pq_path, &clean_bytes).unwrap();
        let mut zero_ind_payload = vec![3_u8, 0, 0, 0, 0x00, 0x00, 0x03];
        zero_ind_payload.extend_from_slice(&42_i32.to_le_bytes());
        let zero_ind_page = craft_v1_data_page(1, 0, &zero_ind_payload);
        rewrite_col_chunk(
            &pq_path,
            6,
            None,
            0,
            zero_ind_payload.len() as i64,
            vec![Encoding::PLAIN],
            &zero_ind_page,
        );
        let store_zero_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&pq_path).unwrap(),
                Some(&METRICS_SCHEMA),
                128,
            )
            .expect_err("zero RLE indicator in definition levels must reject pre-decode");
        assert!(
            store_zero_err.contains("zero RLE indicator"),
            "unexpected store zero-RLE err: {store_zero_err}"
        );
        let ctx_zero = SessionContext::new();
        register_pinned_parquet_table(
            &ctx_zero,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let q_zero_err = ctx_zero
            .sql("SELECT pid FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("zero RLE indicator must reject in PinnedParquetExec");
        assert!(
            q_zero_err.to_string().contains("zero RLE indicator"),
            "unexpected query zero-RLE err: {q_zero_err}"
        );

        // (b) FIXED_LEN_BYTE_ARRAY(4) out-of-bounds RLE_DICTIONARY key >= dict_vals (both RLE & bit-packed).
        let fsb_dir = non_tmp_dir();
        let fsb_pq = fsb_dir.path().join("fsb-dict.parquet");
        let fsb_schema = Arc::new(Schema::new(vec![Field::new(
            "tag",
            DataType::FixedSizeBinary(4),
            false,
        )]));
        let fsb_arr = FixedSizeBinaryArray::try_new(
            4,
            arrow::buffer::Buffer::from_slice_ref([1_u8, 2, 3, 4]),
            None,
        )
        .unwrap();
        let fsb_batch =
            RecordBatch::try_new(Arc::clone(&fsb_schema), vec![Arc::new(fsb_arr)]).unwrap();
        let fsb_props = WriterProperties::builder()
            .set_dictionary_enabled(false)
            .set_statistics_enabled(EnabledStatistics::None)
            .build();
        let mut fw = ArrowWriter::try_new(
            fs::File::create(&fsb_pq).unwrap(),
            fsb_schema,
            Some(fsb_props),
        )
        .unwrap();
        fw.write(&fsb_batch).unwrap();
        fw.close().unwrap();
        let fsb_clean = fs::read(&fsb_pq).unwrap();
        let fsb_dict = craft_dict_page(1, &[1_u8, 2, 3, 4]);
        for (tag, bad_keys) in [
            ("rle_oob", vec![1_u8, 0x02, 0x01]),
            ("packed_oob", vec![1_u8, 0x03, 0x01]),
        ] {
            fs::write(&fsb_pq, &fsb_clean).unwrap();
            let dpage = craft_v1_data_page(1, 8, &bad_keys);
            let mut chunk = fsb_dict.clone();
            chunk.extend_from_slice(&dpage);
            rewrite_col_chunk(
                &fsb_pq,
                0,
                Some(0),
                fsb_dict.len() as i64,
                (4 + bad_keys.len()) as i64,
                vec![Encoding::PLAIN, Encoding::RLE_DICTIONARY],
                &chunk,
            );
            let oob_err =
                fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                    fs::File::open(&fsb_pq).unwrap(),
                    None,
                    128,
                )
                .expect_err("OOB FIXED_LEN_BYTE_ARRAY dictionary key must reject pre-decode");
            assert!(
                oob_err.contains("exceeds maximum allowed"),
                "{tag}: unexpected OOB dict err: {oob_err}"
            );
        }

        // (c) Non-Bool ARROW:schema DataType::Dictionary (Dictionary(Int8, Int64) and Dictionary(Int8, FixedSizeBinary(4))).
        let many_pts: Vec<MetricPoint> = (0..200_i64)
            .map(|i| MetricPoint {
                timestamp_ms: 1_700_000_000_000_i64 + i,
                name: "dict.key.overflow".into(),
                value: i as f64,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: None,
                labels_json: "{}".into(),
            })
            .collect();
        let many_batch = metrics_to_record_batch(&many_pts).unwrap();
        let many_props = WriterProperties::builder()
            .set_dictionary_enabled(false)
            .set_statistics_enabled(EnabledStatistics::None)
            .build();
        let mut mw = ArrowWriter::try_new(
            fs::File::create(&pq_path).unwrap(),
            many_batch.schema(),
            Some(many_props),
        )
        .unwrap();
        mw.write(&many_batch).unwrap();
        mw.close().unwrap();

        let encode_arrow_schema_b64 = |schema: &Schema| -> String {
            let ipc_opts = IpcWriteOptions::default();
            let mut dict_tracker = DictionaryTracker::new(true);
            let enc_msg = IpcDataGenerator::default().schema_to_bytes_with_dictionary_tracker(
                schema,
                &mut dict_tracker,
                &ipc_opts,
            );
            let mut ipc_prefixed = vec![255_u8; 4];
            ipc_prefixed.extend_from_slice(&(enc_msg.ipc_message.len() as u32).to_le_bytes());
            ipc_prefixed.extend_from_slice(&enc_msg.ipc_message);
            test_encode_base64(&ipc_prefixed)
        };
        let mut dict_i64_fields = METRICS_SCHEMA.fields().to_vec();
        dict_i64_fields[0] = Arc::new(Field::new(
            "timestamp_ms",
            DataType::Dictionary(Box::new(DataType::Int8), Box::new(DataType::Int64)),
            false,
        ));
        let dict_i64_b64 = encode_arrow_schema_b64(&Schema::new(dict_i64_fields));
        rewrite_parquet_footer_with_meta(&pq_path, |meta| {
            let fm = meta.file_metadata();
            let new_fm = parquet::file::metadata::FileMetaData::new(
                fm.version(),
                fm.num_rows(),
                fm.created_by().map(str::to_owned),
                Some(vec![KeyValue::new(
                    "ARROW:schema".to_owned(),
                    dict_i64_b64.clone(),
                )]),
                fm.schema_descr_ptr(),
                fm.column_orders().map(|o| o.to_vec()),
            );
            parquet::file::metadata::ParquetMetaData::new(new_fm, meta.row_groups().to_vec())
        });
        let dict_i64_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&pq_path).unwrap(),
                Some(&METRICS_SCHEMA),
                128,
            )
            .expect_err("Dictionary(Int8, Int64) ARROW:schema must reject pre-decode");
        assert!(
            dict_i64_err.contains("dictionary-typed ARROW:schema"),
            "unexpected Dictionary(Int8, Int64) err: {dict_i64_err}"
        );
        let ctx_dict = SessionContext::new();
        register_pinned_parquet_table(
            &ctx_dict,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let q_dict_err = ctx_dict
            .sql("SELECT timestamp_ms FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("Dictionary(Int8, Int64) must reject in PinnedParquetExec");
        assert!(
            q_dict_err
                .to_string()
                .contains("dictionary-typed ARROW:schema"),
            "unexpected query Dictionary(Int8, Int64) err: {q_dict_err}"
        );

        fs::write(&fsb_pq, &fsb_clean).unwrap();
        let dict_fsb_b64 = encode_arrow_schema_b64(&Schema::new(vec![Field::new(
            "tag",
            DataType::Dictionary(
                Box::new(DataType::Int8),
                Box::new(DataType::FixedSizeBinary(4)),
            ),
            false,
        )]));
        rewrite_parquet_footer_with_meta(&fsb_pq, |meta| {
            let fm = meta.file_metadata();
            let new_fm = parquet::file::metadata::FileMetaData::new(
                fm.version(),
                fm.num_rows(),
                fm.created_by().map(str::to_owned),
                Some(vec![KeyValue::new(
                    "ARROW:schema".to_owned(),
                    dict_fsb_b64.clone(),
                )]),
                fm.schema_descr_ptr(),
                fm.column_orders().map(|o| o.to_vec()),
            );
            parquet::file::metadata::ParquetMetaData::new(new_fm, meta.row_groups().to_vec())
        });
        let dict_fsb_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&fsb_pq).unwrap(),
                None,
                128,
            )
            .expect_err("Dictionary(Int8, FixedSizeBinary(4)) must reject pre-decode");
        assert!(
            dict_fsb_err.contains("dictionary-typed ARROW:schema"),
            "unexpected Dictionary(Int8, FixedSizeBinary) err: {dict_fsb_err}"
        );

        // (d) Unbounded streaming codec (Compression::GZIP) must reject pre-decode.
        let gzip_props = WriterProperties::builder()
            .set_compression(Compression::GZIP(GzipLevel::default()))
            .build();
        let mut gw = ArrowWriter::try_new(
            fs::File::create(&pq_path).unwrap(),
            base_batch.schema(),
            Some(gzip_props),
        )
        .unwrap();
        gw.write(&base_batch).unwrap();
        gw.close().unwrap();
        let gzip_store_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&pq_path).unwrap(),
                Some(&METRICS_SCHEMA),
                128,
            )
            .expect_err("GZIP-compressed Parquet must reject unbounded streaming codecs");
        assert!(
            gzip_store_err.contains("unsupported compression codec"),
            "unexpected GZIP store err: {gzip_store_err}"
        );
        let ctx_gzip = SessionContext::new();
        register_pinned_parquet_table(
            &ctx_gzip,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let gzip_query_err = ctx_gzip
            .sql("SELECT name FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("GZIP-compressed Parquet must reject in PinnedParquetExec");
        assert!(
            gzip_query_err
                .to_string()
                .contains("unsupported compression codec"),
            "unexpected GZIP query err: {gzip_query_err}"
        );

        // (e) Dual-expanded ARROW:schema peak (~1.2 MiB single copy <= 2 MiB, ~2.4 MiB dual peak inside
        // ArrowReaderMetadata::try_new) via shared field custom_metadata KeyValue DAG must reject under a
        // 2 MiB pool (above single-copy peak) and succeed under a 4 MiB pool.
        let craft_shared_metrics_field_meta_b64 = |val_len: usize| -> String {
            let align4 = |buf: &mut Vec<u8>| {
                while !buf.len().is_multiple_of(4) {
                    buf.push(0);
                }
            };
            let patch_u32 = |buf: &mut Vec<u8>, slot: usize, target: usize| {
                let rel = u32::try_from(target - slot).unwrap();
                buf[slot..slot + 4].copy_from_slice(&rel.to_le_bytes());
            };
            let mut fb = Vec::with_capacity(val_len + 4096);
            fb.extend_from_slice(&0_u32.to_le_bytes()); // root uoffset -> Message @ 0 patched later
            let msg_vt = fb.len();
            for v in [10_u16, 12, 0, 4, 8, 0] {
                fb.extend_from_slice(&v.to_le_bytes());
            }
            let msg_tbl = fb.len();
            patch_u32(&mut fb, 0, msg_tbl);
            fb.extend_from_slice(&i32::try_from(msg_tbl - msg_vt).unwrap().to_le_bytes());
            fb.extend_from_slice(&[1_u8, 0, 0, 0]); // header_type = Schema
            let schema_slot = fb.len();
            fb.extend_from_slice(&0_u32.to_le_bytes());

            let schema_vt = fb.len();
            for v in [8_u16, 12, 4, 8] {
                fb.extend_from_slice(&v.to_le_bytes());
            }
            let schema_tbl = fb.len();
            patch_u32(&mut fb, schema_slot, schema_tbl);
            fb.extend_from_slice(&i32::try_from(schema_tbl - schema_vt).unwrap().to_le_bytes());
            fb.extend_from_slice(&[0_u8, 0, 0, 0]); // endianness = Little (0) + pad
            let fields_vec_slot = fb.len();
            fb.extend_from_slice(&0_u32.to_le_bytes());

            let fields_vec = fb.len();
            patch_u32(&mut fb, fields_vec_slot, fields_vec);
            fb.extend_from_slice(&8_u32.to_le_bytes());
            let mut field_elem_slots = Vec::with_capacity(8);
            for _ in 0..8 {
                field_elem_slots.push(fb.len());
                fb.extend_from_slice(&0_u32.to_le_bytes());
            }

            let field_vt = fb.len();
            for v in [18_u16, 20, 4, 8, 9, 12, 0, 0, 16] {
                fb.extend_from_slice(&v.to_le_bytes());
            }
            align4(&mut fb);
            let field_specs: [(&str, u8, u8, u8); 8] = [
                ("timestamp_ms", 0, 2, 0), // type Int64
                ("name", 0, 5, 1),         // type Utf8
                ("value", 0, 3, 2),        // type Float64
                ("host", 0, 5, 1),         // type Utf8
                ("slice", 0, 5, 1),        // type Utf8
                ("unit", 0, 5, 1),         // type Utf8
                ("pid", 1, 2, 3),          // type UInt32
                ("labels_json", 0, 5, 1),  // type Utf8
            ];
            let mut name_slots = Vec::with_capacity(8);
            let mut type_slots = Vec::with_capacity(8);
            let mut cm_slots = Vec::with_capacity(8);
            for (idx, (_, nullable, type_type, type_kind)) in field_specs.iter().enumerate() {
                let ft = fb.len();
                patch_u32(&mut fb, field_elem_slots[idx], ft);
                fb.extend_from_slice(&i32::try_from(ft - field_vt).unwrap().to_le_bytes());
                name_slots.push(fb.len());
                fb.extend_from_slice(&0_u32.to_le_bytes());
                fb.extend_from_slice(&[*nullable, *type_type, 0, 0]);
                type_slots.push((fb.len(), *type_kind));
                fb.extend_from_slice(&0_u32.to_le_bytes());
                cm_slots.push(fb.len());
                fb.extend_from_slice(&0_u32.to_le_bytes());
            }

            for (idx, (name, _, _, _)) in field_specs.iter().enumerate() {
                align4(&mut fb);
                let s_off = fb.len();
                patch_u32(&mut fb, name_slots[idx], s_off);
                fb.extend_from_slice(&(name.len() as u32).to_le_bytes());
                fb.extend_from_slice(name.as_bytes());
                fb.push(0);
            }
            align4(&mut fb);

            // Shared type tables: 0=Int64, 1=Utf8, 2=Float64, 3=UInt32.
            let int_vt = fb.len();
            for v in [8_u16, 12, 4, 8] {
                fb.extend_from_slice(&v.to_le_bytes());
            }
            let int64_tbl = fb.len();
            fb.extend_from_slice(&i32::try_from(int64_tbl - int_vt).unwrap().to_le_bytes());
            fb.extend_from_slice(&64_i32.to_le_bytes());
            fb.extend_from_slice(&[1_u8, 0, 0, 0]);

            let uint32_tbl = fb.len();
            fb.extend_from_slice(&i32::try_from(uint32_tbl - int_vt).unwrap().to_le_bytes());
            fb.extend_from_slice(&32_i32.to_le_bytes());
            fb.extend_from_slice(&[0_u8, 0, 0, 0]);

            let utf8_vt = fb.len();
            for v in [4_u16, 4] {
                fb.extend_from_slice(&v.to_le_bytes());
            }
            let utf8_tbl = fb.len();
            fb.extend_from_slice(&i32::try_from(utf8_tbl - utf8_vt).unwrap().to_le_bytes());

            let fp_vt = fb.len();
            for v in [6_u16, 8, 4] {
                fb.extend_from_slice(&v.to_le_bytes());
            }
            align4(&mut fb);
            let fp_tbl = fb.len();
            fb.extend_from_slice(&i32::try_from(fp_tbl - fp_vt).unwrap().to_le_bytes());
            fb.extend_from_slice(&2_i16.to_le_bytes()); // Precision::DOUBLE
            fb.extend_from_slice(&[0_u8, 0]);

            for (slot, kind) in type_slots {
                let tgt = match kind {
                    0 => int64_tbl,
                    1 => utf8_tbl,
                    2 => fp_tbl,
                    _ => uint32_tbl,
                };
                patch_u32(&mut fb, slot, tgt);
            }

            let mut kv_elem_slots = Vec::with_capacity(8);
            for slot in cm_slots {
                let vec_off = fb.len();
                patch_u32(&mut fb, slot, vec_off);
                fb.extend_from_slice(&1_u32.to_le_bytes());
                kv_elem_slots.push(fb.len());
                fb.extend_from_slice(&0_u32.to_le_bytes());
            }

            let kv_vt = fb.len();
            for v in [8_u16, 12, 4, 8] {
                fb.extend_from_slice(&v.to_le_bytes());
            }
            let kv_tbl = fb.len();
            for elem_slot in kv_elem_slots {
                patch_u32(&mut fb, elem_slot, kv_tbl);
            }
            fb.extend_from_slice(&i32::try_from(kv_tbl - kv_vt).unwrap().to_le_bytes());
            let key_slot = fb.len();
            fb.extend_from_slice(&0_u32.to_le_bytes());
            let val_slot = fb.len();
            fb.extend_from_slice(&0_u32.to_le_bytes());

            let k_off = fb.len();
            patch_u32(&mut fb, key_slot, k_off);
            fb.extend_from_slice(&3_u32.to_le_bytes());
            fb.extend_from_slice(b"doc\0");
            align4(&mut fb);

            let v_off = fb.len();
            patch_u32(&mut fb, val_slot, v_off);
            fb.extend_from_slice(&(val_len as u32).to_le_bytes());
            fb.resize(fb.len() + val_len, b'x');
            fb.push(0);
            align4(&mut fb);

            assert!(arrow::ipc::root_as_message(&fb).is_ok());
            let mut ipc_prefixed = vec![255_u8; 4];
            ipc_prefixed.extend_from_slice(&(fb.len() as u32).to_le_bytes());
            ipc_prefixed.extend_from_slice(&fb);
            test_encode_base64(&ipc_prefixed)
        };

        fs::write(&pq_path, &clean_bytes).unwrap();
        let dual_b64 = craft_shared_metrics_field_meta_b64(150 * 1024);
        assert!(dual_b64.len() < 512 * 1024);
        rewrite_parquet_footer_with_meta(&pq_path, |meta| {
            let fm = meta.file_metadata();
            let new_fm = parquet::file::metadata::FileMetaData::new(
                fm.version(),
                fm.num_rows(),
                fm.created_by().map(str::to_owned),
                Some(vec![KeyValue::new(
                    "ARROW:schema".to_owned(),
                    dual_b64.clone(),
                )]),
                fm.schema_descr_ptr(),
                fm.column_orders().map(|o| o.to_vec()),
            );
            parquet::file::metadata::ParquetMetaData::new(new_fm, meta.row_groups().to_vec())
        });

        // Single-copy expanded schema is ~1.2 MiB (plus ~380 KiB metadata/base64 < 2 MiB),
        // while dual-copy try_new peak is ~2.8 MiB (> 2 MiB pool) and must reject cleanly pre-try_new.
        let dual_tight_env = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::new(GreedyMemoryPool::new(2048 * 1024)))
            .build_arc()
            .unwrap();
        let dual_tight_ctx = SessionContext::new_with_config_rt(
            SessionConfig::new().with_target_partitions(1),
            dual_tight_env,
        );
        register_pinned_parquet_table(
            &dual_tight_ctx,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let dual_err = dual_tight_ctx
            .sql("SELECT name FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("2 MiB pool must reject ~2.8 MiB dual-expanded ARROW:schema peak");
        assert!(
            dual_err.to_string().contains("Resources exhausted"),
            "unexpected dual-schema tight pool err: {dual_err}"
        );

        let dual_ok_env = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::new(GreedyMemoryPool::new(4 * 1024 * 1024)))
            .build_arc()
            .unwrap();
        let dual_ok_ctx = SessionContext::new_with_config_rt(
            SessionConfig::new().with_target_partitions(1),
            dual_ok_env,
        );
        register_pinned_parquet_table(
            &dual_ok_ctx,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let ok_batches = dual_ok_ctx
            .sql("SELECT name FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        assert_eq!(
            ok_batches.iter().map(RecordBatch::num_rows).sum::<usize>(),
            1
        );
    }

    #[tokio::test]
    async fn round20_zero_width_fsb_and_surplus_packed_rle_and_v2_boundaries_reject_cleanly() {
        use arrow::array::FixedSizeBinaryArray;
        use arrow::datatypes::Field;
        use arrow::ipc::writer::{DictionaryTracker, IpcDataGenerator, IpcWriteOptions};
        use parquet::arrow::ArrowWriter;
        use parquet::basic::{Compression, Encoding, Repetition, Type as PhysicalType, ZstdLevel};
        use parquet::file::metadata::{
            ColumnChunkMetaData, FileMetaData, KeyValue, ParquetMetaDataBuilder,
            ParquetMetaDataWriter, RowGroupMetaData,
        };
        use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
        use parquet::schema::types::{SchemaDescriptor, Type as SchemaType};

        let write_varint_u32 = |out: &mut Vec<u8>, mut v: u32| {
            loop {
                let mut byte = (v & 0x7f) as u8;
                v >>= 7;
                if v != 0 {
                    byte |= 0x80;
                }
                out.push(byte);
                if v == 0 {
                    break;
                }
            }
        };
        let encode_thrift_zigzag = |v: i32| -> u32 { ((v << 1) ^ (v >> 31)) as u32 };
        let craft_dict_page = |dict_num_vals: i32, payload: &[u8]| -> Vec<u8> {
            let mut hdr = Vec::new();
            hdr.extend_from_slice(&[0x15, encode_thrift_zigzag(2) as u8]);
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.extend_from_slice(&[0x4c, 0x15]);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(dict_num_vals));
            hdr.extend_from_slice(&[0x15, 0x00, 0x00, 0x00]);
            hdr.extend_from_slice(payload);
            hdr
        };
        let craft_v1_data_page = |num_vals: i32, val_enc: i32, payload: &[u8]| -> Vec<u8> {
            let mut hdr = Vec::new();
            hdr.extend_from_slice(&[0x15, 0x00]);
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.extend_from_slice(&[0x2c, 0x15]);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(num_vals));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(val_enc));
            hdr.extend_from_slice(&[0x15, 0x06, 0x15, 0x06, 0x00, 0x00]);
            hdr.extend_from_slice(payload);
            hdr
        };
        let craft_v2_data_page = |num_vals: i32,
                                  num_nulls: i32,
                                  def_len: i32,
                                  val_enc: i32,
                                  payload: &[u8]|
         -> Vec<u8> {
            // PageHeader: type=3 (DATA_PAGE_V2, zig 6), uncomp=comp=payload.len(), DataPageHeaderV2(field 8 -> 0x3c)
            let mut hdr = Vec::new();
            hdr.extend_from_slice(&[0x15, encode_thrift_zigzag(3) as u8]);
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.extend_from_slice(&[0x5c, 0x15]); // fid delta +5 (from fid 3 -> fid 8 struct = 0x5c)
            write_varint_u32(&mut hdr, encode_thrift_zigzag(num_vals));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(num_nulls));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(num_vals));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(val_enc));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(def_len));
            hdr.extend_from_slice(&[0x15, 0x00]); // rep_levels_byte_length = 0
            hdr.extend_from_slice(&[0x12, 0x00, 0x00]); // is_compressed = false, stop v2 struct, stop PageHeader
            hdr.extend_from_slice(payload);
            hdr
        };

        let dir = non_tmp_dir();
        let catalog = fabric_store::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        fs::create_dir_all(&part_dir).unwrap();

        // 1. Unannotated 1-row FIXED_LEN_BYTE_ARRAY(0) PLAIN Parquet must reject cleanly pre-decode
        //    (avoiding FixedLenByteArrayReader `/ byte_length` division-by-zero panic).
        let zero_fsb_pq = dir.path().join("zero-width-fsb.parquet");
        let zero_col_ty = Arc::new(
            SchemaType::primitive_type_builder("tag", PhysicalType::FIXED_LEN_BYTE_ARRAY)
                .with_length(0)
                .with_repetition(Repetition::REQUIRED)
                .build()
                .unwrap(),
        );
        let root_zero = Arc::new(
            SchemaType::group_type_builder("schema")
                .with_fields(vec![zero_col_ty])
                .build()
                .unwrap(),
        );
        let zero_schema = Arc::new(SchemaDescriptor::new(root_zero));
        let empty_plain_page = craft_v1_data_page(1, 0, &[]);
        let zero_col_meta = ColumnChunkMetaData::builder(zero_schema.column(0))
            .set_encodings(vec![Encoding::PLAIN])
            .set_compression(Compression::UNCOMPRESSED)
            .set_num_values(1)
            .set_data_page_offset(4)
            .set_total_compressed_size(empty_plain_page.len() as i64)
            .set_total_uncompressed_size(0)
            .build()
            .unwrap();
        let zero_rg_meta = RowGroupMetaData::builder(Arc::clone(&zero_schema))
            .set_num_rows(1)
            .set_total_byte_size(empty_plain_page.len() as i64)
            .set_column_metadata(vec![zero_col_meta])
            .build()
            .unwrap();
        let zero_fmd = FileMetaData::new(1, 1, None, None, zero_schema, None);
        let zero_pq_meta = ParquetMetaDataBuilder::new(zero_fmd)
            .set_row_groups(vec![zero_rg_meta])
            .build();
        {
            use std::io::Write;
            let mut f = fs::File::create(&zero_fsb_pq).unwrap();
            f.write_all(b"PAR1").unwrap();
            f.write_all(&empty_plain_page).unwrap();
            ParquetMetaDataWriter::new(&mut f, &zero_pq_meta)
                .finish()
                .unwrap();
        }
        let zero_file_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&zero_fsb_pq).unwrap(),
                None,
                128,
            )
            .expect_err("FIXED_LEN_BYTE_ARRAY(0) PLAIN must reject pre-decode from file");
        assert!(
            zero_file_err.contains("FIXED_LEN_BYTE_ARRAY length 0"),
            "unexpected zero-width fsb file err: {zero_file_err}"
        );
        let zf = fs::File::open(&zero_fsb_pq).unwrap();
        let zlen = usize::try_from(zf.metadata().unwrap().len()).unwrap();
        let zbytes =
            <fs::File as parquet::file::reader::ChunkReader>::get_bytes(&zf, 0, zlen).unwrap();
        let zero_bytes_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_bytes(
                zbytes, None, 128,
            )
            .expect_err("FIXED_LEN_BYTE_ARRAY(0) PLAIN must reject pre-decode from bytes");
        assert!(
            zero_bytes_err.contains("FIXED_LEN_BYTE_ARRAY length 0"),
            "unexpected zero-width fsb bytes err: {zero_bytes_err}"
        );

        // 2. ARROW:schema FixedSizeBinary(0) metadata must reject in validate_embedded_arrow_schema_ipc.
        let fsb_pq = dir.path().join("fsb-bytewidth0.parquet");
        let fsb_schema = Arc::new(Schema::new(vec![Field::new(
            "tag",
            DataType::FixedSizeBinary(4),
            false,
        )]));
        let fsb_arr = FixedSizeBinaryArray::try_new(
            4,
            arrow::buffer::Buffer::from_slice_ref([1_u8, 2, 3, 4]),
            None,
        )
        .unwrap();
        let fsb_batch =
            RecordBatch::try_new(Arc::clone(&fsb_schema), vec![Arc::new(fsb_arr)]).unwrap();
        let mut fw =
            ArrowWriter::try_new(fs::File::create(&fsb_pq).unwrap(), fsb_schema, None).unwrap();
        fw.write(&fsb_batch).unwrap();
        fw.close().unwrap();
        let ipc_opts = IpcWriteOptions::default();
        let mut dt = DictionaryTracker::new(true);
        let msg0 = IpcDataGenerator::default().schema_to_bytes_with_dictionary_tracker(
            &Schema::new(vec![Field::new("tag", DataType::FixedSizeBinary(0), false)]),
            &mut dt,
            &ipc_opts,
        );
        let mut ipc_prefixed = vec![255_u8; 4];
        ipc_prefixed.extend_from_slice(&(msg0.ipc_message.len() as u32).to_le_bytes());
        ipc_prefixed.extend_from_slice(&msg0.ipc_message);
        let bw0_b64 = test_encode_base64(&ipc_prefixed);
        rewrite_parquet_footer_with_meta(&fsb_pq, |meta| {
            let fm = meta.file_metadata();
            let new_fm = FileMetaData::new(
                fm.version(),
                fm.num_rows(),
                fm.created_by().map(str::to_owned),
                Some(vec![KeyValue::new(
                    "ARROW:schema".to_owned(),
                    bw0_b64.clone(),
                )]),
                fm.schema_descr_ptr(),
                fm.column_orders().map(|o| o.to_vec()),
            );
            parquet::file::metadata::ParquetMetaData::new(new_fm, meta.row_groups().to_vec())
        });
        let bw0_err = fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
            fs::File::open(&fsb_pq).unwrap(),
            None,
            128,
        )
        .expect_err("FixedSizeBinary(0) ARROW:schema must reject pre-decode");
        assert!(
            bw0_err.contains("FixedSizeBinary byteWidth 0"),
            "unexpected bw0 err: {bw0_err}"
        );

        // 3. Oversized 1 MiB bit-packed RLE dictionary run (8,388,608 values for 1-row page) must
        //    reject immediately in O(1) before per-value scanning across store & query.
        let base_pts = vec![
            MetricPoint {
                timestamp_ms: 1_700_000_000_000_i64,
                name: "r20.v2".into(),
                value: 1.5,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: Some(7),
                labels_json: "{}".into(),
            },
            MetricPoint {
                timestamp_ms: 1_700_000_000_001_i64,
                name: "r20.v2.null".into(),
                value: 2.5,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: None,
                labels_json: "{}".into(),
            },
        ];
        let one_row_batch = metrics_to_record_batch(&base_pts[..1]).unwrap();
        let pq_path = part_dir.join("part-r20-attack.parquet");
        let props_v1 = WriterProperties::builder()
            .set_dictionary_enabled(false)
            .set_statistics_enabled(EnabledStatistics::None)
            .build();
        let mut w1 = ArrowWriter::try_new(
            fs::File::create(&pq_path).unwrap(),
            one_row_batch.schema(),
            Some(props_v1),
        )
        .unwrap();
        w1.write(&one_row_batch).unwrap();
        w1.close().unwrap();
        let one_row_clean = fs::read(&pq_path).unwrap();

        let mut name_dict_payload = 1_u32.to_le_bytes().to_vec();
        name_dict_payload.push(b'm');
        let dict_part = craft_dict_page(1, &name_dict_payload);
        let mut huge_packed_val = vec![1_u8]; // bw = 1
        test_push_vlq(&mut huge_packed_val, (1_048_576_u64 << 1) | 1); // 8,388,608 packed keys
        huge_packed_val.resize(huge_packed_val.len() + 1_048_576, 0);
        let huge_dpage = craft_v1_data_page(1, 8, &huge_packed_val);
        let mut huge_chunk = dict_part.clone();
        huge_chunk.extend_from_slice(&huge_dpage);
        {
            let orig_len = one_row_clean.len();
            let old_footer_len = u32::from_le_bytes(
                one_row_clean[orig_len - 8..orig_len - 4]
                    .try_into()
                    .unwrap(),
            ) as usize;
            let data_end = orig_len - 8 - old_footer_len;
            let meta =
                ParquetMetaDataReader::decode_metadata(&one_row_clean[data_end..orig_len - 8])
                    .unwrap();
            let base_off = data_end as i64;
            let mut b = meta.into_builder();
            let mut rgs = b.take_row_groups();
            let mut rg_b = rgs.remove(0).into_builder();
            let mut cols = rg_b.take_columns();
            let col1 = cols
                .remove(1)
                .into_builder()
                .set_encodings(vec![Encoding::PLAIN, Encoding::RLE_DICTIONARY])
                .set_compression(Compression::UNCOMPRESSED)
                .set_dictionary_page_offset(Some(base_off))
                .set_data_page_offset(base_off + dict_part.len() as i64)
                .set_total_compressed_size(huge_chunk.len() as i64)
                .set_total_uncompressed_size(
                    (name_dict_payload.len() + huge_packed_val.len()) as i64,
                )
                .build()
                .unwrap();
            cols.insert(1, col1);
            rg_b = rg_b.set_column_metadata(cols);
            b = b.add_row_group(rg_b.build().unwrap());
            let mut out = one_row_clean[..data_end].to_vec();
            out.extend_from_slice(&huge_chunk);
            let mut f = fs::File::create(&pq_path).unwrap();
            std::io::Write::write_all(&mut f, &out).unwrap();
            ParquetMetaDataWriter::new(&mut f, &b.build())
                .finish()
                .unwrap();
        }
        let surplus_store_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&pq_path).unwrap(),
                Some(&METRICS_SCHEMA),
                128,
            )
            .expect_err("surplus 1 MiB bit-packed RLE run on 1-row page must reject in O(1)");
        assert!(
            surplus_store_err.contains("surplus bit-packed value run"),
            "unexpected surplus store err: {surplus_store_err}"
        );
        let ctx_surplus = SessionContext::new();
        register_pinned_parquet_table(
            &ctx_surplus,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let surplus_q_err = ctx_surplus
            .sql("SELECT name FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("surplus bit-packed RLE run must reject in PinnedParquetExec");
        assert!(
            surplus_q_err
                .to_string()
                .contains("surplus bit-packed value run"),
            "unexpected surplus query err: {surplus_q_err}"
        );

        // 4. DataPageV2 boundaries: valid ZSTD PARQUET_2_0 file succeeds cleanly, while V2
        //    zero-indicator def levels and non-zero required def levels/nulls reject cleanly.
        let v2_batch = metrics_to_record_batch(&base_pts).unwrap();
        let v2_props = WriterProperties::builder()
            .set_writer_version(WriterVersion::PARQUET_2_0)
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(1).unwrap()))
            .set_statistics_enabled(EnabledStatistics::None)
            .build();
        let mut w2 = ArrowWriter::try_new(
            fs::File::create(&pq_path).unwrap(),
            v2_batch.schema(),
            Some(v2_props),
        )
        .unwrap();
        w2.write(&v2_batch).unwrap();
        w2.close().unwrap();
        let v2_clean = fs::read(&pq_path).unwrap();

        let v2_ok_reader =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&pq_path).unwrap(),
                Some(&METRICS_SCHEMA),
                128,
            )
            .unwrap();
        let v2_ok_rows: usize = v2_ok_reader.map(|r| r.unwrap().num_rows()).sum();
        assert_eq!(v2_ok_rows, 2);

        let rewrite_v2_col = |target: &Path,
                              col_idx: usize,
                              encs: Vec<Encoding>,
                              v2_page: &[u8],
                              uncomp_len: i64| {
            let orig_len = v2_clean.len();
            let old_footer_len =
                u32::from_le_bytes(v2_clean[orig_len - 8..orig_len - 4].try_into().unwrap())
                    as usize;
            let data_end = orig_len - 8 - old_footer_len;
            let meta =
                ParquetMetaDataReader::decode_metadata(&v2_clean[data_end..orig_len - 8]).unwrap();
            let base_off = data_end as i64;
            let mut b = meta.into_builder();
            let mut rgs = b.take_row_groups();
            let mut rg_b = rgs.remove(0).into_builder();
            let mut cols = rg_b.take_columns();
            let col = cols
                .remove(col_idx)
                .into_builder()
                .set_encodings(encs)
                .set_compression(Compression::UNCOMPRESSED)
                .set_dictionary_page_offset(None)
                .set_data_page_offset(base_off)
                .set_total_compressed_size(v2_page.len() as i64)
                .set_total_uncompressed_size(uncomp_len)
                .build()
                .unwrap();
            cols.insert(col_idx, col);
            rg_b = rg_b.set_column_metadata(cols);
            b = b.add_row_group(rg_b.build().unwrap());
            let mut out = v2_clean[..data_end].to_vec();
            out.extend_from_slice(v2_page);
            let mut f = fs::File::create(target).unwrap();
            std::io::Write::write_all(&mut f, &out).unwrap();
            ParquetMetaDataWriter::new(&mut f, &b.build())
                .finish()
                .unwrap();
        };

        // 4a. V2 zero-indicator def levels on optional `pid` (col 6).
        let mut v2_zero_payload = vec![0x00_u8, 0x00, 0x03];
        v2_zero_payload.extend_from_slice(&7_i32.to_le_bytes());
        let v2_zero_page = craft_v2_data_page(2, 1, 3, 0, &v2_zero_payload);
        rewrite_v2_col(
            &pq_path,
            6,
            vec![Encoding::PLAIN],
            &v2_zero_page,
            v2_zero_payload.len() as i64,
        );
        let v2_zero_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&pq_path).unwrap(),
                Some(&METRICS_SCHEMA),
                128,
            )
            .expect_err("V2 zero RLE def indicator must reject pre-decode");
        assert!(
            v2_zero_err.contains("zero RLE indicator"),
            "unexpected V2 zero-RLE err: {v2_zero_err}"
        );

        // 4b. V2 required column `name` (col 1) claiming num_nulls = 1 must reject cleanly.
        let v2_req_null_page = craft_v2_data_page(2, 1, 0, 0, &[]);
        rewrite_v2_col(&pq_path, 1, vec![Encoding::PLAIN], &v2_req_null_page, 0);
        let v2_req_err = fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
            fs::File::open(&pq_path).unwrap(),
            Some(&METRICS_SCHEMA),
            128,
        )
        .expect_err("V2 required column claiming non-zero nulls must reject pre-decode");
        assert!(
            v2_req_err.contains("non-zero definition levels or nulls"),
            "unexpected V2 required-null err: {v2_req_err}"
        );
    }

    #[tokio::test]
    async fn round21_extraneous_v2_header_and_oversized_fixed_width_reject_cleanly() {
        use parquet::arrow::ArrowWriter;
        use parquet::basic::{Compression, Encoding, Repetition, Type as PhysicalType};
        use parquet::file::metadata::{
            ColumnChunkMetaData, FileMetaData, ParquetMetaDataBuilder, ParquetMetaDataWriter,
            RowGroupMetaData,
        };
        use parquet::file::properties::{EnabledStatistics, WriterProperties};
        use parquet::schema::types::{SchemaDescriptor, Type as SchemaType};

        let write_varint_u32 = |out: &mut Vec<u8>, mut v: u32| {
            loop {
                let mut byte = (v & 0x7f) as u8;
                v >>= 7;
                if v != 0 {
                    byte |= 0x80;
                }
                out.push(byte);
                if v == 0 {
                    break;
                }
            }
        };
        let encode_thrift_zigzag = |v: i32| -> u32 { ((v << 1) ^ (v >> 31)) as u32 };

        let dir = non_tmp_dir();
        let catalog = fabric_store::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        fs::create_dir_all(&part_dir).unwrap();
        let pq_path = part_dir.join("part-r21-mixed-hdr.parquet");

        let batch = metrics_to_record_batch(&[MetricPoint {
            timestamp_ms: 1_700_000_000_000_i64,
            name: "r21.mixed".into(),
            value: 1.0,
            host: "h".into(),
            slice: "spindle".into(),
            unit: "svc".into(),
            pid: Some(7),
            labels_json: "{}".into(),
        }])
        .unwrap();
        let props = WriterProperties::builder()
            .set_dictionary_enabled(false)
            .set_statistics_enabled(EnabledStatistics::None)
            .build();
        let mut w = ArrowWriter::try_new(
            fs::File::create(&pq_path).unwrap(),
            batch.schema(),
            Some(props),
        )
        .unwrap();
        w.write(&batch).unwrap();
        w.close().unwrap();
        let clean = fs::read(&pq_path).unwrap();

        // 1. V1 DATA_PAGE carrying an extraneous DataPageHeaderV2 with i32::MAX level lengths
        //    must reject before SerializedPageReader/decode_page hits overflowing add.
        let v1_payload = [2_u8, 0, 0, 0, 0x02, 0x01, 7, 0, 0, 0];
        let mut mixed_page = Vec::new();
        mixed_page.extend_from_slice(&[0x15, 0x00]); // type = DATA_PAGE(0)
        mixed_page.push(0x15);
        write_varint_u32(
            &mut mixed_page,
            encode_thrift_zigzag(v1_payload.len() as i32),
        );
        mixed_page.push(0x15);
        write_varint_u32(
            &mut mixed_page,
            encode_thrift_zigzag(v1_payload.len() as i32),
        );
        mixed_page.extend_from_slice(&[0x2c, 0x15, 0x02, 0x15, 0x00, 0x15, 0x06, 0x15, 0x06, 0x00]);
        mixed_page.extend_from_slice(&[0x3c, 0x15, 0x02, 0x15, 0x00, 0x15, 0x02, 0x15, 0x00]);
        mixed_page.push(0x15);
        test_push_zig_zag(&mut mixed_page, i64::from(i32::MAX));
        mixed_page.push(0x15);
        test_push_zig_zag(&mut mixed_page, i64::from(i32::MAX));
        mixed_page.extend_from_slice(&[0x12, 0x00, 0x00]);
        mixed_page.extend_from_slice(&v1_payload);
        {
            let orig_len = clean.len();
            let old_footer_len =
                u32::from_le_bytes(clean[orig_len - 8..orig_len - 4].try_into().unwrap()) as usize;
            let data_end = orig_len - 8 - old_footer_len;
            let meta =
                ParquetMetaDataReader::decode_metadata(&clean[data_end..orig_len - 8]).unwrap();
            let mut b = meta.into_builder();
            let mut rgs = b.take_row_groups();
            let mut rg_b = rgs.remove(0).into_builder();
            let mut cols = rg_b.take_columns();
            let col6 = cols
                .remove(6)
                .into_builder()
                .set_encodings(vec![Encoding::PLAIN])
                .set_compression(Compression::UNCOMPRESSED)
                .set_dictionary_page_offset(None)
                .set_data_page_offset(data_end as i64)
                .set_total_compressed_size(mixed_page.len() as i64)
                .set_total_uncompressed_size(v1_payload.len() as i64)
                .build()
                .unwrap();
            cols.insert(6, col6);
            rg_b = rg_b.set_column_metadata(cols);
            b = b.add_row_group(rg_b.build().unwrap());
            let mut out = clean[..data_end].to_vec();
            out.extend_from_slice(&mixed_page);
            let mut f = fs::File::create(&pq_path).unwrap();
            std::io::Write::write_all(&mut f, &out).unwrap();
            ParquetMetaDataWriter::new(&mut f, &b.build())
                .finish()
                .unwrap();
        }
        let mixed_store_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&pq_path).unwrap(),
                Some(&METRICS_SCHEMA),
                128,
            )
            .expect_err("V1 DATA_PAGE with extraneous DataPageHeaderV2 must reject pre-decode");
        assert!(
            mixed_store_err.contains("unexpected DictionaryPageHeader or DataPageHeaderV2"),
            "unexpected mixed header store err: {mixed_store_err}"
        );
        let ctx_mixed = SessionContext::new();
        register_pinned_parquet_table(
            &ctx_mixed,
            "t",
            METRICS_SCHEMA.clone(),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let mixed_q_err = ctx_mixed
            .sql("SELECT pid FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("mixed V1+V2 page header must reject in PinnedParquetExec");
        assert!(
            mixed_q_err
                .to_string()
                .contains("unexpected DictionaryPageHeader or DataPageHeaderV2"),
            "unexpected mixed header query err: {mixed_q_err}"
        );

        // 2. All-null 1-row OPTIONAL FIXED_LEN_BYTE_ARRAY(1_073_741_824) PLAIN file must reject
        //    pre-decode in both file and bytes maintenance readers before pad_nulls allocates 1 GiB.
        let huge_fsb_pq = dir.path().join("huge-null-fsb.parquet");
        let huge_col_ty = Arc::new(
            SchemaType::primitive_type_builder("blob", PhysicalType::FIXED_LEN_BYTE_ARRAY)
                .with_length(1_073_741_824)
                .with_repetition(Repetition::OPTIONAL)
                .build()
                .unwrap(),
        );
        let root_huge = Arc::new(
            SchemaType::group_type_builder("schema")
                .with_fields(vec![huge_col_ty])
                .build()
                .unwrap(),
        );
        let huge_schema = Arc::new(SchemaDescriptor::new(root_huge));
        let null_def_payload = [2_u8, 0, 0, 0, 0x02, 0x00];
        let mut null_v1_page = Vec::new();
        null_v1_page.extend_from_slice(&[0x15, 0x00]); // DATA_PAGE
        null_v1_page.push(0x15);
        write_varint_u32(
            &mut null_v1_page,
            encode_thrift_zigzag(null_def_payload.len() as i32),
        );
        null_v1_page.push(0x15);
        write_varint_u32(
            &mut null_v1_page,
            encode_thrift_zigzag(null_def_payload.len() as i32),
        );
        null_v1_page.extend_from_slice(&[
            0x2c, 0x15, 0x02, 0x15, 0x00, 0x15, 0x06, 0x15, 0x06, 0x00, 0x00,
        ]);
        null_v1_page.extend_from_slice(&null_def_payload);
        let huge_col_meta = ColumnChunkMetaData::builder(huge_schema.column(0))
            .set_encodings(vec![Encoding::PLAIN])
            .set_compression(Compression::UNCOMPRESSED)
            .set_num_values(1)
            .set_data_page_offset(4)
            .set_total_compressed_size(null_v1_page.len() as i64)
            .set_total_uncompressed_size(null_def_payload.len() as i64)
            .build()
            .unwrap();
        let huge_rg_meta = RowGroupMetaData::builder(Arc::clone(&huge_schema))
            .set_num_rows(1)
            .set_total_byte_size(null_v1_page.len() as i64)
            .set_column_metadata(vec![huge_col_meta])
            .build()
            .unwrap();
        let huge_fmd = FileMetaData::new(1, 1, None, None, huge_schema, None);
        let huge_pq_meta = ParquetMetaDataBuilder::new(huge_fmd)
            .set_row_groups(vec![huge_rg_meta])
            .build();
        {
            use std::io::Write;
            let mut f = fs::File::create(&huge_fsb_pq).unwrap();
            f.write_all(b"PAR1").unwrap();
            f.write_all(&null_v1_page).unwrap();
            ParquetMetaDataWriter::new(&mut f, &huge_pq_meta)
                .finish()
                .unwrap();
        }
        let huge_file_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&huge_fsb_pq).unwrap(),
                None,
                128,
            )
            .expect_err("1 GiB all-null FIXED_LEN_BYTE_ARRAY must reject before pad_nulls");
        assert!(
            huge_file_err.contains("exceeds maintenance decode headroom"),
            "unexpected huge-width fsb file err: {huge_file_err}"
        );
        let hf = fs::File::open(&huge_fsb_pq).unwrap();
        let hlen = usize::try_from(hf.metadata().unwrap().len()).unwrap();
        let hbytes =
            <fs::File as parquet::file::reader::ChunkReader>::get_bytes(&hf, 0, hlen).unwrap();
        let huge_bytes_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_bytes(
                hbytes, None, 128,
            )
            .expect_err("1 GiB all-null FIXED_LEN_BYTE_ARRAY bytes must reject before pad_nulls");
        assert!(
            huge_bytes_err.contains("exceeds maintenance decode headroom"),
            "unexpected huge-width fsb bytes err: {huge_bytes_err}"
        );
    }

    #[tokio::test]
    async fn round22_decimal_bounds_bytes_residency_index_headers_and_mixed_skew_reject_cleanly() {
        use arrow::array::BinaryArray;
        use arrow::datatypes::Field;
        use arrow::ipc::writer::{DictionaryTracker, IpcDataGenerator, IpcWriteOptions};
        use parquet::arrow::ArrowWriter;
        use parquet::basic::{
            Compression, DecimalType, Encoding, LogicalType, Repetition, Type as PhysicalType,
            ZstdLevel,
        };
        use parquet::file::metadata::{
            ColumnChunkMetaData, FileMetaData, KeyValue, ParquetMetaDataBuilder,
            ParquetMetaDataWriter, RowGroupMetaData,
        };
        use parquet::file::properties::{EnabledStatistics, WriterProperties};
        use parquet::schema::types::{SchemaDescriptor, Type as SchemaType};

        let write_varint_u32 = |out: &mut Vec<u8>, mut v: u32| {
            loop {
                let mut byte = (v & 0x7f) as u8;
                v >>= 7;
                if v != 0 {
                    byte |= 0x80;
                }
                out.push(byte);
                if v == 0 {
                    break;
                }
            }
        };
        let encode_thrift_zigzag = |v: i32| -> u32 { ((v << 1) ^ (v >> 31)) as u32 };
        let craft_v1_data_page = |num_vals: i32, val_enc: i32, payload: &[u8]| -> Vec<u8> {
            let mut hdr = Vec::new();
            hdr.extend_from_slice(&[0x15, 0x00]);
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.extend_from_slice(&[0x2c, 0x15]);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(num_vals));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(val_enc));
            hdr.extend_from_slice(&[0x15, 0x06, 0x15, 0x06, 0x00, 0x00]);
            hdr.extend_from_slice(payload);
            hdr
        };
        let craft_dict_page = |dict_num_vals: i32, payload: &[u8]| -> Vec<u8> {
            let mut hdr = Vec::new();
            hdr.extend_from_slice(&[0x15, encode_thrift_zigzag(2) as u8]);
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.extend_from_slice(&[0x4c, 0x15]);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(dict_num_vals));
            hdr.extend_from_slice(&[0x15, 0x00, 0x00, 0x00]);
            hdr.extend_from_slice(payload);
            hdr
        };
        let craft_index_page = |payload: &[u8]| -> Vec<u8> {
            let mut hdr = Vec::new();
            hdr.extend_from_slice(&[0x15, encode_thrift_zigzag(1) as u8]); // PageType::INDEX_PAGE
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.extend_from_slice(&[0x3c, 0x00, 0x00]); // field 6 IndexPageHeader {}, stop PageHeader
            hdr.extend_from_slice(payload);
            hdr
        };
        let read_all_bytes = |path: &Path| {
            let f = fs::File::open(path).unwrap();
            let len = usize::try_from(f.metadata().unwrap().len()).unwrap();
            <fs::File as parquet::file::reader::ChunkReader>::get_bytes(&f, 0, len)
        };
        let write_single_col_parquet = |target: &Path,
                                        col_ty: SchemaType,
                                        num_rows: i64,
                                        encs: Vec<Encoding>,
                                        dict_off: Option<i64>,
                                        data_off: i64,
                                        uncomp_total: i64,
                                        chunk_bytes: &[u8]| {
            let root = Arc::new(
                SchemaType::group_type_builder("schema")
                    .with_fields(vec![Arc::new(col_ty)])
                    .build()
                    .unwrap(),
            );
            let schema_descr = Arc::new(SchemaDescriptor::new(root));
            let col_meta = ColumnChunkMetaData::builder(schema_descr.column(0))
                .set_encodings(encs)
                .set_compression(Compression::UNCOMPRESSED)
                .set_num_values(num_rows)
                .set_dictionary_page_offset(dict_off)
                .set_data_page_offset(data_off)
                .set_total_compressed_size(chunk_bytes.len() as i64)
                .set_total_uncompressed_size(uncomp_total)
                .build()
                .unwrap();
            let rg_meta = RowGroupMetaData::builder(Arc::clone(&schema_descr))
                .set_num_rows(num_rows)
                .set_total_byte_size(chunk_bytes.len() as i64)
                .set_column_metadata(vec![col_meta])
                .build()
                .unwrap();
            let fmd = FileMetaData::new(1, num_rows, None, None, schema_descr, None);
            let pq_meta = ParquetMetaDataBuilder::new(fmd)
                .set_row_groups(vec![rg_meta])
                .build();
            let mut f = fs::File::create(target).unwrap();
            std::io::Write::write_all(&mut f, b"PAR1").unwrap();
            std::io::Write::write_all(&mut f, chunk_bytes).unwrap();
            ParquetMetaDataWriter::new(&mut f, &pq_meta)
                .finish()
                .unwrap();
        };

        let dir = non_tmp_dir();
        let catalog = fabric_store::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        fs::create_dir_all(&part_dir).unwrap();

        // (a1) 1-row BYTE_ARRAY Decimal(10, 2) with 17-byte PLAIN item rejects before sign_extend_be::<16>.
        let dec_pq = part_dir.join("part-r22-dec-plain.parquet");
        let dec_col_ty = || {
            SchemaType::primitive_type_builder("value", PhysicalType::BYTE_ARRAY)
                .with_repetition(Repetition::REQUIRED)
                .with_logical_type(Some(LogicalType::Decimal(DecimalType {
                    scale: 2,
                    precision: 10,
                })))
                .with_precision(10)
                .with_scale(2)
                .build()
                .unwrap()
        };
        let dec_schema = Arc::new(Schema::new(vec![Field::new(
            "value",
            DataType::Decimal128(10, 2),
            false,
        )]));
        let mut plain_dec17_payload = 17_u32.to_le_bytes().to_vec();
        plain_dec17_payload.extend_from_slice(&[0x01_u8; 17]);
        let plain_dec17_page = craft_v1_data_page(1, 0, &plain_dec17_payload);
        write_single_col_parquet(
            &dec_pq,
            dec_col_ty(),
            1,
            vec![Encoding::PLAIN],
            None,
            4,
            plain_dec17_payload.len() as i64,
            &plain_dec17_page,
        );
        let dec_plain_file_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&dec_pq).unwrap(),
                None,
                128,
            )
            .expect_err("17-byte PLAIN Decimal128 value must reject pre-decode in file reader");
        assert!(
            dec_plain_file_err.contains("exceeds Decimal byte bound 16"),
            "unexpected dec plain file err: {dec_plain_file_err}"
        );
        let dec_plain_bytes_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_bytes(
                read_all_bytes(&dec_pq).unwrap(),
                None,
                128,
            )
            .expect_err("17-byte PLAIN Decimal128 value must reject pre-decode in bytes reader");
        assert!(
            dec_plain_bytes_err.contains("exceeds Decimal byte bound 16"),
            "unexpected dec plain bytes err: {dec_plain_bytes_err}"
        );
        let ctx_dec = SessionContext::new();
        register_pinned_parquet_table(
            &ctx_dec,
            "t",
            Arc::clone(&dec_schema),
            olap.snapshot_pinned_parquet_files("metrics", None).unwrap(),
        )
        .unwrap();
        let dec_plain_q_err = ctx_dec
            .sql("SELECT value FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("17-byte PLAIN Decimal128 value must reject in PinnedParquetExec");
        assert!(
            dec_plain_q_err
                .to_string()
                .contains("exceeds Decimal byte bound 16"),
            "unexpected dec plain query err: {dec_plain_q_err}"
        );

        // (a2) 1-row BYTE_ARRAY Decimal(10, 2) with 17-byte DICTIONARY_PAGE entry rejects pre-decode.
        let mut dict_dec17_payload = 17_u32.to_le_bytes().to_vec();
        dict_dec17_payload.extend_from_slice(&[0x02_u8; 17]);
        let dec_dict_part = craft_dict_page(1, &dict_dec17_payload);
        let dec_data_part = craft_v1_data_page(1, 8, &[0_u8]);
        let mut dec_dict_chunk = dec_dict_part.clone();
        dec_dict_chunk.extend_from_slice(&dec_data_part);
        write_single_col_parquet(
            &dec_pq,
            dec_col_ty(),
            1,
            vec![Encoding::PLAIN, Encoding::RLE_DICTIONARY],
            Some(4),
            4 + dec_dict_part.len() as i64,
            (dict_dec17_payload.len() + 1) as i64,
            &dec_dict_chunk,
        );
        let dec_dict_file_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&dec_pq).unwrap(),
                None,
                128,
            )
            .expect_err(
                "17-byte DICTIONARY Decimal128 value must reject pre-decode in file reader",
            );
        assert!(
            dec_dict_file_err.contains("exceeds Decimal byte bound 16"),
            "unexpected dec dict file err: {dec_dict_file_err}"
        );
        let dec_dict_bytes_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_bytes(
                read_all_bytes(&dec_pq).unwrap(),
                None,
                128,
            )
            .expect_err(
                "17-byte DICTIONARY Decimal128 value must reject pre-decode in bytes reader",
            );
        assert!(
            dec_dict_bytes_err.contains("exceeds Decimal byte bound 16"),
            "unexpected dec dict bytes err: {dec_dict_bytes_err}"
        );
        fs::remove_file(&dec_pq).unwrap();

        // (a3) FIXED_LEN_BYTE_ARRAY(16) narrowed to Decimal32(9, 2) via ARROW:schema rejects pre-decode.
        let dec32_pq = dir.path().join("narrow-dec32.parquet");
        let flba16_col =
            SchemaType::primitive_type_builder("d", PhysicalType::FIXED_LEN_BYTE_ARRAY)
                .with_length(16)
                .with_repetition(Repetition::REQUIRED)
                .with_logical_type(Some(LogicalType::Decimal(DecimalType {
                    scale: 2,
                    precision: 9,
                })))
                .with_precision(9)
                .with_scale(2)
                .build()
                .unwrap();
        let flba16_page = craft_v1_data_page(1, 0, &[0_u8; 16]);
        write_single_col_parquet(
            &dec32_pq,
            flba16_col,
            1,
            vec![Encoding::PLAIN],
            None,
            4,
            16,
            &flba16_page,
        );
        let ipc_opts = IpcWriteOptions::default();
        let mut dt = DictionaryTracker::new(true);
        let dec32_msg = IpcDataGenerator::default().schema_to_bytes_with_dictionary_tracker(
            &Schema::new(vec![Field::new("d", DataType::Decimal32(9, 2), false)]),
            &mut dt,
            &ipc_opts,
        );
        let mut dec32_ipc = vec![255_u8; 4];
        dec32_ipc.extend_from_slice(&(dec32_msg.ipc_message.len() as u32).to_le_bytes());
        dec32_ipc.extend_from_slice(&dec32_msg.ipc_message);
        let dec32_b64 = test_encode_base64(&dec32_ipc);
        rewrite_parquet_footer_with_meta(&dec32_pq, |meta| {
            let fm = meta.file_metadata();
            let new_fm = FileMetaData::new(
                fm.version(),
                fm.num_rows(),
                fm.created_by().map(str::to_owned),
                Some(vec![KeyValue::new(
                    "ARROW:schema".to_owned(),
                    dec32_b64.clone(),
                )]),
                fm.schema_descr_ptr(),
                fm.column_orders().map(|o| o.to_vec()),
            );
            parquet::file::metadata::ParquetMetaData::new(new_fm, meta.row_groups().to_vec())
        });
        let dec32_err = fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
            fs::File::open(&dec32_pq).unwrap(),
            None,
            128,
        )
        .expect_err("FIXED_LEN_BYTE_ARRAY(16) narrowed to Decimal32 must reject pre-decode");
        assert!(
            dec32_err.contains("Decimal32 parquet column") && dec32_err.contains("exceeds 4 bytes"),
            "unexpected Decimal32 narrowing err: {dec32_err}"
        );

        // (a4) INT64 Decimal(10, 2) narrowed to Decimal32(9, 2) via ARROW:schema rejects pre-decode.
        let i64_dec32_pq = dir.path().join("narrow-i64-dec32.parquet");
        let i64_dec_col = SchemaType::primitive_type_builder("d", PhysicalType::INT64)
            .with_repetition(Repetition::REQUIRED)
            .with_logical_type(Some(LogicalType::Decimal(DecimalType {
                scale: 2,
                precision: 10,
            })))
            .with_precision(10)
            .with_scale(2)
            .build()
            .unwrap();
        let i64_dec_page = craft_v1_data_page(1, 0, &42_i64.to_le_bytes());
        write_single_col_parquet(
            &i64_dec32_pq,
            i64_dec_col,
            1,
            vec![Encoding::PLAIN],
            None,
            4,
            8,
            &i64_dec_page,
        );
        rewrite_parquet_footer_with_meta(&i64_dec32_pq, |meta| {
            let fm = meta.file_metadata();
            let new_fm = FileMetaData::new(
                fm.version(),
                fm.num_rows(),
                fm.created_by().map(str::to_owned),
                Some(vec![KeyValue::new(
                    "ARROW:schema".to_owned(),
                    dec32_b64.clone(),
                )]),
                fm.schema_descr_ptr(),
                fm.column_orders().map(|o| o.to_vec()),
            );
            parquet::file::metadata::ParquetMetaData::new(new_fm, meta.row_groups().to_vec())
        });
        let i64_dec32_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&i64_dec32_pq).unwrap(),
                None,
                128,
            )
            .expect_err("INT64 Decimal(10, 2) narrowed to Decimal32 must reject pre-decode");
        assert!(
            i64_dec32_err.contains("unsupported INT64 Decimal32"),
            "unexpected INT64 Decimal32 narrowing err: {i64_dec32_err}"
        );

        // (b) Bytes & file readers reject 36 MiB skipped INDEX_PAGE + 32 MiB all-null FLBA column.
        let idx_flba_pq = dir.path().join("idx-residency-32mib.parquet");
        let idx_col_ty =
            SchemaType::primitive_type_builder("big", PhysicalType::FIXED_LEN_BYTE_ARRAY)
                .with_length(32 * 1024 * 1024)
                .with_repetition(Repetition::OPTIONAL)
                .build()
                .unwrap();
        let big_idx_payload = vec![0_u8; 36 * 1024 * 1024];
        let big_idx_page = craft_index_page(&big_idx_payload);
        let null_1row_payload = [2_u8, 0, 0, 0, 0x02, 0x00];
        let null_1row_page = craft_v1_data_page(1, 0, &null_1row_payload);
        let mut idx_chunk = big_idx_page.clone();
        idx_chunk.extend_from_slice(&null_1row_page);
        write_single_col_parquet(
            &idx_flba_pq,
            idx_col_ty,
            1,
            vec![Encoding::PLAIN],
            None,
            4,
            (big_idx_payload.len() + null_1row_payload.len()) as i64,
            &idx_chunk,
        );
        let idx_bytes_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_bytes(
                read_all_bytes(&idx_flba_pq).unwrap(),
                None,
                128,
            )
            .expect_err("36 MiB skipped INDEX_PAGE + 32 MiB FLBA must reject in bytes reader");
        assert!(
            idx_bytes_err.contains("exceeds maintenance decode headroom"),
            "unexpected idx residency bytes err: {idx_bytes_err}"
        );
        let idx_file_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&idx_flba_pq).unwrap(),
                None,
                128,
            )
            .expect_err("36 MiB skipped INDEX_PAGE + 32 MiB FLBA must reject in file reader");
        assert!(
            idx_file_err.contains("exceeds maintenance decode headroom"),
            "unexpected idx residency file err: {idx_file_err}"
        );

        // (c) IndexPageHeader uniqueness & PageType consistency across all 3 malformed forms.
        let idx_hdr_pq = dir.path().join("idx-header-checks.parquet");
        let small_int_ty = || {
            SchemaType::primitive_type_builder("x", PhysicalType::INT32)
                .with_repetition(Repetition::REQUIRED)
                .build()
                .unwrap()
        };
        // (c1) V1 DATA_PAGE with extraneous fid=6 IndexPageHeader
        let mut data_with_idx = Vec::new();
        data_with_idx.extend_from_slice(&[0x15, 0x00, 0x15, 0x08, 0x15, 0x08]);
        data_with_idx
            .extend_from_slice(&[0x2c, 0x15, 0x02, 0x15, 0x00, 0x15, 0x06, 0x15, 0x06, 0x00]);
        data_with_idx.extend_from_slice(&[0x1c, 0x00, 0x00]); // fid 6 after fid 5
        data_with_idx.extend_from_slice(&7_i32.to_le_bytes());
        write_single_col_parquet(
            &idx_hdr_pq,
            small_int_ty(),
            1,
            vec![Encoding::PLAIN],
            None,
            4,
            4,
            &data_with_idx,
        );
        let c1_err = fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
            fs::File::open(&idx_hdr_pq).unwrap(),
            None,
            128,
        )
        .expect_err("DATA_PAGE with extraneous IndexPageHeader must reject");
        assert!(
            c1_err.contains("unexpected IndexPageHeader on non-INDEX_PAGE"),
            "unexpected c1 err: {c1_err}"
        );
        // (c2) INDEX_PAGE missing IndexPageHeader
        let mut idx_missing_chunk = vec![0x15_u8, 0x02, 0x15, 0x00, 0x15, 0x00, 0x00];
        let ok_int_page = craft_v1_data_page(1, 0, &7_i32.to_le_bytes());
        idx_missing_chunk.extend_from_slice(&ok_int_page);
        write_single_col_parquet(
            &idx_hdr_pq,
            small_int_ty(),
            1,
            vec![Encoding::PLAIN],
            None,
            4,
            4,
            &idx_missing_chunk,
        );
        let c2_err = fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
            fs::File::open(&idx_hdr_pq).unwrap(),
            None,
            128,
        )
        .expect_err("INDEX_PAGE missing IndexPageHeader must reject");
        assert!(
            c2_err.contains("missing IndexPageHeader for INDEX_PAGE"),
            "unexpected c2 err: {c2_err}"
        );
        // (c3) INDEX_PAGE with duplicate IndexPageHeader
        let mut idx_dup_chunk = vec![
            0x15_u8, 0x02, 0x15, 0x00, 0x15, 0x00, // PageHeader type=1, uncomp=0, comp=0
            0x0c, 0x0c, 0x00, // first fid=6 IndexPageHeader {}
            0x0c, 0x0c, 0x00, // duplicate fid=6 IndexPageHeader {}
            0x00, // STOP
        ];
        idx_dup_chunk.extend_from_slice(&ok_int_page);
        write_single_col_parquet(
            &idx_hdr_pq,
            small_int_ty(),
            1,
            vec![Encoding::PLAIN],
            None,
            4,
            4,
            &idx_dup_chunk,
        );
        let c3_err = fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
            fs::File::open(&idx_hdr_pq).unwrap(),
            None,
            128,
        )
        .expect_err("INDEX_PAGE with duplicate IndexPageHeader must reject");
        assert!(
            c3_err.contains("duplicate IndexPageHeader in parquet PageHeader"),
            "unexpected c3 err: {c3_err}"
        );

        // (d) 4-row mixed skewed row group: 44 MiB all-null FLBA + 12 MiB ZSTD skewed BYTE_ARRAY
        //     passes `fixed + avg_varlen <= headroom` (~47 MiB <= 52 MiB) but fails
        //     `worst_row_bytes > headroom` (~56 MiB > 52 MiB) in both maintenance readers.
        let zstd_src_pq = dir.path().join("zstd-skew-src.parquet");
        let skew_schema = Arc::new(Schema::new(vec![Field::new(
            "payload",
            DataType::Binary,
            false,
        )]));
        let skew_row0 = vec![0_u8; 12 * 1024 * 1024];
        let skew_arr = BinaryArray::from_vec(vec![skew_row0.as_slice(), b"", b"", b""]);
        let skew_batch =
            RecordBatch::try_new(Arc::clone(&skew_schema), vec![Arc::new(skew_arr)]).unwrap();
        let zstd_props = WriterProperties::builder()
            .set_dictionary_enabled(false)
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(1).unwrap()))
            .set_statistics_enabled(EnabledStatistics::None)
            .build();
        let mut zw = ArrowWriter::try_new(
            fs::File::create(&zstd_src_pq).unwrap(),
            skew_schema,
            Some(zstd_props),
        )
        .unwrap();
        zw.write(&skew_batch).unwrap();
        zw.close().unwrap();
        let zstd_bytes = fs::read(&zstd_src_pq).unwrap();
        let zstd_meta = ParquetMetaDataReader::new()
            .parse_and_finish(&fs::File::open(&zstd_src_pq).unwrap())
            .unwrap();
        let zc = zstd_meta.row_group(0).column(0);
        let (zc_start, zc_len) = zc.byte_range();
        let zstd_chunk = &zstd_bytes[zc_start as usize..(zc_start + zc_len) as usize];

        let mix_pq = dir.path().join("mixed-skew-bypass.parquet");
        let mix_col0_ty = Arc::new(
            SchemaType::primitive_type_builder("pad", PhysicalType::FIXED_LEN_BYTE_ARRAY)
                .with_length(44 * 1024 * 1024)
                .with_repetition(Repetition::OPTIONAL)
                .build()
                .unwrap(),
        );
        let mix_col1_ty = Arc::new(
            SchemaType::primitive_type_builder("payload", PhysicalType::BYTE_ARRAY)
                .with_repetition(Repetition::REQUIRED)
                .build()
                .unwrap(),
        );
        let mix_root = Arc::new(
            SchemaType::group_type_builder("schema")
                .with_fields(vec![mix_col0_ty, mix_col1_ty])
                .build()
                .unwrap(),
        );
        let mix_schema = Arc::new(SchemaDescriptor::new(mix_root));
        let null_4row_payload = [2_u8, 0, 0, 0, 0x08, 0x00];
        let null_4row_page = craft_v1_data_page(4, 0, &null_4row_payload);
        let mix_c0_meta = ColumnChunkMetaData::builder(mix_schema.column(0))
            .set_encodings(vec![Encoding::PLAIN])
            .set_compression(Compression::UNCOMPRESSED)
            .set_num_values(4)
            .set_data_page_offset(4)
            .set_total_compressed_size(null_4row_page.len() as i64)
            .set_total_uncompressed_size(null_4row_payload.len() as i64)
            .build()
            .unwrap();
        let c1_off = 4 + null_4row_page.len() as i64;
        let mix_c1_meta = ColumnChunkMetaData::builder(mix_schema.column(1))
            .set_encodings(vec![Encoding::PLAIN])
            .set_compression(zc.compression())
            .set_num_values(4)
            .set_data_page_offset(c1_off)
            .set_total_compressed_size(zstd_chunk.len() as i64)
            .set_total_uncompressed_size(zc.uncompressed_size())
            .build()
            .unwrap();
        let mix_rg_meta = RowGroupMetaData::builder(Arc::clone(&mix_schema))
            .set_num_rows(4)
            .set_total_byte_size((null_4row_page.len() + zstd_chunk.len()) as i64)
            .set_column_metadata(vec![mix_c0_meta, mix_c1_meta])
            .build()
            .unwrap();
        let mix_fmd = FileMetaData::new(1, 4, None, None, mix_schema, None);
        let mix_pq_meta = ParquetMetaDataBuilder::new(mix_fmd)
            .set_row_groups(vec![mix_rg_meta])
            .build();
        {
            use std::io::Write;
            let mut f = fs::File::create(&mix_pq).unwrap();
            f.write_all(b"PAR1").unwrap();
            f.write_all(&null_4row_page).unwrap();
            f.write_all(zstd_chunk).unwrap();
            ParquetMetaDataWriter::new(&mut f, &mix_pq_meta)
                .finish()
                .unwrap();
        }
        let mix_file_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&mix_pq).unwrap(),
                None,
                128,
            )
            .expect_err("skewed 44 MiB FLBA + 12 MiB ZSTD BYTE_ARRAY must reject in file reader");
        assert!(
            mix_file_err.contains("exceeds maintenance decode headroom"),
            "unexpected mix skew file err: {mix_file_err}"
        );
        let mix_bytes_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_bytes(
                read_all_bytes(&mix_pq).unwrap(),
                None,
                128,
            )
            .expect_err("skewed 44 MiB FLBA + 12 MiB ZSTD BYTE_ARRAY must reject in bytes reader");
        assert!(
            mix_bytes_err.contains("exceeds maintenance decode headroom"),
            "unexpected mix skew bytes err: {mix_bytes_err}"
        );
    }

    #[tokio::test]
    async fn round23_conversion_peak_slot_expansion_and_result_collection_budget_reject_cleanly() {
        use arrow::datatypes::Field;
        use arrow::ipc::writer::{DictionaryTracker, IpcDataGenerator, IpcWriteOptions};
        use fabric_core::LogRecord;
        use fabric_store::parquet_guard::{
            MAX_MAINTENANCE_ROW_GROUP_DECODE_BYTES, estimate_arrow_schema_bytes,
            load_verified_parquet_metadata_from_file,
        };
        use parquet::basic::{
            Compression, DecimalType, Encoding, LogicalType, Repetition, Type as PhysicalType,
        };
        use parquet::file::metadata::{
            ColumnChunkMetaData, FileMetaData, KeyValue, ParquetMetaDataBuilder,
            ParquetMetaDataWriter, RowGroupMetaData,
        };
        use parquet::schema::types::{SchemaDescriptor, Type as SchemaType};

        let write_varint_u32 = |out: &mut Vec<u8>, mut v: u32| {
            loop {
                let mut byte = (v & 0x7f) as u8;
                v >>= 7;
                if v != 0 {
                    byte |= 0x80;
                }
                out.push(byte);
                if v == 0 {
                    break;
                }
            }
        };
        let encode_thrift_zigzag = |v: i32| -> u32 { ((v << 1) ^ (v >> 31)) as u32 };
        let craft_v1_data_page = |num_vals: i32, payload: &[u8]| -> Vec<u8> {
            let mut hdr = Vec::new();
            hdr.extend_from_slice(&[0x15, 0x00]);
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.extend_from_slice(&[0x2c, 0x15]);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(num_vals));
            hdr.extend_from_slice(&[0x15, 0x00, 0x15, 0x06, 0x15, 0x06, 0x00, 0x00]);
            hdr.extend_from_slice(payload);
            hdr
        };
        let craft_index_page = |payload: &[u8]| -> Vec<u8> {
            let mut hdr = Vec::new();
            hdr.extend_from_slice(&[0x15, encode_thrift_zigzag(1) as u8]);
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.push(0x15);
            write_varint_u32(&mut hdr, encode_thrift_zigzag(payload.len() as i32));
            hdr.extend_from_slice(&[0x3c, 0x00, 0x00]);
            hdr.extend_from_slice(payload);
            hdr
        };
        let craft_null_rle_payload = |num_rows: u32| -> Vec<u8> {
            let mut rle = Vec::new();
            write_varint_u32(&mut rle, num_rows << 1);
            rle.push(0);
            let mut payload = (rle.len() as u32).to_le_bytes().to_vec();
            payload.extend_from_slice(&rle);
            payload
        };
        let arrow_schema_kv = |schema: &Schema| -> KeyValue {
            let ipc_opts = IpcWriteOptions::default();
            let mut dt = DictionaryTracker::new(true);
            let msg = IpcDataGenerator::default()
                .schema_to_bytes_with_dictionary_tracker(schema, &mut dt, &ipc_opts);
            let mut framed = vec![255_u8; 4];
            framed.extend_from_slice(&(msg.ipc_message.len() as u32).to_le_bytes());
            framed.extend_from_slice(&msg.ipc_message);
            KeyValue::new("ARROW:schema".to_owned(), test_encode_base64(&framed))
        };
        let read_all_bytes = |path: &Path| {
            let f = fs::File::open(path).unwrap();
            let len = usize::try_from(f.metadata().unwrap().len()).unwrap();
            <fs::File as parquet::file::reader::ChunkReader>::get_bytes(&f, 0, len)
        };

        let dir = non_tmp_dir();

        // (#159.1) Near-cap 1-row all-null OPTIONAL FIXED_LEN_BYTE_ARRAY(32) Decimal256(50, 2)
        // in both file & bytes maintenance readers: headroom ~50 B (> 32 B physical width,
        // < 70 B simultaneous FixedSizeBinary + Decimal256 + level/null-bitmap peak).
        let write_dec256_flba32_idx_pq = |path: &Path, idx_payload_len: usize| -> (usize, usize) {
            let col_ty =
                SchemaType::primitive_type_builder("dec", PhysicalType::FIXED_LEN_BYTE_ARRAY)
                    .with_length(32)
                    .with_repetition(Repetition::OPTIONAL)
                    .with_logical_type(Some(LogicalType::Decimal(DecimalType {
                        scale: 2,
                        precision: 50,
                    })))
                    .with_precision(50)
                    .with_scale(2)
                    .build()
                    .unwrap();
            let root = Arc::new(
                SchemaType::group_type_builder("schema")
                    .with_fields(vec![Arc::new(col_ty)])
                    .build()
                    .unwrap(),
            );
            let schema_descr = Arc::new(SchemaDescriptor::new(root));
            let arrow_schema =
                Schema::new(vec![Field::new("dec", DataType::Decimal256(50, 2), true)]);
            let p1 = idx_payload_len / 2;
            let p2 = idx_payload_len - p1;
            let mut chunk = craft_index_page(&vec![0_u8; p1]);
            chunk.extend_from_slice(&craft_index_page(&vec![0_u8; p2]));
            let null_payload = craft_null_rle_payload(1);
            let null_page = craft_v1_data_page(1, &null_payload);
            chunk.extend_from_slice(&null_page);
            let col_meta = ColumnChunkMetaData::builder(schema_descr.column(0))
                .set_encodings(vec![Encoding::PLAIN])
                .set_compression(Compression::UNCOMPRESSED)
                .set_num_values(1)
                .set_data_page_offset(4)
                .set_total_compressed_size(chunk.len() as i64)
                .set_total_uncompressed_size((idx_payload_len + null_payload.len()) as i64)
                .build()
                .unwrap();
            let rg_meta = RowGroupMetaData::builder(Arc::clone(&schema_descr))
                .set_num_rows(1)
                .set_total_byte_size(chunk.len() as i64)
                .set_column_metadata(vec![col_meta])
                .build()
                .unwrap();
            let fmd = FileMetaData::new(
                1,
                1,
                None,
                Some(vec![arrow_schema_kv(&arrow_schema)]),
                schema_descr,
                None,
            );
            let pq_meta = ParquetMetaDataBuilder::new(fmd)
                .set_row_groups(vec![rg_meta])
                .build();
            {
                use std::io::Write;
                let mut f = fs::File::create(path).unwrap();
                f.write_all(b"PAR1").unwrap();
                f.write_all(&chunk).unwrap();
                ParquetMetaDataWriter::new(&mut f, &pq_meta)
                    .finish()
                    .unwrap();
            }
            let f = fs::File::open(path).unwrap();
            let (file_len, _, reader_meta) =
                load_verified_parquet_metadata_from_file(&f, None).unwrap();
            let scan = scan_column_chunk_page_headers(
                &mut std::io::Cursor::new(&chunk),
                0,
                chunk.len() as u64,
                PhysicalType::FIXED_LEN_BYTE_ARRAY,
                32,
                1,
                1,
            )
            .unwrap();
            let meta_bytes = reader_meta
                .metadata()
                .memory_size()
                .saturating_add(estimate_arrow_schema_bytes(reader_meta.schema()))
                .max(PINNED_FOOTER_MIN_BYTES);
            let file_rg_resident = meta_bytes + chunk.len() + 256 + scan.resident_bytes();
            let bytes_rg_resident = meta_bytes + (file_len as usize) + 256 + scan.resident_bytes();
            (file_rg_resident, bytes_rg_resident)
        };

        let near_cap_file_pq = dir.path().join("r23-near-cap-file.parquet");
        let mut idx_len_file = 67_090_000_usize;
        let (probe_res_file, _) = write_dec256_flba32_idx_pq(&near_cap_file_pq, idx_len_file);
        let delta_file =
            MAX_MAINTENANCE_ROW_GROUP_DECODE_BYTES as isize - probe_res_file as isize - 50;
        idx_len_file = (idx_len_file as isize + delta_file) as usize;
        let (final_res_file, _) = write_dec256_flba32_idx_pq(&near_cap_file_pq, idx_len_file);
        let file_headroom = MAX_MAINTENANCE_ROW_GROUP_DECODE_BYTES - final_res_file;
        assert!(
            (36..68).contains(&file_headroom),
            "expected near-cap file headroom inside (36, 68), got {file_headroom}"
        );
        let r23_file_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
                fs::File::open(&near_cap_file_pq).unwrap(),
                None,
                128,
            )
            .expect_err("near-cap all-null FLBA(32) Decimal256 must reject dual-buffer conversion peak in file reader");
        assert!(
            r23_file_err.contains("exceeds maintenance decode headroom"),
            "unexpected r23 file err: {r23_file_err}"
        );

        let near_cap_bytes_pq = dir.path().join("r23-near-cap-bytes.parquet");
        let mut idx_len_bytes = 67_089_500_usize;
        let (_, probe_res_bytes) = write_dec256_flba32_idx_pq(&near_cap_bytes_pq, idx_len_bytes);
        let delta_bytes =
            MAX_MAINTENANCE_ROW_GROUP_DECODE_BYTES as isize - probe_res_bytes as isize - 50;
        idx_len_bytes = (idx_len_bytes as isize + delta_bytes) as usize;
        let (_, final_res_bytes) = write_dec256_flba32_idx_pq(&near_cap_bytes_pq, idx_len_bytes);
        let bytes_headroom = MAX_MAINTENANCE_ROW_GROUP_DECODE_BYTES - final_res_bytes;
        assert!(
            (36..68).contains(&bytes_headroom),
            "expected near-cap bytes headroom inside (36, 68), got {bytes_headroom}"
        );
        let r23_bytes_err =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_bytes(
                read_all_bytes(&near_cap_bytes_pq).unwrap(),
                None,
                128,
            )
            .expect_err("near-cap all-null FLBA(32) Decimal256 must reject dual-buffer conversion peak in bytes reader");
        assert!(
            r23_bytes_err.contains("exceeds maintenance decode headroom"),
            "unexpected r23 bytes err: {r23_bytes_err}"
        );

        // (#161.1) PinnedParquetExec accounts Arrow fixed-slot expansion + conversion peak across
        // 8 optional FIXED_LEN_BYTE_ARRAY(1) columns promoted to Decimal256(50, 2) via ARROW:schema.
        let flba1_dec_schema = Arc::new(Schema::new(
            (0..8)
                .map(|i| Field::new(format!("c{i}"), DataType::Decimal256(50, 2), true))
                .collect::<Vec<_>>(),
        ));
        let build_flba1_dec256_pq = |path: &Path, num_rows: u32, idx_payload_len: usize| {
            let fields = (0..8)
                .map(|i| {
                    Arc::new(
                        SchemaType::primitive_type_builder(
                            &format!("c{i}"),
                            PhysicalType::FIXED_LEN_BYTE_ARRAY,
                        )
                        .with_length(1)
                        .with_repetition(Repetition::OPTIONAL)
                        .with_logical_type(Some(LogicalType::Decimal(DecimalType {
                            scale: 0,
                            precision: 2,
                        })))
                        .with_precision(2)
                        .with_scale(0)
                        .build()
                        .unwrap(),
                    )
                })
                .collect::<Vec<_>>();
            let root = Arc::new(
                SchemaType::group_type_builder("schema")
                    .with_fields(fields)
                    .build()
                    .unwrap(),
            );
            let schema_descr = Arc::new(SchemaDescriptor::new(root));
            let null_payload = craft_null_rle_payload(num_rows);
            let null_page = craft_v1_data_page(num_rows as i32, &null_payload);

            let mut file_body = Vec::new();
            let mut col_metas = Vec::with_capacity(8);
            for col_idx in 0..8 {
                let start_off = 4 + file_body.len() as i64;
                let (comp_len, uncomp_len) = if col_idx == 0 && idx_payload_len > 0 {
                    let p1 = idx_payload_len / 2;
                    let p2 = idx_payload_len - p1;
                    let idx_page_1 = craft_index_page(&vec![0_u8; p1]);
                    let idx_page_2 = craft_index_page(&vec![0_u8; p2]);
                    file_body.extend_from_slice(&idx_page_1);
                    file_body.extend_from_slice(&idx_page_2);
                    file_body.extend_from_slice(&null_page);
                    (
                        (idx_page_1.len() + idx_page_2.len() + null_page.len()) as i64,
                        (idx_payload_len + null_payload.len()) as i64,
                    )
                } else {
                    file_body.extend_from_slice(&null_page);
                    (null_page.len() as i64, null_payload.len() as i64)
                };
                let cm = ColumnChunkMetaData::builder(schema_descr.column(col_idx))
                    .set_encodings(vec![Encoding::PLAIN])
                    .set_compression(Compression::UNCOMPRESSED)
                    .set_num_values(i64::from(num_rows))
                    .set_data_page_offset(start_off)
                    .set_total_compressed_size(comp_len)
                    .set_total_uncompressed_size(uncomp_len)
                    .build()
                    .unwrap();
                col_metas.push(cm);
            }
            let total_bytes: i64 = col_metas.iter().map(|c| c.compressed_size()).sum();
            let rg_meta = RowGroupMetaData::builder(Arc::clone(&schema_descr))
                .set_num_rows(i64::from(num_rows))
                .set_total_byte_size(total_bytes)
                .set_column_metadata(col_metas)
                .build()
                .unwrap();
            let fmd = FileMetaData::new(
                1,
                i64::from(num_rows),
                None,
                Some(vec![arrow_schema_kv(&flba1_dec_schema)]),
                schema_descr,
                None,
            );
            let pq_meta = ParquetMetaDataBuilder::new(fmd)
                .set_row_groups(vec![rg_meta])
                .build();
            {
                use std::io::Write;
                let mut f = fs::File::create(path).unwrap();
                f.write_all(b"PAR1").unwrap();
                f.write_all(&file_body).unwrap();
                ParquetMetaDataWriter::new(&mut f, &pq_meta)
                    .finish()
                    .unwrap();
            }
        };

        // Check direct 4,096-row planner output matches schema-aware batch shrinking (<= 840 rows).
        let plan_check_dir = dir.path().join("r23-plan-check");
        fs::create_dir_all(plan_check_dir.join("olap/metrics/dt=2023-11-14")).unwrap();
        let _plan_cat = fabric_store::CatalogStore::open(&plan_check_dir).unwrap();
        let plan_pq = plan_check_dir.join("olap/metrics/dt=2023-11-14/part-4096.parquet");
        build_flba1_dec256_pq(&plan_pq, 4_096, 0);
        let (_, _, plan_rmeta) =
            load_verified_parquet_metadata_from_file(&fs::File::open(&plan_pq).unwrap(), None)
                .unwrap();
        let (plan_batch_rows, plan_res_bytes, plan_decode_budget) = estimate_row_group_decode_plan(
            plan_rmeta.metadata().row_group(0),
            plan_rmeta.schema(),
            fs::File::open(&plan_pq).unwrap().metadata().unwrap().len(),
            &ProjectionMask::all(),
            None,
            8,
            None,
        )
        .unwrap();
        assert!(
            plan_batch_rows <= 840,
            "8-col FLBA(1)->Decimal256 must shrink batch_rows <= 840, got {plan_batch_rows}"
        );
        assert!(
            plan_decode_budget - plan_res_bytes >= plan_batch_rows * 312 + 8 * 256,
            "decode budget {} must include 312 B/row Decimal256 peak over resident {}",
            plan_decode_budget,
            plan_res_bytes
        );

        // Now verify PinnedParquetExec rejects pre-reader.next() when 600-row Decimal256
        // expansion exceeds ~180 KiB remaining headroom inside UNIFIED_POOL_BYTES (64 MiB).
        let flba_exec_root = non_tmp_dir();
        let flba_exec_cat = fabric_store::CatalogStore::open(flba_exec_root.path()).unwrap();
        let flba_exec_olap = OlapParquetStore::new(flba_exec_cat.layout().clone());
        let flba_exec_part = flba_exec_cat
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        fs::create_dir_all(&flba_exec_part).unwrap();
        let flba_exec_pq = flba_exec_part.join("part-r23-flba1-dec256.parquet");
        let mut exec_idx_len = 66_880_000_usize;
        build_flba1_dec256_pq(&flba_exec_pq, 600, exec_idx_len);
        let measure_exec_rg_resident = |pq_path: &Path| -> usize {
            let f = fs::File::open(pq_path).unwrap();
            let (fl, ftl, rmeta) = load_verified_parquet_metadata_from_file(&f, None).unwrap();
            let retained_footer_bytes = rmeta
                .metadata()
                .memory_size()
                .saturating_add(estimate_arrow_schema_bytes(rmeta.schema()))
                .saturating_add(PINNED_FOOTER_MIN_BYTES);
            let d_end = fl.checked_sub(8 + ftl as u64).unwrap();
            let rg = rmeta.metadata().row_group(0);
            let mut scans = vec![None; 8];
            for (col_idx, slot) in scans.iter_mut().enumerate() {
                let col = rg.column(col_idx);
                let (c_off, c_len) = checked_column_byte_range(col, col_idx, d_end).unwrap();
                let c_bytes = <fs::File as parquet::file::reader::ChunkReader>::get_bytes(
                    &f,
                    c_off,
                    c_len as usize,
                )
                .unwrap();
                *slot = Some(
                    scan_column_chunk_page_headers(
                        &mut std::io::Cursor::new(&c_bytes),
                        0,
                        c_len,
                        col.column_type(),
                        col.column_descr().type_length(),
                        col.column_descr().max_def_level(),
                        600,
                    )
                    .unwrap(),
                );
            }
            let (_, rg_res, _) = estimate_row_group_decode_plan(
                rg,
                rmeta.schema(),
                d_end,
                &ProjectionMask::all(),
                None,
                8,
                Some(&scans),
            )
            .unwrap();
            retained_footer_bytes.saturating_add(rg_res)
        };
        let probe_exec_res = measure_exec_rg_resident(&flba_exec_pq);
        let exec_delta = UNIFIED_POOL_BYTES as isize - probe_exec_res as isize - 180_000;
        exec_idx_len = (exec_idx_len as isize + exec_delta) as usize;
        build_flba1_dec256_pq(&flba_exec_pq, 600, exec_idx_len);
        let final_exec_res = measure_exec_rg_resident(&flba_exec_pq);
        let exec_headroom = UNIFIED_POOL_BYTES - final_exec_res;
        assert!(
            (150_000..210_000).contains(&exec_headroom),
            "expected PinnedParquetExec headroom in (150_000, 210_000), got {exec_headroom}"
        );

        let spill_exec_dir = fabric_store::ensure_durable_layout_subdir(
            flba_exec_cat.layout(),
            &flba_exec_cat.layout().spill_dir(),
        )
        .unwrap();
        let exec_env = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::new(GreedyMemoryPool::new(UNIFIED_POOL_BYTES)))
            .with_temp_file_path(spill_exec_dir)
            .with_max_temp_directory_size(UNIFIED_SPILL_BYTES)
            .build_arc()
            .unwrap();
        let exec_ctx = SessionContext::new_with_config_rt(
            SessionConfig::new().with_target_partitions(2),
            exec_env,
        );
        register_pinned_parquet_table(
            &exec_ctx,
            "t",
            Arc::clone(&flba1_dec_schema),
            flba_exec_olap
                .snapshot_pinned_parquet_files("metrics", None)
                .unwrap(),
        )
        .unwrap();
        let exec_err = exec_ctx
            .sql("SELECT c0, c1, c2, c3, c4, c5, c6, c7 FROM t")
            .await
            .unwrap()
            .collect()
            .await
            .expect_err("600-row 8-col FLBA(1)->Decimal256 past ~180 KiB pool headroom must reject pre-decode");
        assert!(
            exec_err
                .to_string()
                .contains("Failed to allocate additional"),
            "unexpected PinnedParquetExec pool err: {exec_err}"
        );

        // (#159.2) Collected result batches + JSON strings obey MAX_UNIFIED_QUERY_RESULT_BYTES
        // inside the unified 64 MiB pool even when row count (40) is well under limit (5000).
        let logs_root = non_tmp_dir();
        let logs_cat = fabric_store::CatalogStore::open(logs_root.path()).unwrap();
        let logs_olap = OlapParquetStore::new(logs_cat.layout().clone());
        let big_msg = "m".repeat(520_000);
        for file_idx in 0..10_usize {
            let chunk: Vec<LogRecord> = (0..4_usize)
                .map(|row| LogRecord {
                    timestamp_ms: 1_700_000_000_000_i64 + ((file_idx * 4 + row) as i64) * 10,
                    level: "info".into(),
                    source: "r23".into(),
                    unit: "test.service".into(),
                    message: format!("{file_idx:02}_{row:02}_{big_msg}"),
                    attributes_json: "{}".into(),
                })
                .collect();
            logs_olap.write_logs(&chunk).unwrap();
        }
        let over_budget_err = execute_unified_query(
            logs_root.path(),
            &UnifiedQuerySpec {
                domain: QueryDomain::Logs,
                since_ms: None,
                where_clauses: Vec::new(),
                group_by: Vec::new(),
                agg_specs: Vec::new(),
                limit: 5_000,
            },
            &[],
            None,
        )
        .await
        .expect_err("40 x 520 KiB log rows (~40 MiB retained) must reject under 32 MiB result cap");
        assert!(
            over_budget_err.contains("exceeds maximum retained budget"),
            "unexpected over-budget unified query err: {over_budget_err}"
        );

        let under_budget_ok = execute_unified_query(
            logs_root.path(),
            &UnifiedQuerySpec {
                domain: QueryDomain::Logs,
                since_ms: None,
                where_clauses: Vec::new(),
                group_by: Vec::new(),
                agg_specs: Vec::new(),
                limit: 2,
            },
            &[],
            None,
        )
        .await
        .expect("2 x 520 KiB log rows (~2.1 MiB retained) must succeed cleanly");
        assert_eq!(under_budget_ok.row_count, 2);
    }

    #[tokio::test]
    async fn round24_bounded_pinned_snapshot_and_live_input_pre_reservation_reject_cleanly() {
        use fabric_core::{AgentSessionRecord, FsChurnEntry, UnitRecord};

        // 1. (#164.1) Bounded pinned parquet snapshot discovery & Arc-shared metadata accounting.
        let dir = non_tmp_dir();
        let cat = fabric_store::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(cat.layout().clone());
        let day1 = 1_700_000_000_000_i64;
        let day2 = day1 + 86_400_000_i64;
        for (idx, ts) in [day1, day1 + 1000, day2, day2 + 1000]
            .into_iter()
            .enumerate()
        {
            olap.write_metrics(&[MetricPoint {
                timestamp_ms: ts,
                name: format!("r24.m.{idx}"),
                value: idx as f64,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "u".into(),
                pid: Some(idx as u32 + 1),
                labels_json: "{}".into(),
            }])
            .unwrap();
        }

        let pinned = olap
            .snapshot_pinned_parquet_files_bounded(
                "metrics",
                None,
                MAX_SNAPSHOT_PARQUET_FILES,
                MAX_SNAPSHOT_METADATA_BYTES,
            )
            .unwrap();
        assert_eq!(pinned.len(), 4);
        assert!(pinned[0].retained_heap_bytes() > 0);
        let cloned = pinned[0].clone();
        assert_eq!(
            pinned[0].file_name().as_ptr(),
            cloned.file_name().as_ptr(),
            "PinnedParquetFile clones must share Arc<str> storage"
        );
        assert_eq!(
            pinned[0]
                .display_path()
                .as_os_str()
                .as_encoded_bytes()
                .as_ptr(),
            cloned
                .display_path()
                .as_os_str()
                .as_encoded_bytes()
                .as_ptr(),
            "PinnedParquetFile clones must share Arc<Path> storage"
        );
        let exact_bytes = fabric_store::snapshot_retained_heap_bytes(&pinned);
        assert!(exact_bytes > 0 && exact_bytes < MAX_SNAPSHOT_METADATA_BYTES);

        let count_err = olap
            .snapshot_pinned_parquet_files_bounded("metrics", None, 3, MAX_SNAPSHOT_METADATA_BYTES)
            .expect_err("4 files must reject when max_files == 3");
        assert!(
            count_err.contains("file count exceeds limit"),
            "unexpected max_files err: {count_err}"
        );

        let bytes_err = olap
            .snapshot_pinned_parquet_files_bounded("metrics", None, 100, exact_bytes - 1)
            .expect_err("snapshot must reject when max_bytes == exact_bytes - 1");
        assert!(
            bytes_err.contains("metadata"),
            "unexpected max_bytes err: {bytes_err}"
        );
        assert_eq!(
            olap.snapshot_pinned_parquet_files_bounded("metrics", None, 100, exact_bytes + 512)
                .unwrap()
                .len(),
            4
        );

        // UnifiedQuerySnapshot pre-reserves MAX_SNAPSHOT_METADATA_BYTES (4 MiB) before discovery,
        // keeps 4 MiB reserved through partition-slice creation, shrinks after register, and
        // ties reservation lifetime to PinnedParquetTable/PinnedParquetExec/blocking readers.
        let pool: Arc<dyn MemoryPool> = Arc::new(GreedyMemoryPool::new(UNIFIED_POOL_BYTES));
        let pressure_res = MemoryConsumer::new("Pressure").register(&pool);
        pressure_res
            .try_grow(UNIFIED_POOL_BYTES - MAX_SNAPSHOT_METADATA_BYTES + 1024)
            .unwrap();
        let snap_budget = Arc::new(SnapshotBudgetGuard::new(
            MemoryConsumer::new("UnifiedQuerySnapshot").register(&pool),
        ));
        let pre_err = discover_bounded_snapshot(&olap, "metrics", None, &snap_budget)
            .expect_err("UnifiedQuerySnapshot 4 MiB pre-reservation must reject with 1 KiB short");
        assert!(
            pre_err.contains("Failed to allocate additional"),
            "unexpected pre-reserve err: {pre_err}"
        );
        assert_eq!(
            pool.reserved(),
            UNIFIED_POOL_BYTES - MAX_SNAPSHOT_METADATA_BYTES + 1024
        );

        pressure_res.shrink(1024);
        assert_eq!(
            pool.reserved(),
            UNIFIED_POOL_BYTES - MAX_SNAPSHOT_METADATA_BYTES
        );
        let discovered = discover_bounded_snapshot(&olap, "metrics", None, &snap_budget)
            .expect("UnifiedQuerySnapshot must succeed with exact 4 MiB headroom");
        assert_eq!(discovered.len(), 4);
        assert_eq!(
            pool.reserved(),
            UNIFIED_POOL_BYTES,
            "UnifiedQuerySnapshot must hold full 4 MiB across partition slice conversion"
        );
        let snap_env = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::clone(&pool))
            .build_arc()
            .unwrap();
        let snap_ctx = SessionContext::new_with_config_rt(
            SessionConfig::new().with_target_partitions(2),
            snap_env,
        );
        register_pinned_parquet_table_with_budget(
            &snap_ctx,
            "t",
            METRICS_SCHEMA.clone(),
            discovered,
            Some(&snap_budget),
        )
        .unwrap();
        let retained_after_register = pool
            .reserved()
            .saturating_sub(UNIFIED_POOL_BYTES - MAX_SNAPSHOT_METADATA_BYTES);
        assert!(
            retained_after_register >= exact_bytes
                && retained_after_register < MAX_SNAPSHOT_METADATA_BYTES,
            "UnifiedQuerySnapshot must shrink after partition conversion to retained bytes ({retained_after_register})"
        );
        // Execute and drop outer snap_budget while stream is still held; reservation stays
        // pinned until stream drops and the blocking reader exits (mid-stream cancel/drop).
        let df = snap_ctx.sql("SELECT timestamp_ms FROM t").await.unwrap();
        let stream = df.execute_stream().await.unwrap();
        drop(snap_budget);
        drop(snap_ctx);
        assert!(
            pool.reserved() >= (UNIFIED_POOL_BYTES - MAX_SNAPSHOT_METADATA_BYTES) + exact_bytes,
            "UnifiedQuerySnapshot must remain reserved while stream/blocking reader may hold files"
        );
        drop(stream);
        tokio::task::yield_now().await;
        for _ in 0..40 {
            if pool.reserved() == UNIFIED_POOL_BYTES - MAX_SNAPSHOT_METADATA_BYTES {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(
            pool.reserved(),
            UNIFIED_POOL_BYTES - MAX_SNAPSHOT_METADATA_BYTES
        );

        // Adversarial .replace-intent.json rejection tests: >16 inputs, duplicate input_ids keys,
        // >256 B replacement/staged_source, and exhausted intent parse budget before decoding.
        let bad_intent_dir = non_tmp_dir();
        let bad_intent_cat = fabric_store::CatalogStore::open(bad_intent_dir.path()).unwrap();
        let bad_intent_olap = OlapParquetStore::new(bad_intent_cat.layout().clone());
        let bad_intent_part = bad_intent_cat
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        fs::create_dir_all(&bad_intent_part).unwrap();
        let many_inputs: Vec<String> = (0..17).map(|i| format!("part-{i}.parquet")).collect();
        let craft_intent = serde_json::json!({
            "replacement": "part-rep.parquet",
            "inputs": many_inputs,
        });
        fs::write(
            bad_intent_part.join(".replace-intent.json"),
            serde_json::to_vec(&craft_intent).unwrap(),
        )
        .unwrap();
        let intent_err = bad_intent_olap
            .snapshot_pinned_parquet_files_bounded(
                "metrics",
                None,
                MAX_SNAPSHOT_PARQUET_FILES,
                MAX_SNAPSHOT_METADATA_BYTES,
            )
            .expect_err("17-input replace intent must reject during bounded serde parse");
        assert!(
            intent_err.contains("input count exceeds limit"),
            "unexpected replace intent err: {intent_err}"
        );

        let dup_keys_json = br#"{"replacement":"part-rep.parquet","inputs":["part-a.parquet"],"input_ids":{"part-a.parquet":{"dev":1,"ino":1},"part-a.parquet":{"dev":1,"ino":2}}}"#;
        fs::write(bad_intent_part.join(".replace-intent.json"), dup_keys_json).unwrap();
        let dup_err = bad_intent_olap
            .snapshot_pinned_parquet_files_bounded(
                "metrics",
                None,
                MAX_SNAPSHOT_PARQUET_FILES,
                MAX_SNAPSHOT_METADATA_BYTES,
            )
            .expect_err("duplicate keys in input_ids must reject during serde parse");
        assert!(
            dup_err.contains("duplicate key"),
            "unexpected duplicate key err: {dup_err}"
        );

        let long_name = format!("{}.parquet", "r".repeat(260));
        let long_rep_json = serde_json::json!({
            "replacement": long_name,
            "inputs": ["part-a.parquet"],
        });
        fs::write(
            bad_intent_part.join(".replace-intent.json"),
            serde_json::to_vec(&long_rep_json).unwrap(),
        )
        .unwrap();
        let long_rep_err = bad_intent_olap
            .snapshot_pinned_parquet_files_bounded(
                "metrics",
                None,
                MAX_SNAPSHOT_PARQUET_FILES,
                MAX_SNAPSHOT_METADATA_BYTES,
            )
            .expect_err(">256 B replacement filename must reject during serde parse");
        assert!(
            long_rep_err.contains("exceeds 256 bytes"),
            "unexpected long replacement err: {long_rep_err}"
        );

        let long_staged_json = serde_json::json!({
            "replacement": "part-rep.parquet",
            "inputs": ["part-a.parquet"],
            "staged_source": format!(".replace-source-{}.staged", "s".repeat(260)),
        });
        fs::write(
            bad_intent_part.join(".replace-intent.json"),
            serde_json::to_vec(&long_staged_json).unwrap(),
        )
        .unwrap();
        let long_staged_err = bad_intent_olap
            .snapshot_pinned_parquet_files_bounded(
                "metrics",
                None,
                MAX_SNAPSHOT_PARQUET_FILES,
                MAX_SNAPSHOT_METADATA_BYTES,
            )
            .expect_err(">256 B staged_source filename must reject during serde parse");
        assert!(
            long_staged_err.contains("exceeds 256 bytes"),
            "unexpected long staged_source err: {long_staged_err}"
        );

        let valid_intent_json = serde_json::json!({
            "replacement": "part-rep.parquet",
            "inputs": ["part-a.parquet"],
        });
        fs::write(
            bad_intent_part.join(".replace-intent.json"),
            serde_json::to_vec(&valid_intent_json).unwrap(),
        )
        .unwrap();
        let budget_err = bad_intent_olap
            .snapshot_pinned_parquet_files_bounded(
                "metrics",
                None,
                MAX_SNAPSHOT_PARQUET_FILES,
                4 * 1024,
            )
            .expect_err("replace intent must reject pre-read when budget < raw + decoded reserve");
        assert!(
            budget_err.contains("replace intent parse budget exceeds"),
            "unexpected intent parse budget err: {budget_err}"
        );

        // >64 partitions exercises SnapshotAnchorTracker overflow accounting cleanly.
        let many_part_dir = non_tmp_dir();
        let many_part_cat = fabric_store::CatalogStore::open(many_part_dir.path()).unwrap();
        let many_part_olap = OlapParquetStore::new(many_part_cat.layout().clone());
        for d in 0..70_i64 {
            many_part_olap
                .write_metrics(&[MetricPoint {
                    timestamp_ms: day1 + d * 86_400_000_i64,
                    name: format!("p70.{d}"),
                    value: d as f64,
                    host: "h".into(),
                    slice: "s".into(),
                    unit: "u".into(),
                    pid: None,
                    labels_json: "{}".into(),
                }])
                .unwrap();
        }
        let many_pinned = many_part_olap
            .snapshot_pinned_parquet_files_bounded(
                "metrics",
                None,
                MAX_SNAPSHOT_PARQUET_FILES,
                MAX_SNAPSHOT_METADATA_BYTES,
            )
            .unwrap();
        assert_eq!(many_pinned.len(), 70);
        let many_exact = snapshot_retained_heap_bytes(&many_pinned);
        assert!(
            many_part_olap
                .snapshot_pinned_parquet_files_bounded("metrics", None, 100, many_exact - 1)
                .is_err()
        );

        // Error during bounded discovery releases UnifiedQuerySnapshot cleanly.
        let sym_dir = non_tmp_dir();
        let sym_cat = fabric_store::CatalogStore::open(sym_dir.path()).unwrap();
        let sym_olap = OlapParquetStore::new(sym_cat.layout().clone());
        let sym_part = sym_cat
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        fs::create_dir_all(&sym_part).unwrap();
        let outside = non_tmp_dir();
        let fake_target = outside.path().join("target.parquet");
        fs::write(&fake_target, b"PAR1").unwrap();
        symlink(&fake_target, sym_part.join("evil.parquet")).unwrap();
        let err_snap_budget = Arc::new(SnapshotBudgetGuard::new(
            MemoryConsumer::new("UnifiedQuerySnapshot").register(&pool),
        ));
        let sym_err = discover_bounded_snapshot(&sym_olap, "metrics", None, &err_snap_budget)
            .expect_err("symlinked parquet must fail discovery");
        assert!(sym_err.contains("symlink"), "unexpected sym err: {sym_err}");
        assert_eq!(
            pool.reserved(),
            UNIFIED_POOL_BYTES - MAX_SNAPSHOT_METADATA_BYTES,
            "failed discovery must release UnifiedQuerySnapshot reservation"
        );
        drop(pressure_res);

        // 2. (#165.1) Live-input peak estimators bound Arrow builders & pre-reserve before build.
        let sample_metrics = vec![MetricPoint {
            timestamp_ms: day1,
            name: "cpu.util".into(),
            value: 0.75,
            host: "node-1".into(),
            slice: "agent.slice".into(),
            unit: "worker.service".into(),
            pid: Some(77),
            labels_json: r#"{"k":"v"}"#.into(),
        }];
        let m_batch = metrics_to_record_batch(&sample_metrics).unwrap();
        assert!(
            estimate_metrics_batch_peak_bytes(&sample_metrics)
                >= retained_record_batch_bytes(&m_batch)
        );

        let sample_fs = vec![FsChurnEntry {
            path: "/data/tmp/a".into(),
            mount_point: "/data".into(),
            size_bytes: 123,
            recent_delta_bytes: -5,
            file_count: 1,
            unlinked_open_fds: 2,
            mtime_unix_ms: 0,
            owner_pid: None,
            owner_comm: None,
            owner_unit: Some("u.service".into()),
            owner_slice: None,
            ..Default::default()
        }];
        let f_batch = fs_churn_to_record_batch(day1, &sample_fs).unwrap();
        assert!(
            estimate_fs_churn_batch_peak_bytes(&sample_fs) >= retained_record_batch_bytes(&f_batch)
        );

        let sample_units = vec![UnitRecord {
            name: "u.service".into(),
            manager: "user".into(),
            slice: "agent.slice".into(),
            active_state: "active".into(),
            sub_state: "running".into(),
            transient: true,
            collect_mode_enabled: false,
            orphaned_reason: Some("orphan".into()),
            main_pid: Some(42),
            memory_current_bytes: 1024,
            cpu_usage_usec: 55,
            populated: true,
        }];
        let u_batch = units_to_record_batch(&sample_units).unwrap();
        assert!(
            estimate_units_batch_peak_bytes(&sample_units) >= retained_record_batch_bytes(&u_batch)
        );

        let sample_agents = vec![
            AgentSessionRecord {
                session_id: "s1".into(),
                agent_name: "codex".into(),
                slice: "agent.slice".into(),
                unit: Some("codex.scope".into()),
                pids: vec![1, 2],
                rss_bytes: 4096,
                write_bytes: 512,
                touched_paths: vec!["/data/a".into(), "/data/b".into()],
            },
            AgentSessionRecord {
                session_id: "s2".into(),
                agent_name: "claude".into(),
                slice: "agent.slice".into(),
                unit: None,
                pids: vec![],
                rss_bytes: 0,
                write_bytes: 0,
                touched_paths: vec![],
            },
        ];
        let a_batch = agents_to_record_batch(&sample_agents).unwrap();
        assert!(
            estimate_agents_batch_peak_bytes(&sample_agents)
                >= retained_record_batch_bytes(&a_batch)
        );
        let touched_col = a_batch
            .column(7)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(touched_col.value(0), "/data/a;/data/b");
        assert_eq!(touched_col.value(1), "");

        // Tight-pool UnifiedQueryLiveInput pre-reservation rejects before batch construction,
        // and 61 MiB live input + 4 MiB UnifiedQuerySnapshot rejects cleanly inside 64 MiB pool.
        let giant_65m = "x".repeat(65 * 1024 * 1024);
        let err_metrics = execute_unified_query(
            dir.path(),
            &UnifiedQuerySpec {
                domain: QueryDomain::Metrics,
                since_ms: None,
                where_clauses: Vec::new(),
                group_by: Vec::new(),
                agg_specs: Vec::new(),
                limit: 10,
            },
            &[MetricPoint {
                timestamp_ms: day1,
                name: giant_65m,
                value: 1.0,
                host: "h".into(),
                slice: "s".into(),
                unit: "u".into(),
                pid: None,
                labels_json: "{}".into(),
            }],
            None,
        )
        .await
        .expect_err("65 MiB live MetricPoint must reject in UnifiedQueryLiveInput pre-reservation");
        assert!(
            err_metrics.contains("Failed to allocate additional"),
            "unexpected oversized live metrics err: {err_metrics}"
        );

        let snap_fs = HostSnapshot {
            timestamp_unix_ms: day1,
            fs_churn: vec![FsChurnEntry {
                path: "p".repeat(65 * 1024 * 1024),
                mount_point: "/data".into(),
                size_bytes: 1,
                recent_delta_bytes: 0,
                file_count: 1,
                unlinked_open_fds: 0,
                mtime_unix_ms: day1,
                ..Default::default()
            }],
            ..Default::default()
        };
        let err_fs = execute_unified_query(
            dir.path(),
            &UnifiedQuerySpec {
                domain: QueryDomain::Fs,
                since_ms: None,
                where_clauses: Vec::new(),
                group_by: Vec::new(),
                agg_specs: Vec::new(),
                limit: 10,
            },
            &[],
            Some(&snap_fs),
        )
        .await
        .expect_err(
            "65 MiB live FsChurnEntry must reject in UnifiedQueryLiveInput pre-reservation",
        );
        drop(snap_fs);
        assert!(
            err_fs.contains("Failed to allocate additional"),
            "unexpected oversized live fs err: {err_fs}"
        );

        let snap_units = HostSnapshot {
            timestamp_unix_ms: day1,
            units: vec![UnitRecord {
                name: "u.service".into(),
                orphaned_reason: Some("o".repeat(65 * 1024 * 1024)),
                ..Default::default()
            }],
            ..Default::default()
        };
        let err_units = execute_unified_query(
            dir.path(),
            &UnifiedQuerySpec {
                domain: QueryDomain::Units,
                since_ms: None,
                where_clauses: Vec::new(),
                group_by: Vec::new(),
                agg_specs: Vec::new(),
                limit: 10,
            },
            &[],
            Some(&snap_units),
        )
        .await
        .expect_err("65 MiB live UnitRecord must reject in UnifiedQueryLiveInput pre-reservation");
        drop(snap_units);
        assert!(
            err_units.contains("Failed to allocate additional"),
            "unexpected oversized live units err: {err_units}"
        );

        let snap_agents = HostSnapshot {
            timestamp_unix_ms: day1,
            agents: vec![AgentSessionRecord {
                session_id: "s1".into(),
                agent_name: "codex".into(),
                slice: "agent.slice".into(),
                touched_paths: vec!["a".repeat(65 * 1024 * 1024)],
                ..Default::default()
            }],
            ..Default::default()
        };
        let err_agents = execute_unified_query(
            dir.path(),
            &UnifiedQuerySpec {
                domain: QueryDomain::Agents,
                since_ms: None,
                where_clauses: Vec::new(),
                group_by: Vec::new(),
                agg_specs: Vec::new(),
                limit: 10,
            },
            &[],
            Some(&snap_agents),
        )
        .await
        .expect_err(
            "65 MiB live AgentSessionRecord must reject in UnifiedQueryLiveInput pre-reservation",
        );
        drop(snap_agents);
        assert!(
            err_agents.contains("Failed to allocate additional"),
            "unexpected oversized live agents err: {err_agents}"
        );

        // 61 MiB live metric fits in UnifiedQueryLiveInput, leaving 3 MiB < 4 MiB
        // UnifiedQuerySnapshot pre-reservation so combined snapshot discovery rejects cleanly.
        let err_combined = execute_unified_query(
            dir.path(),
            &UnifiedQuerySpec {
                domain: QueryDomain::Metrics,
                since_ms: None,
                where_clauses: Vec::new(),
                group_by: Vec::new(),
                agg_specs: Vec::new(),
                limit: 10,
            },
            &[MetricPoint {
                timestamp_ms: day1,
                name: "m".repeat(61 * 1024 * 1024),
                value: 1.0,
                host: "h".into(),
                slice: "s".into(),
                unit: "u".into(),
                pid: None,
                labels_json: "{}".into(),
            }],
            None,
        )
        .await
        .expect_err(
            "61 MiB live input + 4 MiB snapshot pre-reservation must reject on 64 MiB pool",
        );
        assert!(
            err_combined.contains("Failed to allocate additional"),
            "unexpected combined pool pressure err: {err_combined}"
        );
    }
}
