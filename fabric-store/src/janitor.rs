//! Retention janitor enforcing OLAP compaction, >7d downsampling, and `4 GiB` disk cap.

use arrow::array::{Array, Float64Array, Int64Array, StringArray, UInt32Array};
use chrono::{NaiveDate, Utc};
use fabric_core::MetricPoint;
use fabric_core::arrow_schemas::{METRICS_SCHEMA, metrics_to_record_batch};
use parquet::file::reader::ChunkReader;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::olap::{
    CompactionOutcome, FileIdentity, MAX_COMPACT_GROUP_BYTES, MAX_COMPACT_OUTPUT_BYTES,
    MAX_COMPACT_SEALED_FILE_BYTES, MAX_COMPACTED_FILE_ROWS, MAX_REPLACE_FAN_IN_FILES,
    MaintenanceLockGuard, O_DIRECTORY_FLAG, O_NOFOLLOW_FLAG, O_NONBLOCK_FLAG, OLAP_DOMAINS,
    OlapParquetStore, PinnedAuxiliaryFile, PinnedParquetFile, ReplacedInputRecord,
    SupersededUnlinkOutcome, acquire_maintenance_lock, advance_replace_epoch_on_fd,
    inspect_regular_file_identity_in_dir_no_symlink, is_eloop_error, is_enotdir_error,
    is_regular_file_in_dir_no_symlink, load_live_writer_uncommitted_partitions_unlocked,
    next_part_seq, open_anchor_data_dir_if_exists, open_child_dir_nofollow,
    open_olap_partition_dir_nofollow, open_regular_file_in_dir_nofollow, proc_fd_child_path,
    proc_fd_dir_path, reconcile_partition_intent_on_fd, record_replaced_parquet_inputs_on_fd,
    sha256_bytes_hex, unlink_regular_file_in_dir_if_present,
    unlink_regular_file_matching_identity_in_dir, unlink_superseded_file_in_dir,
    validate_parquet_filename, write_atomic_json_in_dir_fd_bounded_pinned,
};
use crate::parquet_guard::open_verified_parquet_batch_reader_from_bytes;

/// Default disk cap enforced across `<data_dir>/olap` (`4 GiB`).
pub const DEFAULT_MAX_DISK_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_DOWNSAMPLE_BUCKETS: usize = 250_000;
const MAX_DOWNSAMPLE_BYTES: usize = 16 * 1024 * 1024;
const MAX_SIDECAR_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_DOWNSAMPLE_INPUT_FILE_BYTES: u64 = 64 * 1024 * 1024;
const DOWNSAMPLE_BATCH_ROWS: usize = 4096;
const MAX_DOWNSAMPLE_OPEN_FDS: usize = MAX_REPLACE_FAN_IN_FILES;

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
    /// Scratch reservation refusal defers quota eviction; other maintenance errors do not.
    pub fn run_pass(&self, now_day: Option<NaiveDate>) -> Result<JanitorStats, String> {
        self.run_pass_excluding(now_day, &BTreeSet::new())
    }

    /// Runs compaction, >7d metric downsampling, and disk quota eviction while skipping
    /// any `(domain, day)` partitions present in `protected_partitions` so unconfirmed
    /// sticky-filename retries cannot have their published files removed mid-flush.
    pub fn run_pass_excluding(
        &self,
        now_day: Option<NaiveDate>,
        protected_partitions: &BTreeSet<(&str, NaiveDate)>,
    ) -> Result<JanitorStats, String> {
        for &(domain, _) in protected_partitions {
            crate::olap::validate_olap_domain(domain)?;
        }
        let maint_guard = acquire_maintenance_lock(self.olap.layout())?;
        let persisted_protected =
            load_live_writer_uncommitted_partitions_unlocked(self.olap.layout(), &maint_guard)?;
        let is_protected = |domain: &str, day: NaiveDate| {
            protected_partitions.contains(&(domain, day))
                || persisted_protected
                    .iter()
                    .any(|(d, dt)| d == domain && *dt == day)
        };
        let today = now_day.unwrap_or_else(|| Utc::now().date_naive());
        let (total_bytes_before, scan_before_errs) = olap_disk_bytes_confined(self.olap.layout())?;
        let mut compacted_partitions = 0;
        let mut downsampled_partitions = 0;
        let mut evicted_partitions = 0;
        let mut first_maintenance_err: Option<String> = scan_before_errs.into_iter().next();
        let mut scratch_refusal: Option<String> = None;

        for &domain in OLAP_DOMAINS {
            let (partitions, part_errors) =
                match list_day_partitions_confined(self.olap.layout(), domain) {
                    Ok(p) => p,
                    Err(e) => {
                        if first_maintenance_err.is_none() {
                            first_maintenance_err = Some(e);
                        }
                        continue;
                    }
                };
            if first_maintenance_err.is_none()
                && let Some(err) = part_errors.into_iter().next()
            {
                first_maintenance_err = Some(err);
            }
            for cand in partitions {
                let day = cand.day;
                let part_dir = cand.part_dir;
                if is_protected(domain, day) {
                    continue;
                }
                let part_fd =
                    match open_olap_partition_dir_nofollow(self.olap.layout(), domain, day) {
                        Ok(Some(fd)) => fd,
                        Ok(None) => continue,
                        Err(e) => {
                            if first_maintenance_err.is_none() {
                                first_maintenance_err = Some(e);
                            }
                            continue;
                        }
                    };
                match part_fd.metadata() {
                    Ok(meta) if meta.dev() == cand.dev && meta.ino() == cand.ino => {}
                    Ok(_) => {
                        if first_maintenance_err.is_none() {
                            first_maintenance_err = Some(format!(
                                "partition dir {} changed concurrently during pass",
                                part_dir.display()
                            ));
                        }
                        continue;
                    }
                    Err(e) => {
                        if first_maintenance_err.is_none() {
                            first_maintenance_err = Some(format!(
                                "failed inspecting partition dir {}: {e}",
                                part_dir.display()
                            ));
                        }
                        continue;
                    }
                }
                let part_fd = Arc::new(part_fd);
                if let Err(e) = reconcile_partition_intent_on_fd(&part_fd, &part_dir, &maint_guard)
                {
                    if first_maintenance_err.is_none() {
                        first_maintenance_err = Some(e);
                    }
                    continue;
                }
                let file_names = match list_partition_parquet_names_on_fd(&part_fd, &part_dir) {
                    Ok(refreshed) => refreshed,
                    Err(e) => {
                        if first_maintenance_err.is_none() {
                            first_maintenance_err = Some(e);
                        }
                        continue;
                    }
                };
                cleanup_orphan_rollup_sidecars_on_fd(&part_fd, &file_names);
                let age_days = (today - day).num_days();
                if domain == "metrics"
                    && age_days >= self.config.downsample_after_days
                    && !file_names.is_empty()
                    && needs_metrics_downsample_on_fd(&part_fd, &file_names)
                {
                    match downsample_metrics_partition(
                        &self.olap,
                        day,
                        &part_dir,
                        &part_fd,
                        &file_names,
                        &maint_guard,
                    ) {
                        Ok(()) => downsampled_partitions += 1,
                        Err(e) => {
                            if first_maintenance_err.is_none() {
                                first_maintenance_err = Some(e);
                            }
                        }
                    }
                    continue;
                }
                let compaction_candidates: Vec<String> = if domain == "metrics" {
                    file_names
                        .into_iter()
                        .filter(|n| !n.starts_with("downsampled-"))
                        .collect()
                } else {
                    file_names
                };
                if compaction_candidates.len() > self.config.compaction_file_threshold {
                    match compact_partition_classified(
                        &self.olap,
                        domain,
                        day,
                        &part_dir,
                        &part_fd,
                        &compaction_candidates,
                        &maint_guard,
                    ) {
                        Ok(CompactionOutcome::Complete(Some(_))) => compacted_partitions += 1,
                        Ok(CompactionOutcome::Complete(None)) => {}
                        Ok(CompactionOutcome::ScratchRefused(error)) => {
                            scratch_refusal = Some(error)
                        }
                        Err(e) => {
                            if first_maintenance_err.is_none() {
                                first_maintenance_err = Some(e);
                            }
                        }
                    }
                }
            }
        }

        let (mut total_bytes_after, scan_after_errs) =
            olap_disk_bytes_confined(self.olap.layout())?;
        if first_maintenance_err.is_none()
            && let Some(err) = scan_after_errs.into_iter().next()
        {
            first_maintenance_err = Some(err);
        }
        if scratch_refusal.is_none()
            && admission_kernel::telemetry::disk_needs_eviction(
                total_bytes_after,
                self.config.max_disk_bytes,
            )
        {
            let mut candidates: Vec<(NaiveDate, &'static str)> = Vec::new();
            for &domain in OLAP_DOMAINS {
                match list_day_partitions_confined(self.olap.layout(), domain) {
                    Ok((parts, part_errors)) => {
                        if first_maintenance_err.is_none()
                            && let Some(err) = part_errors.into_iter().next()
                        {
                            first_maintenance_err = Some(err);
                        }
                        for cand in parts {
                            if !is_protected(domain, cand.day) {
                                candidates.push((cand.day, domain));
                            }
                        }
                    }
                    Err(e) => {
                        if first_maintenance_err.is_none() {
                            first_maintenance_err = Some(e);
                        }
                    }
                }
            }
            candidates.sort();
            for (day, domain) in candidates {
                if !admission_kernel::telemetry::disk_needs_eviction(
                    total_bytes_after,
                    self.config.max_disk_bytes,
                ) {
                    break;
                }
                match evict_partition_dir_confined(self.olap.layout(), domain, day, &maint_guard) {
                    Ok(Some(reclaimed_bytes)) => {
                        total_bytes_after = total_bytes_after.saturating_sub(reclaimed_bytes);
                        evicted_partitions += 1;
                    }
                    Ok(None) => {}
                    Err((partial_reclaimed, e)) => {
                        total_bytes_after = total_bytes_after.saturating_sub(partial_reclaimed);
                        if first_maintenance_err.is_none() {
                            first_maintenance_err = Some(e);
                        }
                    }
                }
            }
        }

        if let Some(err) = scratch_refusal {
            return Err(format!(
                "scratch reservation refused; quota eviction deferred: {err}"
            ));
        }
        if let Some(err) = first_maintenance_err {
            return Err(err);
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

fn proc_fd_child_os_path(
    dir_fd: &File,
    child_name: &OsStr,
    label: &str,
) -> Result<PathBuf, String> {
    let bytes = child_name.as_bytes();
    if bytes.is_empty()
        || bytes == b"."
        || bytes == b".."
        || bytes.contains(&b'/')
        || bytes.contains(&0)
    {
        return Err(format!("invalid {label} {}", child_name.to_string_lossy()));
    }
    let mut raw = OsString::from(format!("/proc/self/fd/{}/", dir_fd.as_raw_fd()));
    raw.push(child_name);
    Ok(PathBuf::from(raw))
}

fn open_child_dir_os_nofollow(
    parent_fd: &File,
    child_name: &OsStr,
    label: &str,
) -> Result<Option<File>, String> {
    let child_proc = proc_fd_child_os_path(parent_fd, child_name, label)?;
    let display_name = child_name.to_string_lossy();
    let dir_file = match OpenOptions::new()
        .read(true)
        .custom_flags(O_DIRECTORY_FLAG | O_NOFOLLOW_FLAG | O_NONBLOCK_FLAG)
        .open(&child_proc)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) if is_eloop_error(&e) => {
            return Err(format!("refusing symlinked {label} {display_name}"));
        }
        Err(e) if is_enotdir_error(&e) => {
            return Err(format!("non-directory {label} {display_name}"));
        }
        Err(e) => return Err(format!("failed opening {label} {display_name}: {e}")),
    };
    let fd_meta = dir_file
        .metadata()
        .map_err(|e| format!("failed inspecting {label} fd {display_name}: {e}"))?;
    if fd_meta.file_type().is_symlink() {
        return Err(format!("refusing symlinked {label} {display_name}"));
    }
    if !fd_meta.is_dir() {
        return Err(format!("non-directory {label} {display_name}"));
    }
    let path_meta = match fs::symlink_metadata(&child_proc) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "{label} {display_name} changed concurrently during open"
            ));
        }
        Err(e) => return Err(format!("failed inspecting {label} {display_name}: {e}")),
    };
    if path_meta.file_type().is_symlink() {
        return Err(format!("refusing symlinked {label} {display_name}"));
    }
    if !path_meta.is_dir() {
        return Err(format!("non-directory {label} {display_name}"));
    }
    if path_meta.dev() != fd_meta.dev() || path_meta.ino() != fd_meta.ino() {
        return Err(format!(
            "{label} {display_name} changed concurrently during open"
        ));
    }
    Ok(Some(dir_file))
}

fn scan_olap_bytes_level(
    dir_fd: File,
    display_dir: PathBuf,
    total: &mut u64,
    errors: &mut Vec<String>,
) -> (File, PathBuf, std::vec::IntoIter<OsString>) {
    let mut subdirs = Vec::new();
    let read_dir = match fs::read_dir(proc_fd_dir_path(&dir_fd)) {
        Ok(rd) => rd,
        Err(e) => {
            errors.push(format!("failed reading {}: {e}", display_dir.display()));
            return (dir_fd, display_dir, subdirs.into_iter());
        }
    };
    for entry_res in read_dir {
        let entry = match entry_res {
            Ok(e) => e,
            Err(e) => {
                errors.push(format!(
                    "failed reading entry in {}: {e}",
                    display_dir.display()
                ));
                continue;
            }
        };
        let fname = entry.file_name();
        if fname.to_str().is_none() {
            errors.push(format!("non-utf8 entry in {}", display_dir.display()));
        }
        let child_proc = match proc_fd_child_os_path(&dir_fd, &fname, "olap entry") {
            Ok(p) => p,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let meta = match fs::symlink_metadata(&child_proc) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                errors.push(format!(
                    "failed inspecting {}/{}: {e}",
                    display_dir.display(),
                    fname.to_string_lossy()
                ));
                continue;
            }
        };
        // Skip symlink entries here as 0 bytes so list_day_partitions_confined can
        // attribute the fault per partition while healthy sibling partitions still evict.
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            subdirs.push(fname);
        } else if meta.is_file() {
            *total = total.saturating_add(meta.len());
        }
    }
    (dir_fd, display_dir, subdirs.into_iter())
}

fn olap_disk_bytes_confined(
    layout: &fabric_core::StorageLayout,
) -> Result<(u64, Vec<String>), String> {
    let Some(data_fd) = open_anchor_data_dir_if_exists(layout)? else {
        return Ok((0, Vec::new()));
    };
    let olap_fd = match open_child_dir_nofollow(&data_fd, "olap", "olap dir") {
        Ok(Some(fd)) => fd,
        Ok(None) => return Ok((0, Vec::new())),
        Err(e) if e.starts_with("non-directory ") => return Ok((0, Vec::new())),
        Err(e) => return Err(e),
    };
    drop(data_fd);
    let mut total = 0u64;
    let mut errors = Vec::new();
    let mut stack = vec![scan_olap_bytes_level(
        olap_fd,
        layout.olap_dir(),
        &mut total,
        &mut errors,
    )];
    while let Some((dir_fd, display_dir, mut subdirs)) = stack.pop() {
        let Some(name) = subdirs.next() else {
            continue;
        };
        let sub_display = display_dir.join(Path::new(&name));
        let sub_res = open_child_dir_os_nofollow(&dir_fd, &name, "olap subdir");
        stack.push((dir_fd, display_dir, subdirs));
        match sub_res {
            Ok(Some(sub_fd)) => {
                stack.push(scan_olap_bytes_level(
                    sub_fd,
                    sub_display,
                    &mut total,
                    &mut errors,
                ));
            }
            Ok(None) => {}
            Err(e) if e.starts_with("non-directory ") => {}
            Err(e) => errors.push(e),
        }
    }
    Ok((total, errors))
}

fn list_partition_parquet_names_on_fd(
    part_fd: &File,
    display_part_dir: &Path,
) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(proc_fd_dir_path(part_fd)).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let fname = entry.file_name();
        let Some(name) = fname.to_str() else {
            return Err(format!(
                "non-utf8 entry in partition {}",
                display_part_dir.display()
            ));
        };
        if Path::new(name).extension().is_some_and(|e| e == "parquet") {
            validate_parquet_filename(name, "parquet file")?;
            let child_proc = proc_fd_child_path(part_fd, name, "parquet file")?;
            match fs::symlink_metadata(&child_proc) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(format!(
                        "failed inspecting parquet file {}: {e}",
                        display_part_dir.join(name).display()
                    ));
                }
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(format!(
                        "refusing symlinked parquet file {}",
                        display_part_dir.join(name).display()
                    ));
                }
                Ok(meta) if !meta.is_file() => {
                    return Err(format!(
                        "non-regular parquet file {}",
                        display_part_dir.join(name).display()
                    ));
                }
                Ok(_) => files.push(name.to_owned()),
            }
        }
    }
    files.sort();
    Ok(files)
}

#[derive(Debug, Clone)]
struct PartitionCandidate {
    day: NaiveDate,
    part_dir: PathBuf,
    dev: u64,
    ino: u64,
}

fn list_day_partitions_confined(
    layout: &fabric_core::StorageLayout,
    domain: &str,
) -> Result<(Vec<PartitionCandidate>, Vec<String>), String> {
    crate::olap::validate_olap_domain(domain)?;
    let Some(data_fd) = open_anchor_data_dir_if_exists(layout)? else {
        return Ok((Vec::new(), Vec::new()));
    };
    let Some(olap_fd) = open_child_dir_nofollow(&data_fd, "olap", "olap dir")? else {
        return Ok((Vec::new(), Vec::new()));
    };
    let Some(domain_fd) = open_child_dir_nofollow(&olap_fd, domain, "domain dir")? else {
        return Ok((Vec::new(), Vec::new()));
    };
    drop(olap_fd);
    drop(data_fd);
    let mut partition_errors = Vec::new();
    let rd = match fs::read_dir(proc_fd_dir_path(&domain_fd)) {
        Ok(rd) => rd,
        Err(e) => {
            partition_errors.push(format!("failed reading domain {domain}: {e}"));
            return Ok((Vec::new(), partition_errors));
        }
    };
    let mut dir_names = Vec::new();
    for entry_res in rd {
        let entry = match entry_res {
            Ok(e) => e,
            Err(e) => {
                partition_errors.push(format!("failed reading entry in domain {domain}: {e}"));
                continue;
            }
        };
        let fname = entry.file_name();
        let Some(name) = fname.to_str() else {
            partition_errors.push(format!("non-utf8 entry in domain {domain}"));
            continue;
        };
        dir_names.push(name.to_owned());
    }
    let mut parts = Vec::new();
    for name in dir_names {
        let Some(date_str) = name.strip_prefix("dt=") else {
            continue;
        };
        let Ok(day) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") else {
            continue;
        };
        let part_fd = match open_child_dir_nofollow(&domain_fd, &name, "partition dir") {
            Ok(Some(f)) => f,
            Ok(None) => continue,
            Err(e) if e.starts_with("non-directory ") => continue,
            Err(e) => {
                partition_errors.push(e);
                continue;
            }
        };
        let part_fd_meta = match part_fd.metadata() {
            Ok(meta) => meta,
            Err(e) => {
                partition_errors.push(format!("failed inspecting partition dir fd {name}: {e}"));
                continue;
            }
        };
        let real_part_dir = layout.olap_domain_dir(domain).join(&name);
        let real_meta = match fs::symlink_metadata(&real_part_dir) {
            Ok(meta) => meta,
            Err(e) => {
                partition_errors.push(format!(
                    "failed inspecting partition dir {}: {e}",
                    real_part_dir.display()
                ));
                continue;
            }
        };
        if real_meta.file_type().is_symlink()
            || !real_meta.is_dir()
            || real_meta.dev() != part_fd_meta.dev()
            || real_meta.ino() != part_fd_meta.ino()
        {
            partition_errors.push(format!(
                "partition dir {} changed concurrently during scan",
                real_part_dir.display()
            ));
            continue;
        }
        parts.push(PartitionCandidate {
            day,
            part_dir: real_part_dir,
            dev: part_fd_meta.dev(),
            ino: part_fd_meta.ino(),
        });
    }
    parts.sort_by_key(|p| p.day);
    Ok((parts, partition_errors))
}

fn evict_partition_dir_confined(
    layout: &fabric_core::StorageLayout,
    domain: &str,
    day: NaiveDate,
    _maint: &MaintenanceLockGuard,
) -> Result<Option<u64>, (u64, String)> {
    if let Err(e) = crate::olap::validate_olap_domain(domain) {
        return Err((0, e));
    }
    let part_name = format!("dt={}", day.format("%Y-%m-%d"));
    let data_fd = crate::olap::open_anchor_data_dir(layout).map_err(|e| (0, e))?;
    let Some(olap_fd) =
        open_child_dir_nofollow(&data_fd, "olap", "olap dir").map_err(|e| (0, e))?
    else {
        return Ok(None);
    };
    let Some(domain_fd) =
        open_child_dir_nofollow(&olap_fd, domain, "domain dir").map_err(|e| (0, e))?
    else {
        return Ok(None);
    };
    let Some(part_fd) =
        open_child_dir_nofollow(&domain_fd, &part_name, "partition dir").map_err(|e| (0, e))?
    else {
        return Ok(None);
    };

    // Pass 1: validate every entry before unlinking anything so an unexpected
    // subdirectory aborts cleanly without leaving the partition half-deleted.
    let mut to_unlink: Vec<(String, Option<u64>)> = Vec::new();
    let entries = fs::read_dir(proc_fd_dir_path(&part_fd)).map_err(|e| (0, e.to_string()))?;
    for entry in entries {
        let entry = entry.map_err(|e| (0, e.to_string()))?;
        let child_os = entry.file_name();
        let child_name = child_os
            .to_str()
            .ok_or_else(|| (0, format!("non-utf8 entry in partition {part_name}")))?;
        let child_proc =
            proc_fd_child_path(&part_fd, child_name, "partition entry").map_err(|e| (0, e))?;
        match fs::symlink_metadata(&child_proc) {
            Ok(meta) => {
                if !meta.file_type().is_symlink() && meta.is_dir() {
                    return Err((
                        0,
                        format!("unexpected subdirectory {child_name} in partition {part_name}"),
                    ));
                }
                let reg_len =
                    (!meta.file_type().is_symlink() && meta.is_file()).then_some(meta.len());
                to_unlink.push((child_name.to_owned(), reg_len));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err((
                    0,
                    format!("failed inspecting {child_name} in partition {part_name}: {e}"),
                ));
            }
        }
    }

    // Pass 2: unlink entries and credit reclaimed regular bytes only after remove_file succeeds.
    let mut reclaimed = 0u64;
    for (child_name, reg_len) in to_unlink {
        let child_proc = proc_fd_child_path(&part_fd, &child_name, "partition entry")
            .map_err(|e| (reclaimed, e))?;
        match fs::remove_file(&child_proc) {
            Ok(()) => {
                if let Some(len) = reg_len {
                    reclaimed = reclaimed.saturating_add(len);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err((
                    reclaimed,
                    format!("failed removing {child_name} in partition {part_name}: {e}"),
                ));
            }
        }
    }
    part_fd.sync_all().map_err(|e| {
        (
            reclaimed,
            format!("failed syncing evicted partition {part_name}: {e}"),
        )
    })?;
    drop(part_fd);
    let part_proc =
        proc_fd_child_path(&domain_fd, &part_name, "partition dir").map_err(|e| (reclaimed, e))?;
    match fs::remove_dir(&part_proc) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err((
                reclaimed,
                format!("failed removing partition dir {part_name}: {e}"),
            ));
        }
    }
    domain_fd.sync_all().map_err(|e| {
        (
            reclaimed,
            format!("failed syncing domain dir {domain}: {e}"),
        )
    })?;
    Ok(Some(reclaimed))
}

fn cleanup_orphan_rollup_sidecars_on_fd(part_fd: &File, active_names: &[String]) {
    let active: HashSet<&str> = active_names.iter().map(String::as_str).collect();
    let Ok(entries) = fs::read_dir(proc_fd_dir_path(part_fd)) else {
        return;
    };
    for entry in entries.flatten() {
        let fname = entry.file_name();
        let Some(name) = fname.to_str() else {
            continue;
        };
        if let Some(rest) = name.strip_prefix(".rollup-counts-")
            && let Some(target_parquet) = rest.strip_suffix(".json")
            && !active.contains(target_parquet)
        {
            let _ = unlink_regular_file_in_dir_if_present(part_fd, name, "rollup sidecar");
        }
    }
}

fn has_rollup_sidecar_on_fd(part_fd: &File, fname: &str) -> bool {
    is_regular_file_in_dir_no_symlink(
        part_fd,
        &format!(".rollup-counts-{fname}.json"),
        "rollup sidecar",
    )
    .unwrap_or(false)
}

fn needs_metrics_downsample_on_fd(part_fd: &File, file_names: &[String]) -> bool {
    let has_raw = file_names.iter().any(|n| !n.starts_with("downsampled-"));
    let weighted_ds_count = file_names
        .iter()
        .filter(|n| n.starts_with("downsampled-") && has_rollup_sidecar_on_fd(part_fd, n))
        .count();
    has_raw || weighted_ds_count > 1
}

fn compact_partition_classified(
    olap: &OlapParquetStore,
    domain: &str,
    day: NaiveDate,
    part_dir: &Path,
    part_fd: &File,
    file_names: &[String],
    maint: &MaintenanceLockGuard,
) -> Result<CompactionOutcome, String> {
    compact_partition_with_limits_classified(
        olap,
        domain,
        day,
        part_dir,
        part_fd,
        file_names,
        MAX_COMPACTED_FILE_ROWS,
        MAX_COMPACT_SEALED_FILE_BYTES,
        MAX_COMPACT_GROUP_BYTES,
        MAX_COMPACT_OUTPUT_BYTES,
        maint,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn compact_partition_with_limits(
    olap: &OlapParquetStore,
    domain: &str,
    day: NaiveDate,
    part_dir: &Path,
    part_fd: &File,
    file_names: &[String],
    max_file_rows: usize,
    max_sealed_bytes: u64,
    max_group_bytes: u64,
    max_output_bytes: u64,
    maint: &MaintenanceLockGuard,
) -> Result<bool, String> {
    match compact_partition_with_limits_classified(
        olap,
        domain,
        day,
        part_dir,
        part_fd,
        file_names,
        max_file_rows,
        max_sealed_bytes,
        max_group_bytes,
        max_output_bytes,
        maint,
    )? {
        CompactionOutcome::Complete(path) => Ok(path.is_some()),
        CompactionOutcome::ScratchRefused(error) => Err(error),
    }
}

#[allow(clippy::too_many_arguments)]
fn compact_partition_with_limits_classified(
    olap: &OlapParquetStore,
    domain: &str,
    day: NaiveDate,
    part_dir: &Path,
    part_fd: &File,
    file_names: &[String],
    max_file_rows: usize,
    max_sealed_bytes: u64,
    max_group_bytes: u64,
    max_output_bytes: u64,
    maint: &MaintenanceLockGuard,
) -> Result<CompactionOutcome, String> {
    let mut unsealed: Vec<(usize, u64, String)> = Vec::with_capacity(file_names.len());
    for fname in file_names {
        let Some((file, meta)) =
            open_regular_file_in_dir_nofollow(part_fd, fname, "compaction input")?
        else {
            continue;
        };
        if meta.len() >= max_sealed_bytes {
            continue;
        }
        let rows = OlapParquetStore::verified_file_num_rows(&file, &part_dir.join(fname))?;
        if rows >= max_file_rows {
            continue;
        }
        unsealed.push((rows, meta.len(), fname.clone()));
    }
    if unsealed.len() < 2 {
        return Ok(CompactionOutcome::Complete(None));
    }
    unsealed.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    if unsealed[0].0.saturating_add(unsealed[1].0) > max_file_rows {
        return Ok(CompactionOutcome::Complete(None));
    }

    let mut selected = Vec::new();
    let mut selected_rows = 0usize;
    let mut selected_bytes = 0u64;
    for (rows, len, fname) in unsealed {
        if selected.len() >= MAX_REPLACE_FAN_IN_FILES
            || (!selected.is_empty()
                && (selected_rows.saturating_add(rows) > max_file_rows
                    || selected_bytes.saturating_add(len) > max_group_bytes))
        {
            break;
        }
        selected_rows = selected_rows.saturating_add(rows);
        selected_bytes = selected_bytes.saturating_add(len);
        selected.push(part_dir.join(fname));
    }
    if selected.len() < 2 {
        return Ok(CompactionOutcome::Complete(None));
    }

    olap.compact_parquet_files_classified(
        domain,
        day,
        &selected,
        max_file_rows,
        max_sealed_bytes,
        max_group_bytes,
        max_output_bytes,
        maint,
    )
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Serialize, Deserialize)]
struct DownsampleKey {
    timestamp_ms: i64,
    name: String,
    host: String,
    slice: String,
    unit: String,
    pid: Option<u32>,
    labels_json: String,
}

#[derive(Serialize, Deserialize)]
struct RollupCountEntry {
    key: DownsampleKey,
    count: u64,
}

#[derive(Serialize)]
struct RollupCountRef<'a> {
    key: &'a DownsampleKey,
    count: u64,
}

struct DownsampleAccum {
    sum: f64,
    count: u64,
}

fn downsample_key_bytes(key: &DownsampleKey) -> usize {
    key.name.len()
        + key.host.len()
        + key.slice.len()
        + key.unit.len()
        + key.labels_json.len()
        + std::mem::size_of::<DownsampleKey>()
        + std::mem::size_of::<DownsampleAccum>()
        + 64
}

fn normalize_rollup_labels(slice: &str, raw: &str) -> String {
    if slice != "spindle" || !raw.contains("\"sequence\"") {
        return raw.to_owned();
    }
    if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(raw)
        && let Some(obj) = val.as_object_mut()
    {
        obj.remove("sequence");
        if let Ok(cleaned) = serde_json::to_string(&val) {
            return cleaned;
        }
    }
    raw.to_owned()
}

struct LoadedRollupCounts {
    map: BTreeMap<DownsampleKey, u64>,
    sidecar: PinnedAuxiliaryFile,
}

fn load_rollup_counts_on_fd(
    part_fd: &File,
    sidecar_name: &str,
    display_part_dir: &Path,
) -> Result<Option<LoadedRollupCounts>, String> {
    let sidecar_display = display_part_dir.join(sidecar_name);
    let Some((file, fd_meta)) =
        open_regular_file_in_dir_nofollow(part_fd, sidecar_name, "rollup sidecar")?
    else {
        return Ok(None);
    };
    if fd_meta.len() > MAX_SIDECAR_FILE_BYTES {
        return Err(format!(
            "rollup sidecar {} size {} exceeds {} bytes",
            sidecar_display.display(),
            fd_meta.len(),
            MAX_SIDECAR_FILE_BYTES
        ));
    }
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_SIDECAR_FILE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| {
            format!(
                "failed reading rollup sidecar {}: {e}",
                sidecar_display.display()
            )
        })?;
    if bytes.len() as u64 > MAX_SIDECAR_FILE_BYTES {
        return Err(format!(
            "rollup sidecar {} read size {} exceeds {} bytes",
            sidecar_display.display(),
            bytes.len(),
            MAX_SIDECAR_FILE_BYTES
        ));
    }
    if bytes.len() as u64 != fd_meta.len() {
        return Err(format!(
            "rollup sidecar {} changed size during read",
            sidecar_display.display()
        ));
    }
    let post_meta = file.metadata().map_err(|e| {
        format!(
            "failed re-inspecting rollup sidecar {}: {e}",
            sidecar_display.display()
        )
    })?;
    if post_meta.len() != bytes.len() as u64 {
        return Err(format!(
            "rollup sidecar {} modified concurrently after read",
            sidecar_display.display()
        ));
    }
    let sha256 = sha256_bytes_hex(&bytes);
    let entries: Vec<RollupCountEntry> = serde_json::from_slice(&bytes)
        .map_err(|e| format!("corrupt {}: {e}", sidecar_display.display()))?;
    let mut map = BTreeMap::new();
    let mut key_bytes: usize = 0;
    for entry in entries {
        if !map.contains_key(&entry.key) {
            let entry_bytes = downsample_key_bytes(&entry.key);
            if map.len() >= MAX_DOWNSAMPLE_BUCKETS
                || key_bytes.saturating_add(entry_bytes) > MAX_DOWNSAMPLE_BYTES
            {
                return Err(format!(
                    "rollup sidecar {} exceeds downsample budget ({MAX_DOWNSAMPLE_BUCKETS} buckets / {MAX_DOWNSAMPLE_BYTES} B)",
                    sidecar_display.display()
                ));
            }
            key_bytes = key_bytes.saturating_add(entry_bytes);
        }
        map.insert(entry.key, entry.count.max(1));
    }
    Ok(Some(LoadedRollupCounts {
        map,
        sidecar: PinnedAuxiliaryFile {
            name: sidecar_name.to_owned(),
            dev: fd_meta.dev(),
            ino: fd_meta.ino(),
            len: fd_meta.len(),
            sha256: Some(sha256),
            _file: Arc::new(file),
        },
    }))
}

fn downsample_metrics_partition_step(
    olap: &OlapParquetStore,
    day: NaiveDate,
    part_dir: &Path,
    part_fd: &Arc<File>,
    file_names: &[String],
    maint: &MaintenanceLockGuard,
) -> Result<bool, String> {
    let mut buckets: BTreeMap<DownsampleKey, DownsampleAccum> = BTreeMap::new();
    let mut retained_bytes: usize = 0;
    let mut extra_remove: Vec<PinnedAuxiliaryFile> = Vec::new();
    let mut merge_inputs: Vec<PinnedParquetFile> = Vec::new();
    let mut slots_used: usize = 0;

    for fname in file_names {
        let is_ds = fname.starts_with("downsampled-");
        let needed_slots = if is_ds {
            if has_rollup_sidecar_on_fd(part_fd, fname) {
                2
            } else {
                // Legacy sidecarless rollup lacks exact sample counts; keep it intact.
                continue;
            }
        } else {
            1
        };
        if !merge_inputs.is_empty()
            && slots_used.saturating_add(needed_slots) > MAX_DOWNSAMPLE_OPEN_FDS
        {
            break;
        }
        let mut pending_sidecar: Option<PinnedAuxiliaryFile> = None;
        let prior_counts = if is_ds {
            let sidecar_name = format!(".rollup-counts-{fname}.json");
            let Some(loaded) = load_rollup_counts_on_fd(part_fd, &sidecar_name, part_dir)? else {
                continue;
            };
            pending_sidecar = Some(loaded.sidecar);
            Some(loaded.map)
        } else {
            None
        };

        let Some((file, file_meta)) =
            open_regular_file_in_dir_nofollow(part_fd, fname, "metrics parquet")?
        else {
            return Err(format!(
                "NotFound: missing metrics parquet {}",
                part_dir.join(fname).display()
            ));
        };
        if file_meta.len() > MAX_DOWNSAMPLE_INPUT_FILE_BYTES {
            return Err(format!(
                "metrics parquet {} size {} exceeds downsample file limit {MAX_DOWNSAMPLE_INPUT_FILE_BYTES}",
                part_dir.join(fname).display(),
                file_meta.len()
            ));
        }
        let exact_len = usize::try_from(file_meta.len()).map_err(|e| e.to_string())?;
        let parquet_bytes =
            <File as ChunkReader>::get_bytes(&file, 0, exact_len).map_err(|e| e.to_string())?;
        let post_meta = file.metadata().map_err(|e| e.to_string())?;
        if post_meta.dev() != file_meta.dev()
            || post_meta.ino() != file_meta.ino()
            || post_meta.len() != file_meta.len()
            || parquet_bytes.len() != exact_len
        {
            return Err(format!(
                "metrics parquet {} changed concurrently while buffering for downsampling",
                part_dir.join(fname).display()
            ));
        }
        let sha256 = sha256_bytes_hex(parquet_bytes.as_ref());
        let file = Arc::new(file);
        if let Some(sidecar_aux) = pending_sidecar {
            extra_remove.push(sidecar_aux);
        }
        merge_inputs.push(PinnedParquetFile::new_pinned_in_partition_with_file(
            Arc::clone(part_fd),
            fname.clone(),
            part_dir.join(fname),
            file_meta.dev(),
            file_meta.ino(),
            file_meta.len(),
            Some(sha256),
            Arc::clone(&file),
        ));
        slots_used = slots_used.saturating_add(needed_slots);
        {
            let reader = open_verified_parquet_batch_reader_from_bytes(
                parquet_bytes,
                Some(&METRICS_SCHEMA),
                DOWNSAMPLE_BATCH_ROWS,
            )?;
            for batch in reader {
                let batch = batch.map_err(|e| e.to_string())?;
                if batch.num_rows() == 0 {
                    continue;
                }
                if batch.num_columns() < 8 {
                    return Err(format!(
                        "metrics batch in {} has {} columns, expected 8",
                        part_dir.join(fname).display(),
                        batch.num_columns()
                    ));
                }
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
                let labels = batch
                    .column(7)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or("metrics labels_json column type mismatch")?;

                for row in 0..batch.num_rows() {
                    let bucket_ts = fabric_core::align_rollup_bucket_ms(ts.value(row));
                    let pid = if pids.is_null(row) {
                        None
                    } else {
                        Some(pids.value(row))
                    };
                    let slice_val = slices.value(row);
                    let key = DownsampleKey {
                        timestamp_ms: bucket_ts,
                        name: names.value(row).to_owned(),
                        host: hosts.value(row).to_owned(),
                        slice: slice_val.to_owned(),
                        unit: units.value(row).to_owned(),
                        pid,
                        labels_json: normalize_rollup_labels(slice_val, labels.value(row)),
                    };
                    let weight = if let Some(ref counts) = prior_counts {
                        counts.get(&key).copied().unwrap_or(1).max(1)
                    } else {
                        1
                    };
                    let weighted_sum = vals.value(row) * (weight as f64);

                    if let Some(entry) = buckets.get_mut(&key) {
                        entry.sum += weighted_sum;
                        entry.count = entry.count.saturating_add(weight);
                    } else {
                        let entry_bytes = downsample_key_bytes(&key);
                        if buckets.len() >= MAX_DOWNSAMPLE_BUCKETS
                            || retained_bytes.saturating_add(entry_bytes) > MAX_DOWNSAMPLE_BYTES
                        {
                            return Err(format!(
                                "metrics partition {} exceeds downsample budget ({MAX_DOWNSAMPLE_BUCKETS} buckets / {MAX_DOWNSAMPLE_BYTES} B)",
                                part_dir.display()
                            ));
                        }
                        retained_bytes = retained_bytes.saturating_add(entry_bytes);
                        buckets.insert(
                            key,
                            DownsampleAccum {
                                sum: weighted_sum,
                                count: weight,
                            },
                        );
                    }
                }
            }
        }
    }

    if buckets.is_empty() {
        if merge_inputs.is_empty() {
            return Ok(false);
        }
        part_fd
            .sync_all()
            .map_err(|e| format!("failed syncing {}: {e}", part_dir.display()))?;
        let replaced_records: Vec<ReplacedInputRecord> = merge_inputs
            .iter()
            .map(|p| ReplacedInputRecord {
                name: p.file_name().to_owned(),
                id: p.file_identity(),
            })
            .collect();
        record_replaced_parquet_inputs_on_fd(part_fd, part_dir, &replaced_records, maint)?;
        let mut mismatch_empty: Option<String> = None;
        for p in &merge_inputs {
            let exp = p.file_identity();
            if matches!(
                unlink_superseded_file_in_dir(
                    part_fd,
                    p.file_name(),
                    Some(&exp),
                    "empty metrics parquet",
                )?,
                SupersededUnlinkOutcome::IdentityMismatch
            ) && mismatch_empty.is_none()
            {
                mismatch_empty = Some(p.file_name().to_owned());
            }
        }
        for aux in &extra_remove {
            let exp = FileIdentity {
                dev: aux.dev,
                ino: aux.ino,
                len: Some(aux.len),
                sha256: aux.sha256.clone(),
            };
            if matches!(
                unlink_superseded_file_in_dir(part_fd, &aux.name, Some(&exp), "rollup sidecar",)?,
                SupersededUnlinkOutcome::IdentityMismatch
            ) && mismatch_empty.is_none()
            {
                mismatch_empty = Some(aux.name.clone());
            }
        }
        part_fd
            .sync_all()
            .map_err(|e| format!("failed syncing {}: {e}", part_dir.display()))?;
        if let Some(bad) = mismatch_empty {
            return Err(format!(
                "empty metrics input {bad} in {} failed identity/digest verification",
                part_dir.display()
            ));
        }
        let _ = advance_replace_epoch_on_fd(part_fd, part_dir, maint)?;
        part_fd
            .sync_all()
            .map_err(|e| format!("failed syncing {}: {e}", part_dir.display()))?;
        return Ok(true);
    }

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let seq = next_part_seq();
    let ds_name = format!(
        "downsampled-5m-{now_ms}-{}-{seq}.parquet",
        std::process::id()
    );
    let sidecar_name = format!(".rollup-counts-{ds_name}.json");

    let count_refs: Vec<RollupCountRef<'_>> = buckets
        .iter()
        .map(|(k, acc)| RollupCountRef {
            key: k,
            count: acc.count.max(1),
        })
        .collect();
    let (_sidecar_file, sidecar_id) = write_atomic_json_in_dir_fd_bounded_pinned(
        part_fd,
        &sidecar_name,
        &count_refs,
        MAX_SIDECAR_FILE_BYTES,
    )?;
    drop(count_refs);

    let rolled: Vec<MetricPoint> = buckets
        .into_iter()
        .map(|(k, acc)| MetricPoint {
            timestamp_ms: k.timestamp_ms,
            name: k.name,
            value: acc.sum / (acc.count.max(1) as f64),
            host: k.host,
            slice: k.slice,
            unit: k.unit,
            pid: k.pid,
            labels_json: k.labels_json,
        })
        .collect();

    let batch_iter = rolled.chunks(DOWNSAMPLE_BATCH_ROWS).map(|chunk| {
        let batch = metrics_to_record_batch(chunk)?;
        debug_assert_eq!(batch.schema(), *METRICS_SCHEMA);
        Ok(batch)
    });

    if let Err(err) = olap.replace_batches_in_pinned_partition(
        "metrics",
        day,
        part_fd,
        &merge_inputs,
        Some(&ds_name),
        &extra_remove,
        batch_iter,
        maint,
    ) {
        if matches!(
            inspect_regular_file_identity_in_dir_no_symlink(
                part_fd,
                &ds_name,
                "downsampled parquet",
            ),
            Ok(None)
        ) {
            let _ = unlink_regular_file_matching_identity_in_dir(
                part_fd,
                &sidecar_name,
                Some(&sidecar_id),
                "rollup sidecar",
            );
        }
        return Err(err);
    }
    Ok(true)
}

fn downsample_metrics_partition(
    olap: &OlapParquetStore,
    day: NaiveDate,
    part_dir: &Path,
    part_fd: &Arc<File>,
    file_names: &[String],
    maint: &MaintenanceLockGuard,
) -> Result<(), String> {
    let max_steps = file_names.len().saturating_add(2);
    let mut cur_names = file_names.to_vec();
    for step in 0..max_steps {
        if step > 0 {
            cur_names = list_partition_parquet_names_on_fd(part_fd, part_dir)?;
        }
        if cur_names.is_empty() || !needs_metrics_downsample_on_fd(part_fd, &cur_names) {
            break;
        }
        if !downsample_metrics_partition_step(olap, day, part_dir, part_fd, &cur_names, maint)? {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olap::{dir_disk_bytes, write_atomic_json_in_dir_bounded};
    use chrono::Duration;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
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

    fn list_partition_parquet_files(part_dir: &Path) -> Result<Vec<PathBuf>, String> {
        let Some(part_fd) = crate::olap::open_standalone_dir_nofollow(part_dir, "partition dir")?
        else {
            return Ok(Vec::new());
        };
        Ok(list_partition_parquet_names_on_fd(&part_fd, part_dir)?
            .into_iter()
            .map(|n| part_dir.join(n))
            .collect())
    }

    fn has_rollup_sidecar(part_dir: &Path, path: &Path) -> bool {
        let Ok(Some(part_fd)) =
            crate::olap::open_standalone_dir_nofollow(part_dir, "partition dir")
        else {
            return false;
        };
        path.file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|fname| has_rollup_sidecar_on_fd(&part_fd, fname))
    }

    #[test]
    fn metric_rollup_drops_raw_spindle_provenance() {
        use fabric_core::{SPINDLE_PROVENANCE_COLUMN, SpindleProvenance};

        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let metric = MetricPoint {
            timestamp_ms: (Utc::now() - Duration::days(10)).timestamp_millis(),
            name: "spindle.receipt-test".into(),
            value: 1.0,
            host: "host".into(),
            slice: "spindle".into(),
            unit: "items".into(),
            pid: None,
            labels_json: r#"{"sequence":4}"#.into(),
        };
        let provenance = SpindleProvenance {
            origin: "local-origin".into(),
            record_id: 9,
            producer: "receipt".into(),
            generation: "boot".into(),
            sequence: 4,
        };
        let mut plan = olap
            .plan_spindle_flush(&[(Some(provenance), metric)], &[], &[])
            .unwrap();
        olap.execute_flush_plan(&mut plan).unwrap();
        let janitor = RetentionJanitor::new(olap.clone(), JanitorConfig::default());
        let stats = janitor.run_pass(None).unwrap();
        assert_eq!(stats.downsampled_partitions, 1);
        let files = olap.list_parquet_files("metrics", None).unwrap();
        assert_eq!(files.len(), 1);
        let reader =
            ParquetRecordBatchReaderBuilder::try_new(File::open(&files[0]).unwrap()).unwrap();
        assert!(
            reader
                .schema()
                .field_with_name(SPINDLE_PROVENANCE_COLUMN)
                .is_err()
        );
    }

    #[test]
    fn scratch_refusal_defers_all_quota_eviction() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let now = Utc::now();
        let point = |offset: i64| MetricPoint {
            timestamp_ms: (now - Duration::days(offset)).timestamp_millis(),
            name: format!("scratch.{offset}"),
            value: 1.0,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        };
        olap.write_metrics(&[point(1)]).unwrap();
        olap.write_metrics(&[point(0)]).unwrap();
        olap.write_metrics(&[point(0)]).unwrap();
        let before = olap
            .list_parquet_files("metrics", None)
            .unwrap()
            .into_iter()
            .map(|p| (p.clone(), fs::read(p).unwrap()))
            .collect::<Vec<_>>();
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                OlapParquetStore::set_test_allocation_refusal(false);
            }
        }
        OlapParquetStore::set_test_allocation_refusal(true);
        let reset = Reset;
        let janitor = RetentionJanitor::new(
            olap.clone(),
            JanitorConfig {
                max_disk_bytes: 1,
                downsample_after_days: 1000,
                compaction_file_threshold: 1,
            },
        );
        let err = janitor.run_pass(Some(now.date_naive())).unwrap_err();
        drop(reset);
        assert!(err.contains("scratch reservation refused; quota eviction deferred"));
        let after = olap
            .list_parquet_files("metrics", None)
            .unwrap()
            .into_iter()
            .map(|p| (p.clone(), fs::read(p).unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(after, before);
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

    #[test]
    fn late_arrival_into_downsampled_partition_preserves_file_and_series() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let old_day = (Utc::now() - Duration::days(10)).date_naive();
        let old_bucket_ms = old_day
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();

        olap.write_metrics(&[
            MetricPoint {
                timestamp_ms: old_bucket_ms,
                name: "proc.rss".into(),
                value: 10.0,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "worker.service".into(),
                pid: Some(101),
                labels_json: r#"{"series":"a"}"#.into(),
            },
            MetricPoint {
                timestamp_ms: old_bucket_ms + 1_000,
                name: "proc.rss".into(),
                value: 20.0,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "worker.service".into(),
                pid: Some(101),
                labels_json: r#"{"series":"a"}"#.into(),
            },
            MetricPoint {
                timestamp_ms: old_bucket_ms,
                name: "proc.rss".into(),
                value: 40.0,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "worker.service".into(),
                pid: Some(102),
                labels_json: r#"{"series":"b"}"#.into(),
            },
        ])
        .unwrap();

        let janitor = RetentionJanitor::new(
            olap.clone(),
            JanitorConfig {
                max_disk_bytes: 10_000_000,
                downsample_after_days: 7,
                compaction_file_threshold: 8,
            },
        );
        let first = janitor.run_pass(None).unwrap();
        assert_eq!(first.downsampled_partitions, 1);

        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join(format!("dt={}", old_day.format("%Y-%m-%d")));
        let first_files = olap.list_parquet_files("metrics", None).unwrap();
        assert_eq!(first_files.len(), 1);
        assert!(
            first_files[0]
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| n.starts_with("downsampled-5m-"))
        );

        // Write late arrivals into the already-downsampled >7d partition:
        // - existing series "a" (100.0, with historical `"sequence"` key stripped on rollup)
        // - new series "c" (99.0)
        olap.write_metrics(&[
            MetricPoint {
                timestamp_ms: old_bucket_ms + 2_000,
                name: "proc.rss".into(),
                value: 100.0,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "worker.service".into(),
                pid: Some(101),
                labels_json: r#"{"sequence":77,"series":"a"}"#.into(),
            },
            MetricPoint {
                timestamp_ms: old_bucket_ms + 2_000,
                name: "proc.rss".into(),
                value: 99.0,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "worker.service".into(),
                pid: Some(103),
                labels_json: r#"{"series":"c"}"#.into(),
            },
        ])
        .unwrap();
        assert_eq!(olap.list_parquet_files("metrics", None).unwrap().len(), 2);

        let second = janitor.run_pass(None).unwrap();
        assert_eq!(second.downsampled_partitions, 1);
        let second_files = olap.list_parquet_files("metrics", None).unwrap();
        assert_eq!(second_files.len(), 1);
        let ds_file = &second_files[0];
        assert!(
            ds_file
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| n.starts_with("downsampled-5m-")),
            "re-downsampling must produce a single consolidated downsampled-5m-*.parquet file"
        );
        assert_eq!(
            list_partition_parquet_files(&part_dir).unwrap().len(),
            1,
            "old downsampled and late raw files must be unlinked after commit"
        );

        let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(ds_file).unwrap())
            .unwrap()
            .build()
            .unwrap();
        let mut seen: BTreeMap<(Option<u32>, String), f64> = BTreeMap::new();
        for batch in reader {
            let batch = batch.unwrap();
            let vals = batch
                .column(2)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap();
            let pids = batch
                .column(6)
                .as_any()
                .downcast_ref::<UInt32Array>()
                .unwrap();
            let labels = batch
                .column(7)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            for row in 0..batch.num_rows() {
                let pid = (!pids.is_null(row)).then(|| pids.value(row));
                seen.insert((pid, labels.value(row).to_owned()), vals.value(row));
            }
        }
        assert_eq!(seen.len(), 3);
        let a_val = *seen
            .get(&(Some(101), r#"{"series":"a"}"#.into()))
            .expect("series a weighted average present");
        assert!(
            (a_val - (10.0 + 20.0 + 100.0) / 3.0).abs() < 1e-9,
            "expected weighted mean 43.3333..., got {a_val}"
        );
        assert_eq!(
            seen.get(&(Some(102), r#"{"series":"b"}"#.into())),
            Some(&40.0)
        );
        assert_eq!(
            seen.get(&(Some(103), r#"{"series":"c"}"#.into())),
            Some(&99.0)
        );
    }

    #[test]
    fn truncated_schema_parquet_fails_cleanly_without_blocking_eviction() {
        use arrow::datatypes::{DataType, Field, Schema};
        use arrow::record_batch::RecordBatch;
        use parquet::arrow::ArrowWriter;
        use std::sync::Arc;

        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let mid_day = (Utc::now() - Duration::days(9)).date_naive();
        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join(format!("dt={}", mid_day.format("%Y-%m-%d")));
        fs::create_dir_all(&part_dir).unwrap();

        // Valid readable 1-column Parquet file (fewer than 8 expected columns).
        let schema = Arc::new(Schema::new(vec![Field::new(
            "only_one_col",
            DataType::Int64,
            false,
        )]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![1_i64, 2_i64]))],
        )
        .unwrap();
        let truncated_path = part_dir.join("part-truncated-schema.parquet");
        let file = File::create(&truncated_path).unwrap();
        let mut writer = ArrowWriter::try_new(file, schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();

        let janitor = RetentionJanitor::new(
            olap,
            JanitorConfig {
                max_disk_bytes: 1,
                downsample_after_days: 7,
                compaction_file_threshold: 8,
            },
        );
        let err = janitor
            .run_pass(None)
            .expect_err("truncated parquet schema must return Err rather than panicking");
        assert!(err.contains("expected 8"), "unexpected error: {err}");
        assert_eq!(
            dir_disk_bytes(&catalog.layout().olap_dir()).unwrap(),
            0,
            "disk quota eviction must still run on schema mismatch"
        );
    }

    #[test]
    fn interrupted_replace_intent_reconciles_without_double_counting() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let old_day = (Utc::now() - Duration::days(9)).date_naive();
        let old_ts = old_day
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        olap.write_metrics(&[MetricPoint {
            timestamp_ms: old_ts,
            name: "cpu".into(),
            value: 10.0,
            host: "h".into(),
            slice: "s".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        }])
        .unwrap();
        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join(format!("dt={}", old_day.format("%Y-%m-%d")));
        let old_file = list_partition_parquet_files(&part_dir).unwrap().remove(0);
        let old_name = old_file.file_name().unwrap().to_str().unwrap().to_owned();
        let new_name = "downsampled-5m-recovered.parquet";
        fs::copy(&old_file, part_dir.join(new_name)).unwrap();
        let intent = format!(r#"{{"replacement":"{new_name}","inputs":["{old_name}"]}}"#);
        fs::write(part_dir.join(".replace-intent.json"), intent).unwrap();

        // Concurrent queries ignore the superseded old file even before janitor recovers.
        let visible = olap.list_parquet_files("metrics", None).unwrap();
        assert_eq!(visible.len(), 1);
        assert!(visible[0].ends_with(new_name));

        // Next janitor pass cleans up superseded old_file and .replace-intent.json.
        let janitor = RetentionJanitor::new(olap.clone(), JanitorConfig::default());
        janitor.run_pass(None).unwrap();
        assert!(!old_file.exists());
        assert!(!part_dir.join(".replace-intent.json").exists());
        assert_eq!(olap.list_parquet_files("metrics", None).unwrap().len(), 1);
    }

    #[test]
    fn corrupt_partition_does_not_block_disk_quota_eviction() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let old_day_ms = (Utc::now() - Duration::days(12)).timestamp_millis();
        let mid_day = (Utc::now() - Duration::days(9)).date_naive();
        olap.write_metrics(&[MetricPoint {
            timestamp_ms: old_day_ms,
            name: "old.metric".into(),
            value: 1.0,
            host: "h".into(),
            slice: "s".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        }])
        .unwrap();

        let bad_part = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join(format!("dt={}", mid_day.format("%Y-%m-%d")));
        fs::create_dir_all(&bad_part).unwrap();
        fs::write(bad_part.join("part-corrupt.parquet"), b"corrupt-bytes").unwrap();

        let janitor = RetentionJanitor::new(
            olap,
            JanitorConfig {
                max_disk_bytes: 1,
                downsample_after_days: 7,
                compaction_file_threshold: 8,
            },
        );
        let err = janitor
            .run_pass(None)
            .expect_err("corrupt parquet error should still be reported");
        assert!(!err.is_empty());
        assert_eq!(
            dir_disk_bytes(&catalog.layout().olap_dir()).unwrap(),
            0,
            "disk quota eviction must still run even when downsampling fails"
        );
    }

    #[test]
    fn non_spindle_sequence_preserved_legacy_sidecarless_untouched_and_oversized_sidecar_bounded() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let old_day = (Utc::now() - Duration::days(10)).date_naive();
        let old_bucket_ms = old_day
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();

        // 1. Non-Spindle metrics must retain distinct `"sequence"` labels across >7d downsampling.
        olap.write_metrics(&[
            MetricPoint {
                timestamp_ms: old_bucket_ms,
                name: "user.custom".into(),
                value: 10.0,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "worker.service".into(),
                pid: Some(201),
                labels_json: r#"{"sequence":1}"#.into(),
            },
            MetricPoint {
                timestamp_ms: old_bucket_ms + 1_000,
                name: "user.custom".into(),
                value: 30.0,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "worker.service".into(),
                pid: Some(201),
                labels_json: r#"{"sequence":2}"#.into(),
            },
        ])
        .unwrap();

        let janitor = RetentionJanitor::new(
            olap.clone(),
            JanitorConfig {
                max_disk_bytes: 10_000_000,
                downsample_after_days: 7,
                compaction_file_threshold: 8,
            },
        );
        let pass1 = janitor.run_pass(None).unwrap();
        assert_eq!(pass1.downsampled_partitions, 1);

        let files1 = olap.list_parquet_files("metrics", None).unwrap();
        assert_eq!(files1.len(), 1);
        let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(&files1[0]).unwrap())
            .unwrap()
            .build()
            .unwrap();
        let row_count: usize = reader.map(|b| b.unwrap().num_rows()).sum();
        assert_eq!(
            row_count, 2,
            "non-spindle metrics with distinct sequence labels must not collapse on rollup"
        );

        // 2. Legacy sidecarless `downsampled-5m-*` file stays intact when late raw arrives.
        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join(format!("dt={}", old_day.format("%Y-%m-%d")));
        let first_ds = &files1[0];
        let first_name = first_ds.file_name().unwrap().to_str().unwrap().to_owned();
        let legacy_path = part_dir.join("downsampled-5m-legacy.parquet");
        fs::rename(first_ds, &legacy_path).unwrap();
        fs::remove_file(part_dir.join(format!(".rollup-counts-{first_name}.json"))).unwrap();

        olap.write_metrics(&[MetricPoint {
            timestamp_ms: old_bucket_ms + 2_000,
            name: "late.raw".into(),
            value: 5.0,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "worker.service".into(),
            pid: Some(202),
            labels_json: "{}".into(),
        }])
        .unwrap();
        let pass2 = janitor.run_pass(None).unwrap();
        assert_eq!(pass2.downsampled_partitions, 1);
        assert!(
            legacy_path.is_file(),
            "legacy sidecarless rollup stays intact"
        );
        let files2 = olap.list_parquet_files("metrics", None).unwrap();
        assert_eq!(files2.len(), 2);

        // Next pass with only 1 legacy sidecarless + 1 sidecar-backed rollup must be a no-op.
        let pass3 = janitor.run_pass(None).unwrap();
        assert_eq!(pass3.downsampled_partitions, 0);

        // 3. Oversized sidecar (>32 MiB) is rejected via metadata before reading into RAM,
        // while max_disk_bytes eviction still reclaims the partition cleanly.
        let ds_backed = files2
            .iter()
            .find(|p| !p.ends_with("downsampled-5m-legacy.parquet"))
            .unwrap();
        let ds_backed_name = ds_backed.file_name().unwrap().to_str().unwrap();
        let sidecar_path = part_dir.join(format!(".rollup-counts-{ds_backed_name}.json"));
        let f = fs::OpenOptions::new()
            .write(true)
            .open(&sidecar_path)
            .unwrap();
        f.set_len(MAX_SIDECAR_FILE_BYTES + 1).unwrap();
        drop(f);

        // Force downsample attempt by dropping another raw file in the same partition.
        olap.write_metrics(&[MetricPoint {
            timestamp_ms: old_bucket_ms + 3_000,
            name: "late.raw".into(),
            value: 7.0,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "worker.service".into(),
            pid: Some(202),
            labels_json: "{}".into(),
        }])
        .unwrap();
        let evicting_janitor = RetentionJanitor::new(
            olap,
            JanitorConfig {
                max_disk_bytes: 1,
                downsample_after_days: 7,
                compaction_file_threshold: 8,
            },
        );
        let err = evicting_janitor
            .run_pass(None)
            .expect_err("oversized sidecar must return Err");
        assert!(err.contains("exceeds"), "unexpected error: {err}");
        assert_eq!(
            dir_disk_bytes(&catalog.layout().olap_dir()).unwrap(),
            0,
            "disk quota eviction must still run on oversized sidecar"
        );
    }

    #[test]
    fn downsampled_metric_files_excluded_from_generic_compaction() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        // Populate > compaction_file_threshold downsampled-5m-* files with sidecars on a
        // recent day (< downsample_after_days); generic compaction must never erase them.
        let recent_day_ms = (Utc::now() - Duration::days(2)).timestamp_millis();
        let recent_day = crate::olap::day_from_ms(recent_day_ms);
        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join(format!("dt={}", recent_day.format("%Y-%m-%d")));
        fs::create_dir_all(&part_dir).unwrap();

        for i in 0..4_u32 {
            let ds_name = format!("downsampled-5m-{i}.parquet");
            let pt = MetricPoint {
                timestamp_ms: recent_day_ms + (i as i64) * 1_000,
                name: format!("ds.metric.{i}"),
                value: i as f64,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: Some(i),
                labels_json: r#"{"series":"s"}"#.into(),
            };
            let batch = metrics_to_record_batch(std::slice::from_ref(&pt)).unwrap();
            olap.write_batches_to_partition(
                "metrics",
                recent_day,
                Some(&ds_name),
                std::iter::once(Ok(batch)),
                true,
            )
            .unwrap();
            let refs = vec![RollupCountEntry {
                key: DownsampleKey {
                    timestamp_ms: pt.timestamp_ms,
                    name: pt.name,
                    host: pt.host,
                    slice: pt.slice,
                    unit: pt.unit,
                    pid: pt.pid,
                    labels_json: pt.labels_json,
                },
                count: 5,
            }];
            write_atomic_json_in_dir_bounded(
                &part_dir,
                &format!(".rollup-counts-{ds_name}.json"),
                &refs,
                MAX_SIDECAR_FILE_BYTES,
            )
            .unwrap();
        }

        let janitor = RetentionJanitor::new(
            olap.clone(),
            JanitorConfig {
                max_disk_bytes: 10_000_000,
                downsample_after_days: 7,
                compaction_file_threshold: 2,
            },
        );
        let stats = janitor.run_pass(None).unwrap();
        assert_eq!(stats.compacted_partitions, 0);
        let remaining = olap.list_parquet_files("metrics", None).unwrap();
        assert_eq!(remaining.len(), 4);
        for f in &remaining {
            assert!(has_rollup_sidecar(&part_dir, f));
        }
    }

    #[test]
    fn rejects_symlinked_domain_and_partition_in_janitor() {
        use std::os::unix::fs::symlink;

        let dir = non_tmp_dir();
        let outside = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let today_ms = Utc::now().timestamp_millis();
        olap.write_metrics(&[MetricPoint {
            timestamp_ms: today_ms,
            name: "keep.quota".into(),
            value: 1.0,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        }])
        .unwrap();

        let logs_dir = catalog.layout().olap_domain_dir("logs");
        fs::remove_dir(&logs_dir).unwrap();
        symlink(outside.path(), &logs_dir).unwrap();

        let janitor = RetentionJanitor::new(
            olap,
            JanitorConfig {
                max_disk_bytes: 1,
                downsample_after_days: 7,
                compaction_file_threshold: 8,
            },
        );
        let err = janitor
            .run_pass(None)
            .expect_err("symlinked domain dir must fail janitor");
        assert!(err.contains("symlink"), "unexpected err: {err}");
        assert_eq!(
            dir_disk_bytes(&catalog.layout().olap_domain_dir("metrics")).unwrap(),
            0,
            "quota eviction must still clear non-symlinked partitions"
        );
    }

    #[test]
    fn run_pass_excluding_protects_pending_partition_while_evicting_older() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let old_ms = 1_690_000_000_000;
        let pending_ms = 1_700_000_000_000;
        let pending_day = crate::olap::day_from_ms(pending_ms);
        let old_day = crate::olap::day_from_ms(old_ms);

        olap.write_metrics(&[MetricPoint {
            timestamp_ms: old_ms,
            name: "old.metric".into(),
            value: 1.0,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        }])
        .unwrap();
        for i in 0..4 {
            olap.write_metrics(&[MetricPoint {
                timestamp_ms: pending_ms + i * 1_000,
                name: format!("pending.metric.{i}"),
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
                max_disk_bytes: 1,
                downsample_after_days: 7,
                compaction_file_threshold: 2,
            },
        );
        let mut protected = BTreeSet::new();
        protected.insert(("metrics", pending_day));
        let stats = janitor.run_pass_excluding(None, &protected).unwrap();
        assert_eq!(stats.compacted_partitions, 0);
        assert_eq!(stats.evicted_partitions, 1);

        let pending_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join(format!("dt={}", pending_day.format("%Y-%m-%d")));
        let old_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join(format!("dt={}", old_day.format("%Y-%m-%d")));
        assert!(
            !old_dir.exists(),
            "unprotected older partition must be evicted"
        );
        assert!(
            pending_dir.exists(),
            "protected pending partition must remain"
        );
        assert_eq!(olap.list_parquet_files("metrics", None).unwrap().len(), 4);
    }

    #[test]
    fn external_janitor_preserves_live_writer_uncommitted_partitions_and_cleans_after_crash() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let writer_guard = crate::try_acquire_writer_lock(catalog.layout()).unwrap();

        let day1_ms = 1_690_000_000_000_i64; // 2023-07-20
        let day2_ms = 1_700_000_000_000_i64; // 2023-11-14
        let make_pt = |ts: i64, name: &str| MetricPoint {
            timestamp_ms: ts,
            name: name.into(),
            value: 1.0,
            host: "h".into(),
            slice: "spindle".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        };
        let pts = vec![
            make_pt(day1_ms, "cross.day1"),
            make_pt(day2_ms, "cross.day2"),
        ];
        let mut plan = olap.plan_flush(&pts, &[], &[], &[], &[]).unwrap();

        let day2_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        fs::write(&day2_dir, b"obstruct").unwrap();
        assert!(
            olap.execute_flush_plan(&mut plan).is_err(),
            "day2 obstruction should fail mid-plan after day1 publishes"
        );
        assert_eq!(olap.list_parquet_files("metrics", None).unwrap().len(), 1);

        let external_janitor = RetentionJanitor::new(
            olap.clone(),
            JanitorConfig {
                max_disk_bytes: 1,
                downsample_after_days: 7,
                compaction_file_threshold: 2,
            },
        );
        let stats_live = external_janitor.run_pass(None).unwrap();
        assert_eq!(
            stats_live.evicted_partitions, 0,
            "external janitor must protect live writer uncommitted partitions"
        );
        assert_eq!(olap.list_parquet_files("metrics", None).unwrap().len(), 1);

        // Dropping the writer lock and live plan simulates writer exit/crash; external janitor may now reclaim.
        drop((writer_guard, plan));
        fs::remove_file(&day2_dir).unwrap();
        let stats_dead = external_janitor.run_pass(None).unwrap();
        assert_eq!(stats_dead.evicted_partitions, 1);
        assert_eq!(
            crate::olap::dir_disk_bytes(&catalog.layout().olap_dir()).unwrap(),
            0
        );
    }

    #[test]
    fn confined_eviction_refuses_symlinked_partition_without_deleting_target() {
        use std::os::unix::fs::symlink;

        let dir = non_tmp_dir();
        let outside = non_tmp_dir();
        let sentinel = outside.path().join("sentinel.txt");
        fs::write(&sentinel, b"do-not-delete").unwrap();

        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let old_ms = 1_690_000_000_000_i64;
        let old_day = crate::olap::day_from_ms(old_ms);
        olap.write_metrics(&[MetricPoint {
            timestamp_ms: old_ms,
            name: "old.metric".into(),
            value: 1.0,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        }])
        .unwrap();

        let sym_part = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join(format!("dt={}", old_day.format("%Y-%m-%d")));
        fs::remove_dir_all(&sym_part).unwrap();
        symlink(outside.path(), &sym_part).unwrap();

        let maint = acquire_maintenance_lock(catalog.layout()).unwrap();
        let (_, evict_err) =
            evict_partition_dir_confined(catalog.layout(), "metrics", old_day, &maint)
                .expect_err("direct confined eviction must refuse symlinked partition");
        assert!(evict_err.contains("symlink"), "unexpected err: {evict_err}");
        drop(maint);

        let janitor = RetentionJanitor::new(
            olap,
            JanitorConfig {
                max_disk_bytes: 1,
                downsample_after_days: 7,
                compaction_file_threshold: 8,
            },
        );
        let pass_err = janitor
            .run_pass(None)
            .expect_err("janitor pass must refuse symlinked partition");
        assert!(pass_err.contains("symlink"), "unexpected err: {pass_err}");
        assert!(
            sentinel.is_file(),
            "sentinel inside symlink target must remain untouched"
        );
    }

    #[test]
    fn downsample_rejects_fifo_nonblocking_and_eviction_counts_only_regular_files() {
        use std::os::unix::fs::symlink;

        let dir = non_tmp_dir();
        let outside = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let old_ms = (Utc::now() - Duration::days(10)).timestamp_millis();
        let old_day = crate::olap::day_from_ms(old_ms);
        let raw_path = olap
            .write_metrics(&[MetricPoint {
                timestamp_ms: old_ms,
                name: "fifo.guard.metric".into(),
                value: 42.0,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: None,
                labels_json: "{}".into(),
            }])
            .unwrap()[0]
            .clone();
        let raw_len = fs::metadata(&raw_path).unwrap().len();
        assert!(raw_len > 0);
        let part_dir = raw_path.parent().unwrap().to_path_buf();

        let fifo_path = part_dir.join("fifo.parquet");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo_path)
            .status()
            .expect("spawn mkfifo");
        assert!(status.success(), "mkfifo succeeded");

        let part_fd = Arc::new(
            crate::olap::open_olap_partition_dir_nofollow(catalog.layout(), "metrics", old_day)
                .unwrap()
                .unwrap(),
        );
        let maint = acquire_maintenance_lock(catalog.layout()).unwrap();
        let ds_err = downsample_metrics_partition(
            &olap,
            old_day,
            &part_dir,
            &part_fd,
            &["fifo.parquet".to_owned()],
            &maint,
        )
        .expect_err("direct downsample must reject FIFO without blocking");
        assert!(ds_err.contains("non-regular"), "unexpected err: {ds_err}");
        drop(maint);
        drop(part_fd);

        let janitor = RetentionJanitor::new(
            olap,
            JanitorConfig {
                max_disk_bytes: DEFAULT_MAX_DISK_BYTES,
                downsample_after_days: 7,
                compaction_file_threshold: 8,
            },
        );
        let pass_err = janitor
            .run_pass(None)
            .expect_err("janitor pass must reject FIFO .parquet without blocking");
        assert!(
            pass_err.contains("non-regular"),
            "unexpected err: {pass_err}"
        );

        let sym_target = outside
            .path()
            .join("long-nonexistent-symlink-target-name-for-size-check");
        let sym_len = fs::symlink_metadata({
            let p = part_dir.join("side-link.txt");
            symlink(&sym_target, &p).unwrap();
            p
        })
        .unwrap()
        .len();
        assert!(sym_len > 0);

        let maint = acquire_maintenance_lock(catalog.layout()).unwrap();
        let reclaimed = evict_partition_dir_confined(catalog.layout(), "metrics", old_day, &maint)
            .unwrap()
            .expect("eviction should return reclaimed regular bytes");
        assert_eq!(
            reclaimed, raw_len,
            "reclaimed bytes must count only regular files and ignore symlinks/FIFOs"
        );
        assert!(!part_dir.exists(), "evicted partition dir must be removed");
    }

    #[test]
    fn symlinked_sibling_partition_still_evicts_healthy_partition_and_subdir_aborts_before_unlink()
    {
        use std::os::unix::fs::symlink;

        let dir = non_tmp_dir();
        let outside = non_tmp_dir();
        let sentinel = outside.path().join("sentinel.txt");
        fs::write(&sentinel, b"do-not-delete").unwrap();

        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let day1_ms = 1_690_000_000_000_i64; // 2023-07-20
        let day2_ms = day1_ms + 86_400_000_i64; // 2023-07-21
        let day1 = crate::olap::day_from_ms(day1_ms);

        let mk_pt = |ts: i64, name: &str| MetricPoint {
            timestamp_ms: ts,
            name: name.into(),
            value: 1.0,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        };
        let p1 = olap
            .write_metrics(&[mk_pt(day1_ms, "sym.sibling")])
            .unwrap()[0]
            .clone();
        let p2 = olap
            .write_metrics(&[mk_pt(day2_ms, "healthy.sibling")])
            .unwrap()[0]
            .clone();
        let sym_part = p1.parent().unwrap().to_path_buf();
        let healthy_part = p2.parent().unwrap().to_path_buf();

        fs::remove_dir_all(&sym_part).unwrap();
        symlink(outside.path(), &sym_part).unwrap();

        let janitor = RetentionJanitor::new(
            olap.clone(),
            JanitorConfig {
                max_disk_bytes: 1,
                downsample_after_days: 7,
                compaction_file_threshold: 8,
            },
        );
        let err = janitor
            .run_pass(None)
            .expect_err("symlinked partition error must still surface after healthy eviction");
        assert!(err.contains("symlink"), "unexpected err: {err}");
        assert!(
            !healthy_part.exists(),
            "healthy sibling partition in the same domain must still be evicted"
        );
        assert!(
            sentinel.is_file(),
            "sentinel inside symlink target must remain untouched"
        );
        fs::remove_file(&sym_part).unwrap();

        // Write a partition containing an unexpected subdirectory and verify two-pass eviction
        // aborts before unlinking any regular Parquet file.
        let p3 = olap
            .write_metrics(&[mk_pt(day1_ms, "subdir.guard")])
            .unwrap()[0]
            .clone();
        let part3 = p3.parent().unwrap().to_path_buf();
        fs::create_dir(part3.join("nested-subdir")).unwrap();
        let maint = acquire_maintenance_lock(catalog.layout()).unwrap();
        let (partial, sub_err) =
            evict_partition_dir_confined(catalog.layout(), "metrics", day1, &maint)
                .expect_err("unexpected subdirectory must abort eviction");
        assert_eq!(
            partial, 0,
            "pre-validation must abort before unlinking any file"
        );
        assert!(
            sub_err.contains("unexpected subdirectory"),
            "unexpected err: {sub_err}"
        );
        assert!(p3.is_file(), "regular parquet file must remain intact");
        assert!(part3.join("nested-subdir").is_dir());
    }

    #[test]
    fn non_utf8_and_unreadable_partition_entries_do_not_block_sibling_compaction_or_eviction() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let old_ms = (Utc::now() - Duration::days(12)).timestamp_millis();
        let recent_ms = Utc::now().timestamp_millis();

        let mk_log = |ts: i64, body: &str| fabric_core::LogRecord {
            timestamp_ms: ts,
            level: "INFO".into(),
            source: "h".into(),
            unit: "svc".into(),
            message: body.into(),
            attributes_json: "{}".into(),
        };

        // Faulty metrics partition with a non-UTF-8 entry inside dt=...
        let fault_pq = olap
            .write_metrics(&[MetricPoint {
                timestamp_ms: recent_ms,
                name: "fault.metric".into(),
                value: 1.0,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "u".into(),
                pid: None,
                labels_json: "{}".into(),
            }])
            .unwrap()[0]
            .clone();

        // Healthy logs partition needing compaction (>2 files) + old traces partition to evict.
        for i in 0..4 {
            olap.write_logs(&[mk_log(recent_ms, &format!("healthy-{i}"))])
                .unwrap();
        }
        let mut lcg = 0x9e37_79b9_7f4a_7c15_u64;
        let incompressible: String = (0..12 * 1024)
            .map(|_| {
                lcg = lcg
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (b'a' + ((lcg >> 32) % 26) as u8) as char
            })
            .collect();
        let old_trace = olap
            .write_traces(&[fabric_core::TraceSpanRecord {
                timestamp_ms: old_ms,
                trace_id: "t1".into(),
                span_id: "s1".into(),
                parent_span_id: None,
                name: "old.span".into(),
                service: "svc".into(),
                duration_ms: 1.0,
                status: "OK".into(),
                attributes_json: format!("{{\"blob\":\"{incompressible}\"}}"),
            }])
            .unwrap()[0]
            .clone();
        let old_len = fs::metadata(&old_trace).unwrap().len();
        assert!(old_len > 10 * 1024 && old_len < 15 * 1024);

        // Also include an unreadable recent partition directory under `traces`; neither the
        // non-UTF-8 metrics fault nor the unreadable trace partition may block compaction of
        // healthy `logs` or eviction of `old_trace`. Measure before corrupt write & chmod so the
        // budget stays exact on both UID 1000 and root UID 0 containers (CAP_DAC_OVERRIDE).
        let unread_trace = olap
            .write_traces(&[fabric_core::TraceSpanRecord {
                timestamp_ms: recent_ms,
                trace_id: "t2".into(),
                span_id: "s2".into(),
                parent_span_id: None,
                name: "unread.span".into(),
                service: "svc".into(),
                duration_ms: 1.0,
                status: "OK".into(),
                attributes_json: "{}".into(),
            }])
            .unwrap()[0]
            .clone();
        let unread_part = unread_trace.parent().unwrap().to_path_buf();
        let total_before_corrupt = dir_disk_bytes(&catalog.layout().olap_dir()).unwrap();
        let non_old_bytes = total_before_corrupt.saturating_sub(old_len);

        let non_utf8_path = fault_pq
            .parent()
            .unwrap()
            .join(OsStr::from_bytes(b"corrupt-\xff.parquet"));
        fs::write(&non_utf8_path, vec![b'z'; 16 * 1024]).unwrap();

        use std::os::unix::fs::PermissionsExt;
        let orig_mode = fs::metadata(&unread_part).unwrap().permissions().mode();
        fs::set_permissions(&unread_part, fs::Permissions::from_mode(0o000)).unwrap();

        let janitor = RetentionJanitor::new(
            olap.clone(),
            JanitorConfig {
                max_disk_bytes: non_old_bytes.saturating_add(16 * 1024 + 2 * 1024),
                downsample_after_days: 7,
                compaction_file_threshold: 2,
            },
        );
        let res = janitor.run_pass(None);
        let _ = fs::set_permissions(&unread_part, fs::Permissions::from_mode(orig_mode));
        let err = res.expect_err("non-utf8 entry error must surface after maintenance");
        assert!(
            err.contains("non-utf8") || err.contains("failed reading"),
            "unexpected err: {err}"
        );
        assert_eq!(
            olap.list_parquet_files("logs", None).unwrap().len(),
            1,
            "healthy logs sibling partition must still compact despite non-utf8/unreadable faults"
        );
        assert!(
            !old_trace.exists(),
            "old healthy traces partition must still be evicted when non-UTF-8 file pushes disk usage over quota"
        );
    }

    #[test]
    fn downsample_metrics_partition_folds_many_small_files_within_fd_budget() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let old_ms = fabric_core::align_rollup_bucket_ms(
            (Utc::now() - Duration::days(10)).timestamp_millis(),
        );
        let old_day = crate::olap::day_from_ms(old_ms);
        for idx in 1..=40_u32 {
            olap.write_metrics(&[MetricPoint {
                timestamp_ms: old_ms + i64::from(idx),
                name: "fanin.rollup".into(),
                value: f64::from(idx),
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "u".into(),
                pid: None,
                labels_json: "{}".into(),
            }])
            .unwrap();
        }
        assert_eq!(olap.list_parquet_files("metrics", None).unwrap().len(), 40);

        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join(format!("dt={}", old_day.format("%Y-%m-%d")));
        // Verify each bounded step folds strictly within MAX_DOWNSAMPLE_OPEN_FDS (40 -> 25 -> 11 -> 1).
        {
            let maint = acquire_maintenance_lock(catalog.layout()).unwrap();
            let part_fd = Arc::new(
                crate::olap::open_olap_partition_dir_nofollow(catalog.layout(), "metrics", old_day)
                    .unwrap()
                    .unwrap(),
            );
            let names1 = list_partition_parquet_names_on_fd(&part_fd, &part_dir).unwrap();
            assert!(
                downsample_metrics_partition_step(
                    &olap, old_day, &part_dir, &part_fd, &names1, &maint
                )
                .unwrap()
            );
            let names2 = list_partition_parquet_names_on_fd(&part_fd, &part_dir).unwrap();
            assert_eq!(names2.len(), 25, "step 1 must consume at most 16 raw files");
            assert!(
                downsample_metrics_partition_step(
                    &olap, old_day, &part_dir, &part_fd, &names2, &maint
                )
                .unwrap()
            );
            let names3 = list_partition_parquet_names_on_fd(&part_fd, &part_dir).unwrap();
            assert_eq!(
                names3.len(),
                11,
                "step 2 must reserve 2 slots for downsampled+sidecar and 14 for raw files"
            );
        }

        let janitor = RetentionJanitor::new(
            olap.clone(),
            JanitorConfig {
                max_disk_bytes: DEFAULT_MAX_DISK_BYTES,
                downsample_after_days: 7,
                compaction_file_threshold: 100,
            },
        );
        let stats = janitor.run_pass(None).unwrap();
        assert_eq!(stats.downsampled_partitions, 1);

        let remaining = list_partition_parquet_files(&part_dir).unwrap();
        assert_eq!(remaining.len(), 1);
        assert!(
            remaining[0]
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("downsampled-5m-"),
            "expected single downsampled file, got {remaining:?}"
        );
        assert!(has_rollup_sidecar(&part_dir, &remaining[0]));

        let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(&remaining[0]).unwrap())
            .unwrap()
            .build()
            .unwrap();
        let batches: Vec<_> = reader.collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_rows(), 1);
        let vals = batches[0]
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!(
            (vals.value(0) - 20.5).abs() < 1e-9,
            "expected weighted average 20.5 across 40 folded files, got {}",
            vals.value(0)
        );
    }

    #[test]
    fn downsample_metrics_partition_consumes_empty_files_ahead_of_nonempty() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let old_ms = fabric_core::align_rollup_bucket_ms(
            (Utc::now() - Duration::days(10)).timestamp_millis(),
        );
        let old_day = crate::olap::day_from_ms(old_ms);
        let empty_batch = metrics_to_record_batch(&[]).unwrap();
        for idx in 0..18_u32 {
            let empty_name = format!("part-0000-{idx:02}.parquet");
            olap.write_batches_to_partition(
                "metrics",
                old_day,
                Some(&empty_name),
                std::iter::once(Ok(empty_batch.clone())),
                true,
            )
            .unwrap()
            .unwrap();
        }
        for val in [10.0_f64, 30.0_f64] {
            olap.write_metrics(&[MetricPoint {
                timestamp_ms: old_ms,
                name: "empty.stall.check".into(),
                value: val,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "u".into(),
                pid: None,
                labels_json: "{}".into(),
            }])
            .unwrap();
        }
        assert_eq!(olap.list_parquet_files("metrics", None).unwrap().len(), 20);

        let janitor = RetentionJanitor::new(
            olap.clone(),
            JanitorConfig {
                max_disk_bytes: DEFAULT_MAX_DISK_BYTES,
                downsample_after_days: 7,
                compaction_file_threshold: 100,
            },
        );
        let stats = janitor.run_pass(None).unwrap();
        assert_eq!(stats.downsampled_partitions, 1);

        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join(format!("dt={}", old_day.format("%Y-%m-%d")));
        let remaining = list_partition_parquet_files(&part_dir).unwrap();
        assert_eq!(remaining.len(), 1);
        assert!(has_rollup_sidecar(&part_dir, &remaining[0]));
        let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(&remaining[0]).unwrap())
            .unwrap()
            .build()
            .unwrap();
        let batches: Vec<_> = reader.collect::<Result<Vec<_>, _>>().unwrap();
        let vals = batches[0]
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((vals.value(0) - 20.0).abs() < 1e-9);
    }

    #[test]
    fn downsample_rejects_malicious_parquet_and_compaction_skips_row_sealed_and_respects_group_cap()
    {
        // 1. Downsampling preflight rejects malformed Thrift wire-type Parquet before decode.
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let old_ms = fabric_core::align_rollup_bucket_ms(
            (Utc::now() - Duration::days(10)).timestamp_millis(),
        );
        let old_day = crate::olap::day_from_ms(old_ms);
        let old_part = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join(format!("dt={}", old_day.format("%Y-%m-%d")));
        fs::create_dir_all(&old_part).unwrap();
        let bad_pq = old_part.join("part-bad-downsample.parquet");
        let diff_footer = [
            0x15_u8, 0x02, 0x19, 0x1c, 0x48, 0x01, b'r', 0x00, 0x16, 0x00, 0x13, 0x00, 0x00,
        ];
        let mut raw = b"PAR1".to_vec();
        raw.extend_from_slice(&diff_footer);
        raw.extend_from_slice(&u32::try_from(diff_footer.len()).unwrap().to_le_bytes());
        raw.extend_from_slice(b"PAR1");
        fs::write(&bad_pq, raw).unwrap();
        let janitor = RetentionJanitor::new(
            olap.clone(),
            JanitorConfig {
                max_disk_bytes: DEFAULT_MAX_DISK_BYTES,
                downsample_after_days: 7,
                compaction_file_threshold: 10,
            },
        );
        let err = janitor
            .run_pass(None)
            .expect_err("downsample preflight must reject malformed Thrift wire type");
        assert!(
            err.contains("wire type"),
            "unexpected downsample err: {err}"
        );

        // 2. Row-sealed front files must not starve later small-row files in compact_partition_with_limits,
        //    and group byte caps bound merged inputs.
        let dir2 = non_tmp_dir();
        let catalog2 = crate::CatalogStore::open(dir2.path()).unwrap();
        let olap2 = OlapParquetStore::new(catalog2.layout().clone());
        let day_ms = Utc::now().timestamp_millis();
        let day = crate::olap::day_from_ms(day_ms);
        let mk_pts = |start: i64, n: i64, blob_len: usize| -> Vec<MetricPoint> {
            (0..n)
                .map(|i| MetricPoint {
                    timestamp_ms: day_ms + start + i,
                    name: format!("c.{start}.{i}"),
                    value: i as f64,
                    host: "h".into(),
                    slice: "agent.slice".into(),
                    unit: "u".into(),
                    pid: None,
                    labels_json: format!("{{\"b\":\"{}\"}}", "x".repeat(blob_len)),
                })
                .collect()
        };

        // Two small-byte, high-row (row-sealed: 5 rows >= max_file_rows=4) files + two 1-row files
        // with larger byte size: old smallest-byte sort starved the 1-row files completely!
        let sealed1 = olap2
            .write_metrics(&mk_pts(0, 5, 4))
            .unwrap()
            .pop()
            .unwrap();
        let sealed2 = olap2
            .write_metrics(&mk_pts(100, 5, 4))
            .unwrap()
            .pop()
            .unwrap();
        let small1 = olap2
            .write_metrics(&mk_pts(200, 1, 2048))
            .unwrap()
            .pop()
            .unwrap();
        let small2 = olap2
            .write_metrics(&mk_pts(300, 1, 2048))
            .unwrap()
            .pop()
            .unwrap();
        assert!(
            fs::metadata(&sealed1).unwrap().len() < fs::metadata(&small1).unwrap().len(),
            "sealed file has fewer bytes so old byte-only sort placed it ahead of 1-row files"
        );
        let part_dir2 = small1.parent().unwrap().to_path_buf();
        let part_fd2 = open_olap_partition_dir_nofollow(catalog2.layout(), "metrics", day)
            .unwrap()
            .unwrap();
        let names = list_partition_parquet_names_on_fd(&part_fd2, &part_dir2).unwrap();
        let maint2 = acquire_maintenance_lock(catalog2.layout()).unwrap();
        assert!(
            compact_partition_with_limits(
                &olap2, "metrics", day, &part_dir2, &part_fd2, &names,
                4, // max_file_rows: sealed1 & sealed2 (5 rows) filtered as sealed
                1_000_000, 1_000_000, 1_000_000, &maint2,
            )
            .unwrap(),
            "row-sealed files must be skipped so the two 1-row files still compact"
        );
        drop(maint2);
        assert!(sealed1.is_file() && sealed2.is_file());
        assert!(!small1.exists() && !small2.exists());
        assert_eq!(
            list_partition_parquet_names_on_fd(&part_fd2, &part_dir2)
                .unwrap()
                .len(),
            3
        );

        // Group byte cap: if remaining unsealed pair exceeds max_group_bytes, refuse merge.
        let s3 = olap2
            .write_metrics(&mk_pts(400, 1, 4096))
            .unwrap()
            .pop()
            .unwrap();
        let s4 = olap2
            .write_metrics(&mk_pts(500, 1, 4096))
            .unwrap()
            .pop()
            .unwrap();
        let names_after = list_partition_parquet_names_on_fd(&part_fd2, &part_dir2).unwrap();
        let maint3 = acquire_maintenance_lock(catalog2.layout()).unwrap();
        assert!(
            !compact_partition_with_limits(
                &olap2,
                "metrics",
                day,
                &part_dir2,
                &part_fd2,
                &names_after,
                100,
                100_000,
                100, // tighter than smallest pair bytes
                100_000,
                &maint3,
            )
            .unwrap(),
            "compact_partition_with_limits must respect max_group_bytes"
        );
        assert!(s3.is_file() && s4.is_file());
    }
}
