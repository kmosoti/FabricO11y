//! Unified DataFusion/Arrow query engine spanning live state, `HotMetricRing`,
//! and ZSTD date-partitioned Parquet partitions across `metrics`, `logs`,
//! `traces`, `units`, `fs`, and `agents`.

use arrow::array::{
    Array, BooleanArray, Float64Array, Int64Array, StringArray, StringViewArray, UInt32Array,
    UInt64Array,
};
use arrow::datatypes::{DataType, SchemaRef};
use arrow::record_batch::RecordBatch;
use datafusion::execution::memory_pool::GreedyMemoryPool;
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::object_store::limit::LimitStore;
use datafusion::object_store::local::LocalFileSystem;
use datafusion::prelude::*;
use fabric_core::arrow_schemas::{
    AGENTS_SCHEMA, FS_EVENTS_SCHEMA, LOGS_SCHEMA, METRICS_SCHEMA, TRACES_SCHEMA, UNITS_SCHEMA,
    agents_to_record_batch, fs_churn_to_record_batch, metrics_to_record_batch,
    units_to_record_batch,
};
pub use fabric_core::query_spec::{
    QueryDomain, UnifiedQuerySpec, build_unified_sql, parse_duration_ms,
};
use fabric_core::{HostSnapshot, MetricPoint, StorageLayout};
use fabric_store::OlapParquetStore;
use std::path::Path;
use std::sync::Arc;

const UNIFIED_POOL_BYTES: usize = 64 * 1024 * 1024;
const UNIFIED_SPILL_BYTES: u64 = 256 * 1024 * 1024;

/// Output returned by `execute_unified_query` in both JSON and Arrow forms.
#[derive(Debug, Clone)]
pub struct UnifiedQueryResult {
    pub domain: String,
    pub row_count: usize,
    pub rows: Vec<serde_json::Value>,
    pub schema: SchemaRef,
    pub batches: Vec<RecordBatch>,
}

/// Executes a bounded DataFusion/Arrow query over persisted Parquet + live ring/snapshot telemetry.
pub async fn execute_unified_query(
    data_dir: &Path,
    spec: &UnifiedQuerySpec,
    live_metrics: &[MetricPoint],
    live_snapshot: Option<&HostSnapshot>,
) -> Result<UnifiedQueryResult, String> {
    let layout = StorageLayout::new(data_dir).map_err(|e| e.to_string())?;
    let olap = OlapParquetStore::new(layout.clone());
    std::fs::create_dir_all(layout.spill_dir()).map_err(|e| e.to_string())?;

    let env = RuntimeEnvBuilder::new()
        .with_memory_pool(Arc::new(GreedyMemoryPool::new(UNIFIED_POOL_BYTES)))
        .with_temp_file_path(layout.spill_dir())
        .with_max_temp_directory_size(UNIFIED_SPILL_BYTES)
        .build_arc()
        .map_err(|e| e.to_string())?;
    env.register_object_store(
        ObjectStoreUrl::local_filesystem().as_ref(),
        Arc::new(LimitStore::new(LocalFileSystem::new(), 8)),
    );

    let cfg = SessionConfig::new()
        .with_target_partitions(2)
        .set_usize("datafusion.execution.meta_fetch_concurrency", 8);
    let ctx = SessionContext::new_with_config_rt(cfg, env);

    let default_order = match spec.domain {
        QueryDomain::Metrics => {
            register_metrics_table(&ctx, &olap, spec.since_ms, live_metrics).await?;
            Some("timestamp_ms DESC")
        }
        QueryDomain::Logs => {
            register_parquet_or_empty(&ctx, &olap, "logs", spec.since_ms, LOGS_SCHEMA.clone())
                .await?;
            Some("timestamp_ms DESC")
        }
        QueryDomain::Traces => {
            register_parquet_or_empty(&ctx, &olap, "traces", spec.since_ms, TRACES_SCHEMA.clone())
                .await?;
            Some("timestamp_ms DESC")
        }
        QueryDomain::Fs => {
            register_fs_table(&ctx, &olap, spec.since_ms, live_snapshot).await?;
            Some("size_bytes DESC, timestamp_ms DESC")
        }
        QueryDomain::Units => {
            let batch = match live_snapshot {
                Some(snap) => units_to_record_batch(&snap.units)?,
                None => RecordBatch::new_empty(UNITS_SCHEMA.clone()),
            };
            ctx.register_batch("t", batch).map_err(|e| e.to_string())?;
            Some("memory_current_bytes DESC, name ASC")
        }
        QueryDomain::Agents => {
            let batch = match live_snapshot {
                Some(snap) => agents_to_record_batch(&snap.agents)?,
                None => RecordBatch::new_empty(AGENTS_SCHEMA.clone()),
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
    let batches = df.collect().await.map_err(|e| e.to_string())?;
    let rows = batches_to_json_rows(&batches)?;

    Ok(UnifiedQueryResult {
        domain: spec.domain.to_string(),
        row_count: rows.len(),
        rows,
        schema,
        batches,
    })
}

async fn register_parquet_or_empty(
    ctx: &SessionContext,
    olap: &OlapParquetStore,
    domain: &str,
    since_ms: Option<i64>,
    schema: SchemaRef,
) -> Result<(), String> {
    let files = olap.list_parquet_files(domain, since_ms)?;
    if files.is_empty() {
        ctx.register_batch("t", RecordBatch::new_empty(schema))
            .map_err(|e| e.to_string())?;
    } else {
        let paths: Vec<String> = files
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let df = ctx
            .read_parquet(paths, ParquetReadOptions::default().schema(&schema))
            .await
            .map_err(|e| e.to_string())?;
        ctx.register_table("t", df.into_view())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn register_metrics_table(
    ctx: &SessionContext,
    olap: &OlapParquetStore,
    since_ms: Option<i64>,
    live_metrics: &[MetricPoint],
) -> Result<(), String> {
    let schema = METRICS_SCHEMA.clone();
    let files = olap.list_parquet_files("metrics", since_ms)?;
    if files.is_empty() {
        let batch = metrics_to_record_batch(live_metrics)?;
        ctx.register_batch("t", batch).map_err(|e| e.to_string())?;
        return Ok(());
    }

    let paths: Vec<String> = files
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let pq_df = ctx
        .read_parquet(paths, ParquetReadOptions::default().schema(&schema))
        .await
        .map_err(|e| e.to_string())?;
    if live_metrics.is_empty() {
        ctx.register_table("t", pq_df.into_view())
            .map_err(|e| e.to_string())?;
    } else {
        let live_batch = metrics_to_record_batch(live_metrics)?;
        ctx.register_table("t_pq", pq_df.into_view())
            .map_err(|e| e.to_string())?;
        ctx.register_batch("t_live", live_batch)
            .map_err(|e| e.to_string())?;
        let union_df = ctx
            .sql("SELECT * FROM t_pq UNION SELECT * FROM t_live")
            .await
            .map_err(|e| e.to_string())?;
        ctx.register_table("t", union_df.into_view())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn register_fs_table(
    ctx: &SessionContext,
    olap: &OlapParquetStore,
    since_ms: Option<i64>,
    live_snapshot: Option<&HostSnapshot>,
) -> Result<(), String> {
    let schema = FS_EVENTS_SCHEMA.clone();
    let files = olap.list_parquet_files("fs_events", since_ms)?;
    let live_batch = live_snapshot
        .filter(|s| !s.fs_churn.is_empty())
        .map(|s| fs_churn_to_record_batch(s.timestamp_unix_ms, &s.fs_churn))
        .transpose()?;

    match (files.is_empty(), live_batch) {
        (true, None) => {
            ctx.register_batch("t", RecordBatch::new_empty(schema))
                .map_err(|e| e.to_string())?;
        }
        (true, Some(batch)) => {
            ctx.register_batch("t", batch).map_err(|e| e.to_string())?;
        }
        (false, None) => {
            let paths: Vec<String> = files
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            let df = ctx
                .read_parquet(paths, ParquetReadOptions::default().schema(&schema))
                .await
                .map_err(|e| e.to_string())?;
            ctx.register_table("t", df.into_view())
                .map_err(|e| e.to_string())?;
        }
        (false, Some(batch)) => {
            let paths: Vec<String> = files
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            let df = ctx
                .read_parquet(paths, ParquetReadOptions::default().schema(&schema))
                .await
                .map_err(|e| e.to_string())?;
            ctx.register_table("t_pq", df.into_view())
                .map_err(|e| e.to_string())?;
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

fn batches_to_json_rows(batches: &[RecordBatch]) -> Result<Vec<serde_json::Value>, String> {
    let mut rows = Vec::new();
    for batch in batches {
        for row_idx in 0..batch.num_rows() {
            let mut obj = serde_json::Map::new();
            for (field, array) in batch.schema().fields().iter().zip(batch.columns()) {
                let val = if array.is_null(row_idx) {
                    serde_json::Value::Null
                } else {
                    match array.data_type() {
                        DataType::Utf8 => serde_json::Value::String(
                            array
                                .as_any()
                                .downcast_ref::<StringArray>()
                                .ok_or("Utf8 downcast failed")?
                                .value(row_idx)
                                .to_owned(),
                        ),
                        DataType::Utf8View => serde_json::Value::String(
                            array
                                .as_any()
                                .downcast_ref::<StringViewArray>()
                                .ok_or("Utf8View downcast failed")?
                                .value(row_idx)
                                .to_owned(),
                        ),
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
