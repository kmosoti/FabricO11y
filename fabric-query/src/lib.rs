//! Rebuildable, durable Parquet projection. Canonical custody remains in Fabric.
pub mod transfer;
pub mod unified;
use arrow::{
    array::{Array, ArrayRef, StringArray, UInt64Array},
    datatypes::{DataType, Field, Schema},
    record_batch::RecordBatch,
};
use datafusion::{
    execution::{
        memory_pool::GreedyMemoryPool, object_store::ObjectStoreUrl, runtime_env::RuntimeEnvBuilder,
    },
    object_store::{limit::LimitStore, local::LocalFileSystem},
    prelude::*,
};
use fabric::{
    canonical::CanonicalRow,
    observation::{self, Data, Outcome},
    query::*,
};
use hmac::{Hmac, Mac};
use parquet::{arrow::ArrowWriter, file::properties::WriterProperties, schema::types::ColumnPath};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
pub use unified::probe_spindle_projection;
type Result<T> = std::result::Result<T, String>;
const MANIFEST: &str = "manifest.json";
const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_MANIFEST_DEPTH: usize = 16;
const MAX_APPLY_ROWS: usize = 8192;
const MAX_APPLY_RAW_BYTES: usize = 8 * 1024 * 1024;
const MAX_RECEIPT_SEGMENT_BYTES: u64 = 32 * 1024 * 1024;
const RESERVE: u64 = 64 * 1024;
const POOL: usize = 256 * 1024 * 1024;
const SPILL: u64 = 512 * 1024 * 1024;
const FILE_REQUESTS: usize = 8;
const METADATA_PREFETCH_BYTES: usize = 8 * 1024;
const EXECUTION_THREADS: usize = 2;

fn shared_query_runtime() -> Result<&'static tokio::runtime::Runtime> {
    static RUNTIME: OnceLock<std::result::Result<tokio::runtime::Runtime, String>> =
        OnceLock::new();
    match RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(EXECUTION_THREADS)
            .enable_all()
            .build()
            .map_err(err)
    }) {
        Ok(rt) => Ok(rt),
        Err(e) => Err(e.clone()),
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Segment {
    file: String,
    digest: String,
    bytes: u64,
    rows: u32,
    origin: String,
    runs: Vec<String>,
    min_ns: u64,
    max_ns: u64,
    min_source: u64,
    max_source: u64,
    schemas: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    #[serde(default)]
    authority: Option<String>,
    transform: String,
    progress: ProjectionProgress,
    segments: Vec<Segment>,
    cursor_key: Vec<u8>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    digest: String,
    manifest: Manifest,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "k", content = "v", rename_all = "snake_case")]
enum CursorKey {
    CanonicalId(u64),
    Key(String),
    BucketNs(u64),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    identity: String,
    snapshot: Snapshot,
    #[serde(default)]
    offset: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after: Option<CursorKey>,
    expires: u64,
}

/// Explicitly releases `flock` before closing so sibling `fork()`s do not pin the lock.
struct ExclusiveLock(File);
impl Drop for ExclusiveLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

pub struct Projection {
    dir: PathBuf,
    quota: u64,
    origin_quotas: BTreeMap<String, u64>,
    default_origin_quota: Option<u64>,
    manifest: Manifest,
    _lock: ExclusiveLock,
    active: AtomicBool,
    poisoned: bool,
}
impl Projection {
    pub fn open(dir: impl AsRef<Path>, quota_bytes: u64) -> Result<Self> {
        Self::open_with_origin_quotas(dir, quota_bytes, BTreeMap::new(), None)
    }

    pub fn open_with_origin_quotas(
        dir: impl AsRef<Path>,
        quota_bytes: u64,
        origin_quotas: BTreeMap<String, u64>,
        default_origin_quota: Option<u64>,
    ) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        ensure_private_directory(&dir)?;
        let lock_path = dir.join("projection.lock");
        regular_metadata(&lock_path)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)
            .map_err(err)?;
        enforce_private_file_mode(&lock, "projection.lock")?;
        lock.try_lock()
            .map_err(|e| format!("projection already open: {e}"))?;
        let lock = ExclusiveLock(lock);
        let manifest_path = dir.join(MANIFEST);
        let manifest = if let Some(metadata) = regular_metadata(&manifest_path)? {
            if metadata.len() > MAX_MANIFEST_BYTES as u64
                || metadata
                    .len()
                    .checked_add(RESERVE)
                    .is_none_or(|n| n > quota_bytes)
            {
                return Err("projection manifest byte limit or quota exceeded".into());
            }
            let bytes = read_manifest(&manifest_path, metadata.len() as usize)?;
            observation::check_json_depth(&bytes, MAX_MANIFEST_BYTES, MAX_MANIFEST_DEPTH)?;
            let envelope: Envelope = serde_json::from_slice(&bytes).map_err(err)?;
            if manifest_digest(&envelope.manifest, MAX_MANIFEST_BYTES)? != envelope.digest
                || envelope.manifest.version != 1
                || envelope.manifest.transform != TRANSFORM_VERSION
                || envelope.manifest.cursor_key.len() != 32
            {
                return Err("projection manifest corrupt or incompatible; rebuild required".into());
            }
            let expected = envelope.manifest.segments.iter().try_fold(
                metadata.len() + RESERVE,
                |total, segment| {
                    total
                        .checked_add(segment.bytes)
                        .ok_or("projection size overflow")
                },
            )?;
            if expected > quota_bytes {
                return Err("projection manifest storage exceeds quota".into());
            }
            for segment in &envelope.manifest.segments {
                if !valid_segment_name(&segment.file) {
                    return Err("invalid projection segment path".into());
                }
                let path = dir.join(&segment.file);
                let metadata = regular_metadata(&path)?
                    .ok_or("projection segment missing; rebuild required")?;
                if metadata.len() != segment.bytes || file_digest(&path)? != segment.digest {
                    return Err("projection segment corrupt; rebuild required".into());
                }
            }
            envelope.manifest
        } else {
            let mut key = vec![0; 32];
            File::open("/dev/urandom")
                .map_err(err)?
                .read_exact(&mut key)
                .map_err(err)?;
            Manifest {
                version: 1,
                authority: None,
                transform: TRANSFORM_VERSION.into(),
                progress: ProjectionProgress::default(),
                segments: vec![],
                cursor_key: key,
            }
        };
        let projection = Self {
            dir,
            quota: quota_bytes,
            origin_quotas,
            default_origin_quota,
            manifest,
            _lock: lock,
            active: AtomicBool::new(false),
            poisoned: false,
        };
        projection.publish(&projection.manifest)?;
        // Files absent from the durable manifest are interrupted publication debris.
        let live: BTreeSet<_> = projection
            .manifest
            .segments
            .iter()
            .map(|s| s.file.as_str())
            .collect();
        for entry in fs::read_dir(&projection.dir).map_err(err)? {
            let entry = entry.map_err(err)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if (valid_segment_name(&name) && !live.contains(name.as_ref()))
                || name.starts_with("staging-")
            {
                fs::remove_file(entry.path()).map_err(err)?;
            }
        }
        Ok(projection)
    }
    /// Per-origin segment byte budget, falling back to `default_origin_quota` when omitted.
    fn origin_quota(&self, origin: &str) -> Option<u64> {
        self.origin_quotas
            .get(origin)
            .copied()
            .or(self.default_origin_quota)
    }

    /// Bind replay state to its canonical authority before accepting any rows.
    /// Reusing a projection for another ledger requires an explicit rebuild.
    pub fn bind_authority(&mut self, authority: &str) -> Result<()> {
        if self.poisoned {
            return Err("projection publication uncertain; reopen required".into());
        }
        if authority.is_empty() || authority.len() > 256 {
            return Err("invalid canonical authority".into());
        }
        match &self.manifest.authority {
            Some(existing) if existing == authority => return Ok(()),
            Some(_) => {
                return Err(
                    "projection belongs to another canonical authority; rebuild required".into(),
                );
            }
            None if self.manifest.progress.checkpoint != 0 => {
                return Err("unbound populated projection; rebuild required".into());
            }
            None => {}
        }
        let mut next = self.manifest.clone();
        next.authority = Some(authority.into());
        if let Err((e, post_rename_uncertain)) = self.publish_checked(&next) {
            if post_rename_uncertain {
                self.poisoned = true;
            }
            return Err(e);
        }
        self.manifest = next;
        Ok(())
    }
    pub fn progress(&self) -> ProjectionProgress {
        self.manifest.progress.clone()
    }
    fn scoped_progress(&self, scope: &[String]) -> ProjectionProgress {
        let mut progress = self.progress();
        progress
            .frontiers
            .retain(|origin, _| scope.contains(origin));
        progress
    }

    /// Returns only the projection frontier for the selected origin.
    pub fn source_frontier(&self, origin: &str) -> Result<u64> {
        if self.poisoned {
            return Err("projection publication uncertain; reopen required".into());
        }
        Ok(self
            .manifest
            .progress
            .frontiers
            .get(origin)
            .copied()
            .unwrap_or(0))
    }

    /// Confirms one raw canonical receipt against its full typed row in the
    /// committed manifest. It reads one bounded segment and never spills.
    pub fn proves_projected_row(
        &self,
        row: &CanonicalRow,
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> Result<bool> {
        let check_budget = || -> Result<()> {
            if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
                Err("receipt projection inspection interrupted".into())
            } else {
                Ok(())
            }
        };
        check_budget()?;
        if row.source_id > self.source_frontier(&row.origin)? {
            return Ok(false);
        }
        let mut segments = self.manifest.segments.iter().filter(|segment| {
            segment.origin == row.origin
                && segment.min_source <= row.source_id
                && row.source_id <= segment.max_source
        });
        let Some(segment) = segments.next() else {
            return Ok(false);
        };
        if segments.next().is_some()
            || segment.bytes == 0
            || segment.bytes > MAX_RECEIPT_SEGMENT_BYTES
            || segment.rows as usize > MAX_APPLY_ROWS
        {
            return Err("ambiguous or oversized receipt projection segment".into());
        }
        let path = self.dir.join(&segment.file);
        use rustix::fs::{Mode, OFlags};
        let handle = rustix::fs::open(
            &path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(err)?;
        let mut file = File::from(handle);
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata().map_err(err)?;
        if !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.len() != segment.bytes
        {
            return Err("receipt projection segment metadata mismatch".into());
        }

        // Verify the bytes of the same descriptor passed to the guarded decoder.
        let mut hasher = Sha256::new();
        let mut bytes = Read::by_ref(&mut file).take(segment.bytes);
        let mut buffer = [0; 64 * 1024];
        loop {
            check_budget()?;
            let count = bytes.read(&mut buffer).map_err(err)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        if bytes.limit() != 0 || hex(&hasher.finalize()) != segment.digest {
            return Err("receipt projection segment digest mismatch".into());
        }
        file.rewind().map_err(err)?;
        check_budget()?;
        let expected = to_batch(&[TypedRow::decode(row)?])?;
        let reader = fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_file(
            file,
            Some(&schema()),
            128,
        )?;
        let mut matched = false;
        let mut total_rows = 0;
        for batch in reader {
            check_budget()?;
            let batch = batch.map_err(err)?;
            total_rows += batch.num_rows();
            if total_rows > segment.rows as usize {
                return Err("receipt projection segment row count mismatch".into());
            }
            let source_ids = batch
                .column_by_name("source_id")
                .and_then(|column| column.as_any().downcast_ref::<UInt64Array>())
                .ok_or("invalid projected source ID column")?;
            for (index, source_id) in source_ids.iter().enumerate() {
                if source_id == Some(row.source_id) {
                    if matched || batch.slice(index, 1) != expected {
                        return Err("receipt projection row mismatch".into());
                    }
                    matched = true;
                }
            }
        }
        check_budget()?;
        if total_rows != segment.rows as usize {
            return Err("receipt projection segment row count mismatch".into());
        }
        Ok(matched)
    }

    /// Prove each requested canonical row against a single locked manifest.
    /// Each segment is hashed and decoded once; limits apply to the entire call.
    pub fn proves_projected_rows(
        &self,
        rows: &[CanonicalRow],
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> (Vec<Result<bool>>, u64, u64, u64) {
        let mut results = vec![Err("projection evidence unavailable".to_string()); rows.len()];
        if rows.len() > 256 || self.poisoned {
            return (results, 0, 0, 0);
        }
        let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (i, row) in rows.iter().enumerate() {
            let mut matched = None;
            let mut ambiguous = false;
            for (segment, s) in self.manifest.segments.iter().enumerate() {
                if segment % 64 == 0
                    && (cancel.load(Ordering::Acquire) || Instant::now() >= deadline)
                {
                    return (results, 0, 0, 0);
                }
                if s.origin == row.origin
                    && s.min_source <= row.source_id
                    && row.source_id <= s.max_source
                {
                    if matched.replace(segment).is_some() {
                        ambiguous = true;
                        break;
                    }
                }
            }
            if !ambiguous {
                if let Some(segment) = matched {
                    groups.entry(segment).or_default().push(i);
                } else {
                    results[i] = Ok(false);
                }
            }
        }
        let mut used_bytes = 0u64;
        let mut used_rows = 0u64;
        let mut scans = 0u64;
        for (segment, indexes) in groups {
            let s = &self.manifest.segments[segment];
            if cancel.load(Ordering::Acquire)
                || Instant::now() >= deadline
                || s.bytes == 0
                || s.bytes > MAX_RECEIPT_SEGMENT_BYTES
                || s.rows as usize > MAX_APPLY_ROWS
                || used_bytes.saturating_add(s.bytes) > 64 * 1024 * 1024
                || used_rows.saturating_add(u64::from(s.rows)) > 32768
            {
                continue;
            }
            used_bytes += s.bytes;
            used_rows += u64::from(s.rows);
            scans += 1;
            let group = self.prove_receipt_segment(s, rows, &indexes, cancel, deadline);
            if let Ok(group) = group {
                for (index, value) in indexes.into_iter().zip(group) {
                    results[index] = value;
                }
            }
        }
        (results, scans, used_bytes, used_rows)
    }
    fn prove_receipt_segment(
        &self,
        segment: &Segment,
        rows: &[CanonicalRow],
        indexes: &[usize],
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> Result<Vec<Result<bool>>> {
        let check = || -> Result<()> {
            if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
                Err("receipt projection inspection interrupted".into())
            } else {
                Ok(())
            }
        };
        check()?;
        use rustix::fs::{Mode, OFlags};
        let handle = rustix::fs::open(
            &self.dir.join(&segment.file),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(err)?;
        let mut file = File::from(handle);
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata().map_err(err)?;
        if !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.len() != segment.bytes
        {
            return Err("receipt projection segment metadata mismatch".into());
        }
        // Capture each bounded segment once; decoding uses the same immutable bytes.
        let mut captured = Vec::with_capacity(segment.bytes as usize);
        let mut buffer = [0; 64 * 1024];
        while captured.len() < segment.bytes as usize {
            check()?;
            let remaining = segment.bytes as usize - captured.len();
            let take = remaining.min(buffer.len());
            let count = file.read(&mut buffer[..take]).map_err(err)?;
            if count == 0 {
                return Err("receipt projection segment truncated".into());
            }
            captured.extend_from_slice(&buffer[..count]);
        }
        if hex(&Sha256::digest(&captured)) != segment.digest {
            return Err("receipt projection segment digest mismatch".into());
        }
        check()?;
        let mut expected = Vec::with_capacity(indexes.len());
        for i in indexes {
            check()?;
            expected.push(TypedRow::decode(&rows[*i]).and_then(|r| to_batch(&[r])));
        }
        let mut matched = vec![false; indexes.len()];
        let mut bad = vec![false; indexes.len()];
        let mut total_rows = 0usize;
        let reader =
            fabric_store::parquet_guard::open_verified_parquet_batch_reader_from_bytes_with_check(
                bytes::Bytes::from(captured),
                Some(&schema()),
                128,
                check,
            )?;
        for batch in reader {
            check()?;
            let batch = batch.map_err(err)?;
            total_rows += batch.num_rows();
            if total_rows > segment.rows as usize {
                return Err("receipt projection segment row count mismatch".into());
            }
            let ids = batch
                .column_by_name("source_id")
                .and_then(|c| c.as_any().downcast_ref::<UInt64Array>())
                .ok_or("invalid projected source ID column")?;
            for (row_index, id) in ids.iter().enumerate() {
                for (j, index) in indexes.iter().enumerate() {
                    if id == Some(rows[*index].source_id) {
                        if matched[j]
                            || expected[j]
                                .as_ref()
                                .map_or(true, |expected| batch.slice(row_index, 1) != *expected)
                        {
                            bad[j] = true;
                        }
                        matched[j] = true;
                    }
                }
            }
        }
        check()?;
        if total_rows != segment.rows as usize {
            return Err("receipt projection segment row count mismatch".into());
        }
        Ok((0..indexes.len())
            .map(|i| {
                if bad[i] || expected[i].is_err() {
                    Err("receipt projection row mismatch".into())
                } else {
                    Ok(matched[i])
                }
            })
            .collect())
    }

    pub fn apply(&mut self, rows: &[CanonicalRow]) -> Result<ProjectionProgress> {
        if self.poisoned {
            return Err("projection publication uncertain; reopen required".into());
        }
        if rows.is_empty() {
            return Ok(self.progress());
        }
        if rows.len() > MAX_APPLY_ROWS {
            return Err("projection apply row limit exceeded".into());
        }
        let raw_bytes = rows.iter().try_fold(0usize, |total, row| {
            total
                .checked_add(row.raw.len())
                .ok_or("projection apply raw byte limit exceeded")
        })?;
        if raw_bytes > MAX_APPLY_RAW_BYTES {
            return Err("projection apply raw byte limit exceeded".into());
        }
        if rows
            .iter()
            .any(|row| row.origin.is_empty() || row.origin.len() > 256 || row.origin.contains('\0'))
        {
            return Err("invalid projection origin".into());
        }
        let mut progress = self.progress();
        let mut grouped: BTreeMap<String, Vec<TypedRow>> = BTreeMap::new();
        for row in rows {
            if row.canonical_id
                != progress
                    .checkpoint
                    .checked_add(1)
                    .ok_or("canonical ID exhausted")?
                || row.source_id
                    != progress
                        .frontiers
                        .get(&row.origin)
                        .copied()
                        .unwrap_or(0)
                        .checked_add(1)
                        .ok_or("source ID exhausted")?
            {
                return Err("noncontiguous projection input".into());
            }
            let typed = TypedRow::decode(row)?;
            grouped.entry(row.origin.clone()).or_default().push(typed);
            progress.checkpoint = row.canonical_id;
            progress.frontiers.insert(row.origin.clone(), row.source_id);
        }
        let mut next = self.manifest.clone();
        let mut staged = Vec::new();
        let mut created_staging: Option<PathBuf> = None;
        let mut post_rename_uncertain = false;
        let transaction = (|| {
            for (_, mut group) in grouped {
                group.sort_by(|a, b| {
                    a.run
                        .cmp(&b.run)
                        .then(a.timestamp_ns.cmp(&b.timestamp_ns))
                        .then(a.canonical_id.cmp(&b.canonical_id))
                });
                let first = &group[0];
                let batch = to_batch(&group)?;
                let file = format!("segment-{}-{}.parquet", rows[0].canonical_id, staged.len());
                let staging = self.dir.join(format!("staging-{file}"));
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&staging)
                    .map_err(err)?;
                created_staging = Some(staging.clone());
                enforce_private_file_mode(&output, &file)?;
                let origin_cap = self.origin_quota(&first.origin);
                let origin_used = if origin_cap.is_some() {
                    next.segments
                        .iter()
                        .filter(|s| s.origin == first.origin)
                        .try_fold(0u64, |n, s| {
                            n.checked_add(s.bytes).ok_or("projection size overflow")
                        })?
                } else {
                    0
                };
                let origin_remaining = origin_cap
                    .map(|cap| {
                        cap.checked_sub(origin_used)
                            .ok_or("projection origin quota exhausted")
                    })
                    .transpose()?;
                {
                    let committed_metadata =
                        fs::metadata(self.dir.join(MANIFEST)).map_err(err)?.len();
                    let used = next
                        .segments
                        .iter()
                        .try_fold(RESERVE + committed_metadata, |n, s| {
                            n.checked_add(s.bytes).ok_or("projection size overflow")
                        })?;
                    let global_remaining = self
                        .quota
                        .checked_sub(used)
                        .ok_or("projection quota exhausted")?;
                    let remaining = origin_remaining
                        .map_or(global_remaining, |orig| global_remaining.min(orig));
                    let limited = LimitedFile {
                        file: &mut output,
                        remaining,
                    };
                    let mut writer = ArrowWriter::try_new(
                        limited,
                        batch.schema(),
                        Some(
                            WriterProperties::builder()
                                .set_max_row_group_row_count(Some(512))
                                .set_column_bloom_filter_enabled(ColumnPath::from("run_key"), true)
                                .build(),
                        ),
                    )
                    .map_err(err)?;
                    writer.write(&batch).map_err(err)?;
                    writer.close().map_err(err)?;
                }
                output.sync_all().map_err(err)?;
                drop(output);
                let bytes = fs::metadata(&staging).map_err(err)?.len();
                let used = next.segments.iter().try_fold(RESERVE, |n, s| {
                    n.checked_add(s.bytes).ok_or("projection size overflow")
                })?;
                if used.checked_add(bytes).ok_or("projection size overflow")? > self.quota {
                    return Err("projection quota exhausted".into());
                }
                if let Some(cap) = origin_cap
                    && origin_used
                        .checked_add(bytes)
                        .ok_or("projection size overflow")?
                        > cap
                {
                    return Err("projection origin quota exhausted".into());
                }
                let hash = file_digest(&staging)?;
                fs::rename(&staging, self.dir.join(&file)).map_err(err)?;
                created_staging = None;
                staged.push(file.clone());
                next.segments.push(Segment {
                    file,
                    digest: hash,
                    bytes,
                    rows: group.len().try_into().map_err(err)?,
                    origin: first.origin.clone(),
                    runs: group
                        .iter()
                        .filter_map(|r| r.run.clone())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                    min_ns: group.iter().map(|r| r.timestamp_ns).min().unwrap(),
                    max_ns: group.iter().map(|r| r.timestamp_ns).max().unwrap(),
                    min_source: group.iter().map(|r| r.source_id).min().unwrap(),
                    max_source: group.iter().map(|r| r.source_id).max().unwrap(),
                    schemas: group
                        .iter()
                        .map(|r| r.registry.clone())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                });
            }
            next.progress = progress;
            self.publish_checked(&next).map_err(|(e, post_rename)| {
                post_rename_uncertain = post_rename;
                e
            })
        })();
        if let Err(e) = transaction {
            if post_rename_uncertain {
                self.poisoned = true;
            } else {
                if let Some(staging) = created_staging {
                    let _ = fs::remove_file(staging);
                }
                for file in staged {
                    let _ = fs::remove_file(self.dir.join(file));
                }
            }
            return Err(e);
        }
        self.manifest = next;
        Ok(self.progress())
    }
    fn publish(&self, manifest: &Manifest) -> Result<()> {
        self.publish_checked(manifest).map_err(|(e, _)| e)
    }
    fn publish_checked(&self, manifest: &Manifest) -> std::result::Result<(), (String, bool)> {
        let fail_pre = |e: String| (e, false);
        let old_size = regular_metadata(&self.dir.join(MANIFEST))
            .map_err(&fail_pre)?
            .map_or(0, |metadata| metadata.len());
        let fixed = manifest
            .segments
            .iter()
            .try_fold(RESERVE + old_size, |total, segment| {
                total
                    .checked_add(segment.bytes)
                    .ok_or_else(|| "projection size overflow".to_owned())
            })
            .map_err(&fail_pre)?;
        let available = self
            .quota
            .checked_sub(fixed)
            .ok_or_else(|| "projection quota exhausted".to_owned())
            .map_err(&fail_pre)?;
        let limit = available.min(MAX_MANIFEST_BYTES as u64) as usize;
        let digest = manifest_digest(manifest, limit).map_err(&fail_pre)?;
        #[derive(Serialize)]
        struct EnvelopeRef<'a> {
            digest: &'a str,
            manifest: &'a Manifest,
        }
        let bytes = bounded_json(
            &EnvelopeRef {
                digest: &digest,
                manifest,
            },
            limit,
        )
        .map_err(&fail_pre)?;
        let temp = self.dir.join("staging-manifest");
        regular_metadata(&temp).map_err(&fail_pre)?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temp)
            .map_err(err)
            .map_err(&fail_pre)?;
        if let Err(e) = enforce_private_file_mode(&file, "staging-manifest") {
            drop(file);
            let _ = fs::remove_file(&temp);
            return Err((e, false));
        }
        if let Err(e) = file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(err)
        {
            drop(file);
            let _ = fs::remove_file(&temp);
            return Err((e, false));
        }
        drop(file);
        let dir_file = File::open(&self.dir).map_err(err).map_err(|e| {
            let _ = fs::remove_file(&temp);
            (e, false)
        })?;
        fs::rename(&temp, self.dir.join(MANIFEST)).map_err(|e| {
            let _ = fs::remove_file(&temp);
            (err(e), false)
        })?;
        dir_file.sync_all().map_err(|e| (err(e), true))
    }
    fn resolve(&self, mut request: QueryRequest) -> Result<(QueryRequest, u64, Option<CursorKey>)> {
        if self.poisoned {
            return Err("projection publication uncertain; reopen required".into());
        }
        request.validate()?;
        if self.manifest.authority.is_some() && request.authority != self.manifest.authority {
            return Err("query canonical authority mismatch".into());
        }
        request.scope.sort();
        request.scope.dedup();
        let mut offset = 0;
        let mut after = None;
        if let Some(token) = request.cursor.take() {
            let cursor = self.decode_cursor(&token)?;
            if now() > cursor.expires {
                return Err("cursor expired".into());
            }
            if !request.snapshot.is_empty() && request.snapshot != cursor.snapshot {
                return Err("cursor snapshot mismatch".into());
            }
            request.snapshot = cursor.snapshot;
            if identity(&request)? != cursor.identity {
                return Err("cursor query or scope mismatch".into());
            }
            offset = cursor.offset;
            after = cursor.after;
        } else if request.snapshot.is_empty() {
            request.snapshot = request
                .scope
                .iter()
                .map(|s| {
                    (
                        s.clone(),
                        self.manifest
                            .progress
                            .frontiers
                            .get(s)
                            .copied()
                            .unwrap_or(0),
                    )
                })
                .collect();
        }
        if request
            .scope
            .iter()
            .any(|s| !request.snapshot.contains_key(s))
            || request.snapshot.iter().any(|(s, p)| {
                *p > self
                    .manifest
                    .progress
                    .frontiers
                    .get(s)
                    .copied()
                    .unwrap_or(0)
            })
        {
            return Err("snapshot is unavailable in projection".into());
        }
        Ok((request, offset, after))
    }
    fn selected<'a>(&'a self, r: &QueryRequest) -> Vec<&'a Segment> {
        self.manifest
            .segments
            .iter()
            .filter(|s| {
                r.snapshot
                    .get(&s.origin)
                    .is_some_and(|p| *p >= s.min_source)
                    && r.filter
                        .run
                        .as_ref()
                        .is_none_or(|k| s.runs.binary_search(&k.encoded()).is_ok())
                    && r.filter.start_ns.is_none_or(|start| s.max_ns >= start)
                    && r.filter.end_ns.is_none_or(|end| s.min_ns < end)
            })
            .collect()
    }
    pub fn plan(&self, request: QueryRequest) -> Result<QueryPlan> {
        let (r, _, _) = self.resolve(request)?;
        let selected = self.selected(&r);
        Ok(QueryPlan {
            version: API_VERSION,
            canonical_authority: self
                .manifest
                .authority
                .clone()
                .unwrap_or_else(|| "unbound-projection".into()),
            scope: r.scope.clone(),
            snapshot: r.snapshot.clone(),
            schemas: selected
                .iter()
                .flat_map(|s| s.schemas.iter().cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            selected_segments: Some(selected.len().try_into().map_err(err)?),
            estimated_scan_rows: selected
                .iter()
                .map(|s| u64::from(s.rows))
                .sum::<u64>()
                .try_into()
                .unwrap_or(u32::MAX),
            projection: self.scoped_progress(&r.scope),
            budgets: r.budgets,
            transform: TRANSFORM_VERSION.into(),
        })
    }
    pub fn query(&self, request: QueryRequest) -> Result<QueryResponse> {
        self.query_with_cancel(request, &AtomicBool::new(false))
    }
    pub fn query_with_cancel(
        &self,
        request: QueryRequest,
        cancel: &AtomicBool,
    ) -> Result<QueryResponse> {
        let deadline = Instant::now() + Duration::from_millis(request.budgets.timeout_ms.into());
        self.query_with_cancel_deadline(request, cancel, deadline)
    }
    /// A receiver can include projection-wait time in the original request budget
    /// without changing the request identity used for continuation cursors.
    pub fn query_with_cancel_deadline(
        &self,
        request: QueryRequest,
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> Result<QueryResponse> {
        if self
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("query worker busy".into());
        }
        struct Guard<'a>(&'a AtomicBool);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _guard = Guard(&self.active);
        let started = Instant::now();
        let (r, offset, after) = self.resolve(request)?;
        let deadline = deadline.min(started + Duration::from_millis(r.budgets.timeout_ms.into()));
        if Instant::now() >= deadline {
            return Err("query timeout".into());
        }
        let selected = self.selected(&r);
        let scanned = selected.iter().map(|s| u64::from(s.rows)).sum::<u64>();
        if scanned > u64::from(r.budgets.scan_rows) {
            return Err("scan budget exhausted; aggregate totals unavailable".into());
        }
        if cancel.load(Ordering::Acquire) {
            return Err("query cancelled".into());
        }
        // Scope-selected files only enter this session; filters precede every aggregate.
        let runtime = shared_query_runtime()?;
        let result = runtime.block_on(async {
            let query = self.execute(&r, &selected, offset, after.as_ref());
            tokio::pin!(query);
            let timeout = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
            tokio::pin!(timeout);
            let mut poll = tokio::time::interval(Duration::from_millis(5));
            loop {
                tokio::select! {
                    result = &mut query => break result,
                    _ = &mut timeout => break Err("query timeout".into()),
                    _ = poll.tick() => if cancel.load(Ordering::Acquire) {
                        break Err("query cancelled".into());
                    }
                }
            }
        });
        let (mut rows, mut cursor_keys, gaps, schemas, has_more) = result?;
        let mut coverage = Coverage {
            canonical_authority: self
                .manifest
                .authority
                .clone()
                .unwrap_or_else(|| "unbound-projection".into()),
            scope: r.scope.clone(),
            snapshot: r.snapshot.clone(),
            transform: TRANSFORM_VERSION.into(),
            schemas,
            projection: self.scoped_progress(&r.scope),
            canonical_frontier: self.scoped_progress(&r.scope).frontiers,
            observed_gaps: gaps,
            complete: !has_more,
            truncated: has_more,
            consumed: ConsumedBudget {
                scanned_rows: scanned.try_into().map_err(err)?,
                output_bytes: 0,
                elapsed_ms: started.elapsed().as_millis().try_into().unwrap_or(u32::MAX),
            },
        };
        let mut next_offset = offset + rows.len() as u64;
        loop {
            let next_cursor = if coverage.truncated {
                let key = cursor_keys.last().cloned();
                Some(self.encode_cursor(&Cursor {
                    identity: identity(&r)?,
                    snapshot: r.snapshot.clone(),
                    offset: if key.is_some() { 0 } else { next_offset },
                    after: key,
                    expires: now() + 900,
                })?)
            } else {
                None
            };
            let mut response = QueryResponse {
                version: API_VERSION,
                rows: rows.clone(),
                coverage: coverage.clone(),
                next_cursor,
            };
            let bytes = loop {
                let n = serde_json::to_vec(&response).map_err(err)?.len();
                if response.coverage.consumed.output_bytes as usize == n {
                    break n;
                }
                response.coverage.consumed.output_bytes = n.try_into().map_err(err)?;
            };
            if bytes <= r.budgets.output_bytes as usize {
                return Ok(response);
            }
            if rows.is_empty() {
                return Err("output budget too small for coverage".into());
            }
            rows.pop();
            cursor_keys.pop();
            next_offset = next_offset.saturating_sub(1);
            coverage.truncated = true;
            coverage.complete = false;
            if rows.is_empty() {
                return Err("output budget too small for one result row".into());
            }
        }
    }
    fn encode_cursor(&self, c: &Cursor) -> Result<String> {
        let bytes = serde_json::to_vec(c).map_err(err)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.manifest.cursor_key).map_err(err)?;
        mac.update(&bytes);
        Ok(format!(
            "{}.{}",
            hex(&bytes),
            hex(&mac.finalize().into_bytes())
        ))
    }
    fn decode_cursor(&self, s: &str) -> Result<Cursor> {
        let (payload, signature) = s.split_once('.').ok_or("invalid cursor")?;
        let bytes = unhex(payload)?;
        let signature = unhex(signature)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.manifest.cursor_key).map_err(err)?;
        mac.update(&bytes);
        mac.verify_slice(&signature)
            .map_err(|_| "invalid cursor signature")?;
        serde_json::from_slice(&bytes).map_err(err)
    }
    async fn execute(
        &self,
        r: &QueryRequest,
        segments: &[&Segment],
        offset: u64,
        after: Option<&CursorKey>,
    ) -> Result<(
        Vec<serde_json::Value>,
        Vec<CursorKey>,
        Vec<String>,
        Vec<String>,
        bool,
    )> {
        if matches!(r.operation, Operation::Capabilities) {
            return Ok((
                vec![
                    serde_json::json!({"api_version":API_VERSION,"operations":["capabilities","list_runs","run_summary","tool_summary","service_summary","activity","evidence"],"free_sql":false,"integer_encoding":"unsigned decimal strings","pool_bytes":POOL.to_string(),"spill_bytes":SPILL.to_string()}),
                ],
                vec![],
                vec![],
                vec![],
                false,
            ));
        }
        let env = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::new(GreedyMemoryPool::new(POOL)))
            .with_temp_file_path(self.dir.join("spill"))
            .with_max_temp_directory_size(SPILL)
            .build_arc()
            .map_err(err)?;
        // ListingTable eagerly heads every singleton table path before its
        // metadata-concurrency buffer. Bound requests at the store, including
        // that earlier phase, rather than relying on target_partitions.
        env.register_object_store(
            ObjectStoreUrl::local_filesystem().as_ref(),
            Arc::new(LimitStore::new(LocalFileSystem::new(), FILE_REQUESTS)),
        );
        let config = SessionConfig::new()
            .with_target_partitions(EXECUTION_THREADS)
            .set_usize("datafusion.execution.meta_fetch_concurrency", FILE_REQUESTS);
        let ctx = SessionContext::new_with_config_rt(config, env);
        if segments.is_empty() {
            ctx.register_batch("observations", RecordBatch::new_empty(schema()))
                .map_err(err)?;
        } else {
            let paths = segments
                .iter()
                .map(|s| self.dir.join(&s.file).to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let fixed_schema = schema();
            let df = ctx
                .read_parquet(
                    paths,
                    ParquetReadOptions::default()
                        .schema(&fixed_schema)
                        .metadata_size_hint(Some(METADATA_PREFETCH_BYTES)),
                )
                .await
                .map_err(err)?;
            ctx.register_table("observations", df.into_view())
                .map_err(err)?;
        }
        let mut predicates = vec![format!(
            "({})",
            r.snapshot
                .iter()
                .map(|(o, p)| format!(
                    "(origin={} AND source_id <= CAST({} AS BIGINT UNSIGNED))",
                    quote(o),
                    quote(&p.to_string())
                ))
                .collect::<Vec<_>>()
                .join(" OR ")
        )];
        if let Some(run) = &r.filter.run {
            predicates.push(format!("run_key={}", quote(&run.encoded())));
        }
        if let Some(start) = r.filter.start_ns {
            predicates.push(format!(
                "timestamp_ns >= CAST({} AS BIGINT UNSIGNED)",
                quote(&start.to_string())
            ));
        }
        if let Some(end) = r.filter.end_ns {
            predicates.push(format!(
                "timestamp_ns < CAST({} AS BIGINT UNSIGNED)",
                quote(&end.to_string())
            ));
        }
        if let Some(resource) = &r.filter.resource {
            predicates.push(format!("resource={}", quote(resource)));
        }
        let filtered = ctx
            .sql(&format!(
                "SELECT * FROM observations WHERE {}",
                predicates.join(" AND ")
            ))
            .await
            .map_err(err)?;
        ctx.register_table("scoped", filtered.into_view())
            .map_err(err)?;
        let gap_batches = ctx
            .sql("SELECT DISTINCT gap FROM scoped WHERE gap IS NOT NULL ORDER BY gap LIMIT 257")
            .await
            .map_err(err)?
            .collect()
            .await
            .map_err(err)?;
        let gap_values = values(&gap_batches)?;
        if gap_values.len() > 256 {
            return Err("gap metadata exceeds coverage budget".into());
        }
        let gaps = gap_values
            .into_iter()
            .filter_map(|v| v.get("gap").and_then(|s| s.as_str()).map(str::to_owned))
            .collect();
        let stats = "COUNT(*) AS count, SUM(CASE WHEN status='error' THEN 1 ELSE 0 END) AS errors, SUM(CASE WHEN status='cancelled' THEN 1 ELSE 0 END) AS cancelled, COUNT(duration_ns) AS duration_count, MIN(duration_ns) AS min_ns, MAX(duration_ns) AS max_ns, SUM(CAST(duration_ns AS DECIMAL(38,0))) AS sum_ns";
        let schemas = segments
            .iter()
            .flat_map(|s| s.schemas.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let base = match (&r.operation, after) {
            (Operation::Evidence, Some(CursorKey::CanonicalId(id))) => format!(
                "SELECT canonical_id, origin, source_id, timestamp_ns, run_key, resource, raw FROM scoped WHERE canonical_id > CAST({} AS BIGINT UNSIGNED) ORDER BY canonical_id",
                quote(&id.to_string())
            ),
            (Operation::Evidence, None) => {
                "SELECT canonical_id, origin, source_id, timestamp_ns, run_key, resource, raw FROM scoped ORDER BY canonical_id".into()
            }
            (Operation::ListRuns | Operation::RunSummary, Some(CursorKey::Key(key))) => format!(
                "SELECT run_key, {stats} FROM scoped WHERE run_key IS NOT NULL AND run_key > {} GROUP BY run_key ORDER BY run_key",
                quote(key)
            ),
            (Operation::ListRuns | Operation::RunSummary, None) => format!(
                "SELECT run_key, {stats} FROM scoped WHERE run_key IS NOT NULL GROUP BY run_key ORDER BY run_key"
            ),
            (Operation::ToolSummary, Some(CursorKey::Key(key))) => format!(
                "SELECT name AS tool, {stats} FROM scoped WHERE kind='tool_execution' AND name > {} GROUP BY name ORDER BY name",
                quote(key)
            ),
            (Operation::ToolSummary, None) => format!(
                "SELECT name AS tool, {stats} FROM scoped WHERE kind='tool_execution' GROUP BY name ORDER BY name"
            ),
            (Operation::ServiceSummary, Some(CursorKey::Key(key))) => format!(
                "SELECT name AS service, {stats} FROM scoped WHERE kind='service_request' AND name > {} GROUP BY name ORDER BY name",
                quote(key)
            ),
            (Operation::ServiceSummary, None) => format!(
                "SELECT name AS service, {stats} FROM scoped WHERE kind='service_request' GROUP BY name ORDER BY name"
            ),
            (Operation::Activity { bucket_ns }, Some(CursorKey::BucketNs(last_bucket))) => {
                let Some(next_start) = last_bucket.checked_add(*bucket_ns) else {
                    return Ok((Vec::new(), Vec::new(), gaps, schemas, false));
                };
                format!(
                    "SELECT (timestamp_ns / CAST({} AS BIGINT UNSIGNED))*CAST({} AS BIGINT UNSIGNED) AS bucket_start_ns, {stats} FROM scoped WHERE timestamp_ns >= CAST({} AS BIGINT UNSIGNED) GROUP BY bucket_start_ns ORDER BY bucket_start_ns",
                    quote(&bucket_ns.to_string()),
                    quote(&bucket_ns.to_string()),
                    quote(&next_start.to_string())
                )
            }
            (Operation::Activity { bucket_ns }, None) => format!(
                "SELECT (timestamp_ns / CAST({} AS BIGINT UNSIGNED))*CAST({} AS BIGINT UNSIGNED) AS bucket_start_ns, {stats} FROM scoped GROUP BY bucket_start_ns ORDER BY bucket_start_ns",
                quote(&bucket_ns.to_string()),
                quote(&bucket_ns.to_string())
            ),
            (Operation::Capabilities, _) => unreachable!(),
            _ => return Err("cursor key type mismatch".into()),
        };
        let sql = if after.is_none() && offset > 0 {
            format!(
                "{base} LIMIT {} OFFSET {offset}",
                u64::from(r.budgets.rows) + 1
            )
        } else {
            format!("{base} LIMIT {}", u64::from(r.budgets.rows) + 1)
        };
        let batches = ctx
            .sql(&sql)
            .await
            .map_err(err)?
            .collect()
            .await
            .map_err(err)?;
        let mut rows = values(&batches)?;
        let has_more = rows.len() > r.budgets.rows as usize;
        rows.truncate(r.budgets.rows as usize);
        let mut cursor_keys = Vec::with_capacity(rows.len());
        for row in &mut rows {
            if let Some(o) = row.as_object_mut() {
                match &r.operation {
                    Operation::Evidence => {
                        let id = o
                            .get("canonical_id")
                            .and_then(|v| v.as_str())
                            .ok_or("missing canonical_id for cursor")?
                            .parse::<u64>()
                            .map_err(err)?;
                        cursor_keys.push(CursorKey::CanonicalId(id));
                    }
                    Operation::ListRuns | Operation::RunSummary => {
                        let key = o
                            .get("run_key")
                            .and_then(|v| v.as_str())
                            .ok_or("missing run_key for cursor")?;
                        cursor_keys.push(CursorKey::Key(key.to_owned()));
                    }
                    Operation::ToolSummary => {
                        let key = o
                            .get("tool")
                            .and_then(|v| v.as_str())
                            .ok_or("missing tool for cursor")?;
                        cursor_keys.push(CursorKey::Key(key.to_owned()));
                    }
                    Operation::ServiceSummary => {
                        let key = o
                            .get("service")
                            .and_then(|v| v.as_str())
                            .ok_or("missing service for cursor")?;
                        cursor_keys.push(CursorKey::Key(key.to_owned()));
                    }
                    Operation::Activity { .. } => {
                        let b = o
                            .get("bucket_start_ns")
                            .and_then(|v| v.as_str())
                            .ok_or("missing bucket_start_ns for cursor")?
                            .parse::<u64>()
                            .map_err(err)?;
                        cursor_keys.push(CursorKey::BucketNs(b));
                    }
                    Operation::Capabilities => {}
                }
                if let Some(key) = o.remove("run_key") {
                    let key = match key.as_str() {
                        Some(s) => serde_json::from_str(s).map_err(err)?,
                        None => serde_json::Value::Null,
                    };
                    o.insert("run".into(), key);
                }
                if let (Some(sum), Some(count)) =
                    (o.get("sum_ns").cloned(), o.get("duration_count").cloned())
                {
                    o.insert(
                        "mean_ns".into(),
                        if sum.is_null() {
                            serde_json::Value::Null
                        } else {
                            serde_json::json!({"numerator":sum,"denominator":count})
                        },
                    );
                }
            }
        }
        Ok((rows, cursor_keys, gaps, schemas, has_more))
    }
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
fn file_digest(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(err)?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf).map_err(err)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hex(&hash.finalize()))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(s: &str) -> Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return Err("invalid cursor encoding".into());
    }
    s.as_bytes()
        .chunks(2)
        .map(|b| {
            std::str::from_utf8(b)
                .map_err(err)
                .and_then(|s| u8::from_str_radix(s, 16).map_err(err))
        })
        .collect()
}
fn valid_segment_name(s: &str) -> bool {
    s.starts_with("segment-")
        && s.ends_with(".parquet")
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'))
}
fn identity(r: &QueryRequest) -> Result<String> {
    let mut r = r.clone();
    r.cursor = None;
    r.query_id.clear();
    Ok(digest(&serde_json::to_vec(&r).map_err(err)?))
}
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
struct TypedRow {
    canonical_id: u64,
    origin: String,
    source_id: u64,
    timestamp_ns: u64,
    run: Option<String>,
    resource: String,
    kind: String,
    name: Option<String>,
    duration: Option<u64>,
    status: Option<String>,
    raw: String,
    registry: String,
    gap: Option<String>,
}
impl TypedRow {
    fn decode(row: &CanonicalRow) -> Result<Self> {
        let stored = observation::decode_versioned(row.raw.as_bytes())?;
        let identity = stored.identity();
        let data = stored.data();
        let legacy_run = match data {
            Data::AgentRun { run_id, .. } | Data::ToolExecution { run_id, .. } => {
                Some(run_id.clone())
            }
            _ => None,
        };
        let run = if let Some(context) = stored.context() {
            Some(
                RunKey::Global {
                    run_id: context.run_id.clone(),
                }
                .encoded(),
            )
        } else {
            legacy_run.map(|run_id| {
                RunKey::Legacy {
                    origin: row.origin.clone(),
                    producer: identity.producer.clone(),
                    generation: identity.generation.clone(),
                    run_id,
                }
                .encoded()
            })
        };
        let (kind, name, duration, status, gap) = match data {
            Data::ServiceRequest {
                service,
                duration_ns,
                status,
                ..
            } => (
                "service_request",
                Some(service.clone()),
                Some(*duration_ns),
                Some(*status),
                None,
            ),
            Data::AgentRun {
                agent,
                duration_ns,
                status,
                ..
            } => (
                "agent_run",
                Some(agent.clone()),
                Some(*duration_ns),
                Some(*status),
                None,
            ),
            Data::ToolExecution {
                tool,
                duration_ns,
                status,
                ..
            } => (
                "tool_execution",
                Some(tool.clone()),
                Some(*duration_ns),
                Some(*status),
                None,
            ),
            Data::Gap { source, reason } => (
                "gap",
                None,
                None,
                None,
                Some(serde_json::to_string(&(source, reason)).map_err(err)?),
            ),
            Data::Log { .. } => ("log", None, None, None, None),
            Data::JsonLog { .. } => ("json_log", None, None, None, None),
            Data::Event { .. } => ("event", None, None, None, None),
            Data::Metric { .. } => ("metric", None, None, None, None),
        };
        Ok(Self {
            canonical_id: row.canonical_id,
            origin: row.origin.clone(),
            source_id: row.source_id,
            timestamp_ns: stored.timestamp_ns(),
            run,
            resource: stored.resource().into(),
            kind: kind.into(),
            name,
            duration,
            status: status.map(|s| {
                match s {
                    Outcome::Ok => "ok",
                    Outcome::Error => "error",
                    Outcome::Cancelled => "cancelled",
                }
                .into()
            }),
            raw: row.raw.clone(),
            registry: stored.registry().into(),
            gap,
        })
    }
}
fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("canonical_id", DataType::UInt64, false),
        Field::new("origin", DataType::Utf8, false),
        Field::new("source_id", DataType::UInt64, false),
        Field::new("timestamp_ns", DataType::UInt64, false),
        Field::new("run_key", DataType::Utf8, true),
        Field::new("resource", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("duration_ns", DataType::UInt64, true),
        Field::new("status", DataType::Utf8, true),
        Field::new("raw", DataType::Utf8, false),
        Field::new("registry", DataType::Utf8, false),
        Field::new("gap", DataType::Utf8, true),
    ]))
}
fn to_batch(rows: &[TypedRow]) -> Result<RecordBatch> {
    let cols: Vec<ArrayRef> = vec![
        Arc::new(UInt64Array::from_iter_values(
            rows.iter().map(|r| r.canonical_id),
        )),
        Arc::new(StringArray::from_iter_values(
            rows.iter().map(|r| r.origin.as_str()),
        )),
        Arc::new(UInt64Array::from_iter_values(
            rows.iter().map(|r| r.source_id),
        )),
        Arc::new(UInt64Array::from_iter_values(
            rows.iter().map(|r| r.timestamp_ns),
        )),
        Arc::new(StringArray::from_iter(
            rows.iter().map(|r| r.run.as_deref()),
        )),
        Arc::new(StringArray::from_iter_values(
            rows.iter().map(|r| r.resource.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            rows.iter().map(|r| r.kind.as_str()),
        )),
        Arc::new(StringArray::from_iter(
            rows.iter().map(|r| r.name.as_deref()),
        )),
        Arc::new(UInt64Array::from_iter(rows.iter().map(|r| r.duration))),
        Arc::new(StringArray::from_iter(
            rows.iter().map(|r| r.status.as_deref()),
        )),
        Arc::new(StringArray::from_iter_values(
            rows.iter().map(|r| r.raw.as_str()),
        )),
        Arc::new(StringArray::from_iter_values(
            rows.iter().map(|r| r.registry.as_str()),
        )),
        Arc::new(StringArray::from_iter(
            rows.iter().map(|r| r.gap.as_deref()),
        )),
    ];
    RecordBatch::try_new(schema(), cols).map_err(err)
}
fn values(batches: &[RecordBatch]) -> Result<Vec<serde_json::Value>> {
    use arrow::array::{Decimal128Array, Int64Array, StringViewArray, UInt32Array};
    let mut rows = Vec::new();
    for batch in batches {
        for row in 0..batch.num_rows() {
            let mut object = serde_json::Map::new();
            for (field, array) in batch.schema().fields().iter().zip(batch.columns()) {
                let value = if array.is_null(row) {
                    serde_json::Value::Null
                } else {
                    match array.data_type() {
                        DataType::Utf8 => serde_json::Value::String(
                            array
                                .as_any()
                                .downcast_ref::<StringArray>()
                                .ok_or("invalid string array")?
                                .value(row)
                                .into(),
                        ),
                        DataType::Utf8View => serde_json::Value::String(
                            array
                                .as_any()
                                .downcast_ref::<StringViewArray>()
                                .ok_or("invalid string view array")?
                                .value(row)
                                .into(),
                        ),
                        DataType::UInt64 => serde_json::Value::String(
                            array
                                .as_any()
                                .downcast_ref::<UInt64Array>()
                                .ok_or("invalid integer array")?
                                .value(row)
                                .to_string(),
                        ),
                        DataType::Int64 => serde_json::Value::String(
                            array
                                .as_any()
                                .downcast_ref::<Int64Array>()
                                .ok_or("invalid integer array")?
                                .value(row)
                                .to_string(),
                        ),
                        DataType::UInt32 => serde_json::Value::String(
                            array
                                .as_any()
                                .downcast_ref::<UInt32Array>()
                                .ok_or("invalid integer array")?
                                .value(row)
                                .to_string(),
                        ),
                        DataType::Decimal128(_, 0) => serde_json::Value::String(
                            array
                                .as_any()
                                .downcast_ref::<Decimal128Array>()
                                .ok_or("invalid decimal array")?
                                .value(row)
                                .to_string(),
                        ),
                        other => return Err(format!("unsupported output type {other}")),
                    }
                };
                object.insert(field.name().clone(), value);
            }
            rows.push(serde_json::Value::Object(object));
        }
    }
    Ok(rows)
}

/// Refuse a write before it can consume the publication/recovery reserve.
struct LimitedFile<'a> {
    file: &'a mut File,
    remaining: u64,
}
impl Write for LimitedFile<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(std::io::Error::other("projection quota exhausted"));
        }
        let n = self.file.write(bytes)?;
        self.remaining -= n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    use rustix::fs::{Mode, OFlags, fchmod};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder
                .recursive(true)
                .mode(0o700)
                .create(path)
                .map_err(err)?;
            let dir_fd = rustix::fs::open(
                path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(err)?;
            fchmod(&dir_fd, Mode::from_raw_mode(0o700)).map_err(err)?;
        }
        Err(error) => return Err(err(error)),
    }
    let metadata = fs::symlink_metadata(path).map_err(err)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err("projection directory must be owned, private and nonsymlink".into());
    }
    Ok(())
}

fn enforce_private_file_mode(file: &File, label: &str) -> Result<()> {
    let st = rustix::fs::fstat(file).map_err(err)?;
    if ((st.st_mode as u32) & 0o777) != 0o600 {
        rustix::fs::fchmod(file, rustix::fs::Mode::from_raw_mode(0o600))
            .map_err(|e| format!("failed setting 0600 on {label}: {e}"))?;
    }
    Ok(())
}
/// State lives under an owned private directory with one exclusive writer. Path
/// validation precedes opens; concurrent hostile replacement by that same owner
/// is outside this local state API's trust boundary.
fn regular_metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    use std::os::unix::fs::MetadataExt;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.uid() != rustix::process::geteuid().as_raw()
            {
                return Err(format!(
                    "projection path must be an owned regular nonsymlink file: {}",
                    path.display()
                ));
            }
            Ok(Some(metadata))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(err(error)),
    }
}
fn read_manifest(path: &Path, expected: usize) -> Result<Vec<u8>> {
    if expected > MAX_MANIFEST_BYTES {
        return Err("projection manifest byte limit exceeded".into());
    }
    let mut file = File::open(path).map_err(err)?;
    let mut bytes = vec![0; expected];
    file.read_exact(&mut bytes).map_err(err)?;
    let mut extra = [0];
    if file.read(&mut extra).map_err(err)? != 0 {
        return Err("projection manifest changed size".into());
    }
    Ok(bytes)
}
struct ManifestHash {
    hash: Sha256,
    remaining: usize,
}
impl Write for ManifestHash {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(std::io::Error::other(
                "projection manifest byte limit exceeded",
            ));
        }
        self.hash.update(bytes);
        self.remaining -= bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn manifest_digest(manifest: &Manifest, limit: usize) -> Result<String> {
    let mut writer = ManifestHash {
        hash: Sha256::new(),
        remaining: limit.min(MAX_MANIFEST_BYTES),
    };
    serde_json::to_writer(&mut writer, manifest).map_err(err)?;
    Ok(hex(&writer.hash.finalize()))
}
struct ManifestBuffer {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for ManifestBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let length = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| std::io::Error::other("projection manifest byte limit exceeded"))?;
        if length > self.limit {
            return Err(std::io::Error::other(
                "projection manifest byte limit exceeded",
            ));
        }
        if length > self.bytes.capacity() {
            let capacity = length.next_power_of_two().min(self.limit);
            self.bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .map_err(|error| std::io::Error::other(error.to_string()))?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn bounded_json(value: &impl Serialize, limit: usize) -> Result<Vec<u8>> {
    let mut writer = ManifestBuffer {
        bytes: Vec::new(),
        limit: limit.min(MAX_MANIFEST_BYTES),
    };
    serde_json::to_writer(&mut writer, value).map_err(err)?;
    Ok(writer.bytes)
}

#[cfg(test)]
mod resource_tests {
    use super::*;
    #[test]
    fn serialization_limit_precedes_growth_and_publication() {
        let mut buffer = ManifestBuffer {
            bytes: Vec::new(),
            limit: 1024,
        };
        buffer.write_all(&[1; 1000]).unwrap();
        let capacity = buffer.bytes.capacity();
        assert!(buffer.write_all(&[2; 25]).is_err());
        assert_eq!(buffer.bytes.len(), 1000);
        assert_eq!(buffer.bytes.capacity(), capacity);
        assert!(capacity <= 1024);
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let projection = Projection::open(dir.path(), 64 * 1024 * 1024).unwrap();
        let before = fs::read(dir.path().join(MANIFEST)).unwrap();
        let mut oversized = projection.manifest.clone();
        oversized.authority = Some(String::new());
        oversized.progress.checkpoint = 1;
        let base = bounded_json(&oversized, MAX_MANIFEST_BYTES).unwrap().len();
        oversized.authority = Some("x".repeat(MAX_MANIFEST_BYTES - base));
        // The manifest itself fits exactly; its envelope must still be refused.
        manifest_digest(&oversized, MAX_MANIFEST_BYTES).unwrap();
        assert!(
            projection
                .publish(&oversized)
                .unwrap_err()
                .contains("manifest byte limit")
        );
        assert_eq!(projection.progress().checkpoint, 0);
        assert_eq!(fs::read(dir.path().join(MANIFEST)).unwrap(), before);
        assert!(!dir.path().join("staging-manifest").exists());
    }
}
