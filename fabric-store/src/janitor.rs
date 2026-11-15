//! Retention janitor enforcing OLAP compaction, >7d downsampling, and `4 GiB` disk cap.

use arrow::array::{Array, Float64Array, Int64Array, StringArray, UInt32Array};
use arrow::compute::concat_batches;
use arrow::record_batch::RecordBatch;
use chrono::{NaiveDate, Utc};
use fabric_core::MetricPoint;
use fabric_core::arrow_schemas::{METRICS_SCHEMA, metrics_to_record_batch};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use crate::olap::{OLAP_DOMAINS, OlapParquetStore, dir_disk_bytes};

/// Default disk cap enforced across `<data_dir>/olap` (`4 GiB`).
pub const DEFAULT_MAX_DISK_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Configuration for `RetentionJanitor`.
#[derive(Debug, Clone, Copy)]
pub struct JanitorConfig {
    pub max_disk_bytes: u64,
    pub downsample_after_days: i64,
    pub compaction_file_threshold: usize,
}

impl Default for JanitorConfig {
    fn default() -> Self {
        Self {
            max_disk_bytes: DEFAULT_MAX_DISK_BYTES,
            downsample_after_days: 7,
            compaction_file_threshold: 8,
        }
    }
}

/// Telemetry emitted after a janitor pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JanitorStats {
    pub total_bytes_before: u64,
    pub total_bytes_after: u64,
    pub compacted_partitions: usize,
    pub downsampled_partitions: usize,
    pub evicted_partitions: usize,
}

/// Executes compaction, 7-day metric downsampling, and oldest-partition eviction.
#[derive(Debug, Clone)]
pub struct RetentionJanitor {
    olap: OlapParquetStore,
    config: JanitorConfig,
}

impl RetentionJanitor {
    #[must_use]
    pub fn new(olap: OlapParquetStore, config: JanitorConfig) -> Self {
        Self { olap, config }
    }

    /// Runs compaction, >7d metric downsampling, and disk quota eviction.
    pub fn run_pass(&self, now_day: Option<NaiveDate>) -> Result<JanitorStats, String> {
        let today = now_day.unwrap_or_else(|| Utc::now().date_naive());
        let total_bytes_before = dir_disk_bytes(&self.olap.layout().olap_dir())?;
        let mut compacted_partitions = 0;
        let mut downsampled_partitions = 0;
        let mut evicted_partitions = 0;

        for &domain in OLAP_DOMAINS {
            let domain_dir = self.olap.layout().olap_domain_dir(domain);
            if !domain_dir.is_dir() {
                continue;
            }
            for (day, part_dir, files) in list_day_partitions(&domain_dir)? {
                let age_days = (today - day).num_days();
                if domain == "metrics"
                    && age_days >= self.config.downsample_after_days
                    && !files.is_empty()
                    && !is_already_downsampled(&files)
                {
                    downsample_metrics_partition(&self.olap, day, &part_dir, &files)?;
                    downsampled_partitions += 1;
                    continue;
                }
                if files.len() > self.config.compaction_file_threshold {
                    compact_partition(&self.olap, domain, day, &files)?;
                    compacted_partitions += 1;
                }
            }
        }

        let mut total_bytes_after = dir_disk_bytes(&self.olap.layout().olap_dir())?;
        if total_bytes_after > self.config.max_disk_bytes {
            let mut candidates: Vec<(NaiveDate, PathBuf)> = Vec::new();
            for &domain in OLAP_DOMAINS {
                let domain_dir = self.olap.layout().olap_domain_dir(domain);
                if !domain_dir.is_dir() {
                    continue;
                }
                for (day, part_dir, _) in list_day_partitions(&domain_dir)? {
                    candidates.push((day, part_dir));
                }
            }
            candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            for (_, part_dir) in candidates {
                if total_bytes_after <= self.config.max_disk_bytes {
                    break;
                }
                let part_bytes = dir_disk_bytes(&part_dir)?;
                if part_dir.exists() {
                    fs::remove_dir_all(&part_dir).map_err(|e| e.to_string())?;
                    total_bytes_after = total_bytes_after.saturating_sub(part_bytes);
                    evicted_partitions += 1;
                }
            }
        }

        Ok(JanitorStats {
            total_bytes_before,
            total_bytes_after,
            compacted_partitions,
            downsampled_partitions,
            evicted_partitions,
        })
    }
}

fn list_day_partitions(
    domain_dir: &Path,
) -> Result<Vec<(NaiveDate, PathBuf, Vec<PathBuf>)>, String> {
    let mut parts = Vec::new();
    for entry in fs::read_dir(domain_dir).map_err(|e| e.to_string())? {
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
        let mut files = Vec::new();
        for f in fs::read_dir(&path).map_err(|e| e.to_string())? {
            let f = f.map_err(|e| e.to_string())?;
            let fp = f.path();
            if fp.is_file() && fp.extension().is_some_and(|e| e == "parquet") {
                files.push(fp);
            }
        }
        files.sort();
        parts.push((day, path, files));
    }
    parts.sort_by_key(|p| p.0);
    Ok(parts)
}

fn is_already_downsampled(files: &[PathBuf]) -> bool {
    files.len() == 1
        && files[0]
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|n| n.starts_with("downsampled-"))
}

fn read_parquet_batches(path: &Path) -> Result<Vec<RecordBatch>, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| e.to_string())?;
    let reader = builder.build().map_err(|e| e.to_string())?;
    reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn compact_partition(
    olap: &OlapParquetStore,
    domain: &str,
    day: NaiveDate,
    files: &[PathBuf],
) -> Result<(), String> {
    let mut all_batches = Vec::new();
    for file in files {
        all_batches.extend(read_parquet_batches(file)?);
    }
    if all_batches.is_empty() {
        return Ok(());
    }
    let schema = all_batches[0].schema();
    let merged = concat_batches(&schema, &all_batches).map_err(|e| e.to_string())?;
    let _new_path = olap.write_batch_to_partition(domain, day, &merged)?;
    for old in files {
        let _ = fs::remove_file(old);
    }
    Ok(())
}

#[derive(Ord, PartialOrd, Eq, PartialEq)]
struct DownsampleKey {
    timestamp_ms: i64,
    name: String,
    host: String,
    slice: String,
    unit: String,
}

struct DownsampleAccum {
    sum: f64,
    count: usize,
    pid: Option<u32>,
}

fn downsample_metrics_partition(
    olap: &OlapParquetStore,
    day: NaiveDate,
    part_dir: &Path,
    files: &[PathBuf],
) -> Result<(), String> {
    let mut buckets: BTreeMap<DownsampleKey, DownsampleAccum> = BTreeMap::new();
    for file in files {
        for batch in read_parquet_batches(file)? {
            let ts = batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or("metrics timestamp_ms column type mismatch")?;
            let names = batch
                .column(1)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or("metrics name column type mismatch")?;
            let vals = batch
                .column(2)
                .as_any()
                .downcast_ref::<Float64Array>()
                .ok_or("metrics value column type mismatch")?;
            let hosts = batch
                .column(3)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or("metrics host column type mismatch")?;
            let slices = batch
                .column(4)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or("metrics slice column type mismatch")?;
            let units = batch
                .column(5)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or("metrics unit column type mismatch")?;
            let pids = batch
                .column(6)
                .as_any()
                .downcast_ref::<UInt32Array>()
                .ok_or("metrics pid column type mismatch")?;

            for row in 0..batch.num_rows() {
                let bucket_ts = (ts.value(row) / 300_000) * 300_000;
                let key = DownsampleKey {
                    timestamp_ms: bucket_ts,
                    name: names.value(row).to_owned(),
                    host: hosts.value(row).to_owned(),
                    slice: slices.value(row).to_owned(),
                    unit: units.value(row).to_owned(),
                };
                let pid = if pids.is_null(row) {
                    None
                } else {
                    Some(pids.value(row))
                };
                let entry = buckets.entry(key).or_insert(DownsampleAccum {
                    sum: 0.0,
                    count: 0,
                    pid,
                });
                entry.sum += vals.value(row);
                entry.count += 1;
            }
        }
    }

    let rolled: Vec<MetricPoint> = buckets
        .into_iter()
        .map(|(k, acc)| MetricPoint {
            timestamp_ms: k.timestamp_ms,
            name: k.name,
            value: acc.sum / (acc.count.max(1) as f64),
            host: k.host,
            slice: k.slice,
            unit: k.unit,
            pid: acc.pid,
            labels_json: r#"{"rollup":"5m"}"#.into(),
        })
        .collect();

    if rolled.is_empty() {
        return Ok(());
    }
    let batch = metrics_to_record_batch(&rolled)?;
    debug_assert_eq!(batch.schema(), *METRICS_SCHEMA);
    let written = olap.write_batch_to_partition("metrics", day, &batch)?;
    let target_name = part_dir.join("downsampled-5m.parquet");
    fs::rename(&written, &target_name).map_err(|e| e.to_string())?;
    for old in files {
        let _ = fs::remove_file(old);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
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

    #[test]
    fn compacts_downsamples_and_evicts_over_quota() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let old_day_ms = (Utc::now() - Duration::days(10)).timestamp_millis();
        let today_ms = Utc::now().timestamp_millis();

        olap.write_metrics(&[
            MetricPoint {
                timestamp_ms: old_day_ms,
                name: "cpu.pct".into(),
                value: 10.0,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "u".into(),
                pid: None,
                labels_json: "{}".into(),
            },
            MetricPoint {
                timestamp_ms: old_day_ms + 1_000,
                name: "cpu.pct".into(),
                value: 20.0,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "u".into(),
                pid: None,
                labels_json: "{}".into(),
            },
        ])
        .unwrap();

        for i in 0..4 {
            olap.write_metrics(&[MetricPoint {
                timestamp_ms: today_ms + i,
                name: "mem.bytes".into(),
                value: i as f64,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "u".into(),
                pid: None,
                labels_json: "{}".into(),
            }])
            .unwrap();
        }

        let janitor = RetentionJanitor::new(
            olap.clone(),
            JanitorConfig {
                max_disk_bytes: 1_000_000,
                downsample_after_days: 7,
                compaction_file_threshold: 2,
            },
        );
        let stats = janitor.run_pass(None).unwrap();
        assert_eq!(stats.downsampled_partitions, 1);
        assert_eq!(stats.compacted_partitions, 1);
        assert_eq!(olap.list_parquet_files("metrics", None).unwrap().len(), 2);

        let tiny_janitor = RetentionJanitor::new(
            olap.clone(),
            JanitorConfig {
                max_disk_bytes: 10,
                downsample_after_days: 7,
                compaction_file_threshold: 8,
            },
        );
        let evicted = tiny_janitor.run_pass(None).unwrap();
        assert!(evicted.evicted_partitions >= 1);
    }
}
