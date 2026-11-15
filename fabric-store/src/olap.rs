//! ZSTD-compressed Arrow/Parquet OLAP writer and date-partitioned reader discovery.

use arrow::record_batch::RecordBatch;
use chrono::{DateTime, NaiveDate, Utc};
use fabric_core::arrow_schemas::{
    fs_events_to_record_batch, graph_edges_to_record_batch, logs_to_record_batch,
    metrics_to_record_batch, traces_to_record_batch,
};
use fabric_core::{
    FsEventRecord, GraphEdgeRecord, LogRecord, MetricPoint, StorageLayout, TraceSpanRecord,
};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Supported partitioned OLAP domains beneath `<data_dir>/olap`.
pub const OLAP_DOMAINS: &[&str] = &["metrics", "logs", "traces", "fs_events", "edges"];

static PART_SEQ: AtomicU64 = AtomicU64::new(0);

/// Atomic ZSTD Parquet writer & date-partitioned locator.
#[derive(Debug, Clone)]
pub struct OlapParquetStore {
    layout: StorageLayout,
}

impl OlapParquetStore {
    /// Creates an OLAP Parquet store handle using an existing validated `StorageLayout`.
    #[must_use]
    pub fn new(layout: StorageLayout) -> Self {
        Self { layout }
    }

    #[must_use]
    pub fn layout(&self) -> &StorageLayout {
        &self.layout
    }

    /// Writes `MetricPoint` rows grouped by UTC date (`dt=YYYY-MM-DD`) in ZSTD Parquet.
    pub fn write_metrics(&self, rows: &[MetricPoint]) -> Result<Vec<PathBuf>, String> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let mut by_day: std::collections::BTreeMap<NaiveDate, Vec<MetricPoint>> =
            std::collections::BTreeMap::new();
        for row in rows {
            by_day
                .entry(day_from_ms(row.timestamp_ms))
                .or_default()
                .push(row.clone());
        }
        let mut written = Vec::new();
        for (day, group) in by_day {
            let batch = metrics_to_record_batch(&group)?;
            written.push(self.write_batch_to_partition("metrics", day, &batch)?);
        }
        Ok(written)
    }

    /// Writes `LogRecord` rows grouped by UTC date in ZSTD Parquet.
    pub fn write_logs(&self, rows: &[LogRecord]) -> Result<Vec<PathBuf>, String> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let mut by_day: std::collections::BTreeMap<NaiveDate, Vec<LogRecord>> =
            std::collections::BTreeMap::new();
        for row in rows {
            by_day
                .entry(day_from_ms(row.timestamp_ms))
                .or_default()
                .push(row.clone());
        }
        let mut written = Vec::new();
        for (day, group) in by_day {
            let batch = logs_to_record_batch(&group)?;
            written.push(self.write_batch_to_partition("logs", day, &batch)?);
        }
        Ok(written)
    }

    /// Writes `TraceSpanRecord` rows grouped by UTC date in ZSTD Parquet.
    pub fn write_traces(&self, rows: &[TraceSpanRecord]) -> Result<Vec<PathBuf>, String> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let mut by_day: std::collections::BTreeMap<NaiveDate, Vec<TraceSpanRecord>> =
            std::collections::BTreeMap::new();
        for row in rows {
            by_day
                .entry(day_from_ms(row.timestamp_ms))
                .or_default()
                .push(row.clone());
        }
        let mut written = Vec::new();
        for (day, group) in by_day {
            let batch = traces_to_record_batch(&group)?;
            written.push(self.write_batch_to_partition("traces", day, &batch)?);
        }
        Ok(written)
    }

    /// Writes `FsEventRecord` rows grouped by UTC date in ZSTD Parquet.
    pub fn write_fs_events(&self, rows: &[FsEventRecord]) -> Result<Vec<PathBuf>, String> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let mut by_day: std::collections::BTreeMap<NaiveDate, Vec<FsEventRecord>> =
            std::collections::BTreeMap::new();
        for row in rows {
            by_day
                .entry(day_from_ms(row.timestamp_ms))
                .or_default()
                .push(row.clone());
        }
        let mut written = Vec::new();
        for (day, group) in by_day {
            let batch = fs_events_to_record_batch(&group)?;
            written.push(self.write_batch_to_partition("fs_events", day, &batch)?);
        }
        Ok(written)
    }

    /// Writes `GraphEdgeRecord` rows grouped by UTC date in ZSTD Parquet.
    pub fn write_edges(&self, rows: &[GraphEdgeRecord]) -> Result<Vec<PathBuf>, String> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let mut by_day: std::collections::BTreeMap<NaiveDate, Vec<GraphEdgeRecord>> =
            std::collections::BTreeMap::new();
        for row in rows {
            by_day
                .entry(day_from_ms(row.timestamp_ms))
                .or_default()
                .push(row.clone());
        }
        let mut written = Vec::new();
        for (day, group) in by_day {
            let batch = graph_edges_to_record_batch(&group)?;
            written.push(self.write_batch_to_partition("edges", day, &batch)?);
        }
        Ok(written)
    }

    /// Writes one `RecordBatch` atomically to `<data_dir>/olap/<domain>/dt=YYYY-MM-DD/part-*.parquet`.
    pub fn write_batch_to_partition(
        &self,
        domain: &str,
        day: NaiveDate,
        batch: &RecordBatch,
    ) -> Result<PathBuf, String> {
        let part_dir = self
            .layout
            .olap_domain_dir(domain)
            .join(format!("dt={}", day.format("%Y-%m-%d")));
        fs::create_dir_all(&part_dir)
            .map_err(|e| format!("failed creating {}: {e}", part_dir.display()))?;
        fs::create_dir_all(self.layout.staging_dir())
            .map_err(|e| format!("failed creating staging dir: {e}"))?;

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_millis();
        let seq = PART_SEQ.fetch_add(1, Ordering::Relaxed);
        let file_name = format!("part-{now_ms}-{}-{seq}.parquet", std::process::id());
        let staging_path = self.layout.staging_dir().join(&file_name);
        let final_path = part_dir.join(&file_name);

        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(
                ZstdLevel::try_new(3).map_err(|e| e.to_string())?,
            ))
            .build();

        {
            let file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&staging_path)
                .map_err(|e| format!("failed creating staging parquet: {e}"))?;
            let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(props))
                .map_err(|e| e.to_string())?;
            writer.write(batch).map_err(|e| e.to_string())?;
            let file = writer.into_inner().map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
        }
        fs::rename(&staging_path, &final_path).map_err(|e| format!("parquet rename: {e}"))?;
        Ok(final_path)
    }

    /// Discovers Parquet files for `domain`, pruning `dt=YYYY-MM-DD` directories older than `since_ms`.
    pub fn list_parquet_files(
        &self,
        domain: &str,
        since_ms: Option<i64>,
    ) -> Result<Vec<PathBuf>, String> {
        let domain_dir = self.layout.olap_domain_dir(domain);
        if !domain_dir.is_dir() {
            return Ok(Vec::new());
        }
        let min_day = since_ms.map(day_from_ms);
        let mut files = Vec::new();

        for entry in fs::read_dir(&domain_dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(date_str) = name.strip_prefix("dt=") else {
                continue;
            };
            let Ok(day) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") else {
                continue;
            };
            if let Some(cutoff) = min_day
                && day < cutoff
            {
                continue;
            }
            for part in fs::read_dir(&path).map_err(|e| e.to_string())? {
                let part = part.map_err(|e| e.to_string())?;
                let part_path = part.path();
                if part_path.is_file() && part_path.extension().is_some_and(|ext| ext == "parquet")
                {
                    files.push(part_path);
                }
            }
        }
        files.sort();
        Ok(files)
    }
}

/// Converts Unix milliseconds into UTC calendar date.
#[must_use]
pub fn day_from_ms(ms: i64) -> NaiveDate {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_else(Utc::now)
        .date_naive()
}

/// Computes total disk bytes under a directory tree without following symlinks.
pub fn dir_disk_bytes(root: &Path) -> Result<u64, String> {
    if !root.exists() {
        return Ok(0);
    }
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let meta = entry.path().symlink_metadata().map_err(|e| e.to_string())?;
            if meta.is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    Ok(total)
}
