//! ZSTD-compressed Arrow/Parquet OLAP writer and date-partitioned reader discovery.

use crate::parquet_guard::{
    MAX_PARQUET_FOOTER_SERIALIZED_BYTES, VerifiedParquetBatchReader,
    load_verified_parquet_metadata_from_file, open_verified_parquet_batch_reader_from_file,
    validate_provenance_compatible_parquet_schema,
};
use arrow::array::StringArray;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use chrono::{DateTime, NaiveDate, Utc};
use fabric_core::arrow_schemas::{
    fs_events_to_record_batch, graph_edges_to_record_batch, logs_to_record_batch,
    metrics_to_record_batch, traces_to_record_batch,
};
use fabric_core::{
    FsEventRecord, GraphEdgeRecord, LOGS_SCHEMA, LogRecord, METRICS_SCHEMA, MetricPoint,
    SPINDLE_PROVENANCE_COLUMN, SpindleProvenance, StorageLayout, TRACES_SCHEMA, TraceSpanRecord,
};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{BufWriter, ErrorKind, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub(crate) const O_DIRECTORY_FLAG: i32 = 0x4000;
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub(crate) const O_NOFOLLOW_FLAG: i32 = 0x8000;
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub(crate) const O_NONBLOCK_FLAG: i32 = 0x800;

#[cfg(all(target_os = "linux", not(target_arch = "aarch64")))]
pub(crate) const O_DIRECTORY_FLAG: i32 = 0x10000;
#[cfg(all(target_os = "linux", not(target_arch = "aarch64")))]
pub(crate) const O_NOFOLLOW_FLAG: i32 = 0x20000;
#[cfg(all(target_os = "linux", not(target_arch = "aarch64")))]
pub(crate) const O_NONBLOCK_FLAG: i32 = 0x800;

#[cfg(target_os = "macos")]
pub(crate) const O_DIRECTORY_FLAG: i32 = 0x100000;
#[cfg(target_os = "macos")]
pub(crate) const O_NOFOLLOW_FLAG: i32 = 0x0100;
#[cfg(target_os = "macos")]
pub(crate) const O_NONBLOCK_FLAG: i32 = 0x0004;

pub(crate) fn is_eloop_error(err: &std::io::Error) -> bool {
    matches!(err.raw_os_error(), Some(40 | 62))
}

pub(crate) fn is_enotdir_error(err: &std::io::Error) -> bool {
    matches!(err.raw_os_error(), Some(20))
}

pub(crate) const PARQUET_ROW_GROUP_ROWS: usize = 16_384;
pub(crate) const MAX_PARQUET_ROW_GROUP_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_COMPACTED_FILE_ROWS: usize = 8 * 1024 * 1024;
pub(crate) const MAX_COMPACT_SEALED_FILE_BYTES: u64 = 24 * 1024 * 1024;
pub(crate) const MAX_COMPACT_GROUP_BYTES: u64 = 48 * 1024 * 1024;
pub(crate) const MAX_COMPACT_OUTPUT_BYTES: u64 = 64 * 1024 * 1024;

const REPLACE_INTENT_FILE: &str = ".replace-intent.json";
const REPLACE_EPOCH_FILE: &str = ".replace-epoch";
const REPLACED_INPUTS_FILE: &str = ".replaced-inputs.json";
const UNCOMMITTED_PARTITIONS_FILE: &str = ".uncommitted-partitions.json";
const MAX_UNCOMMITTED_PARTITIONS_BYTES: u64 = 4 * 1024 * 1024;
const MAX_REPLACE_INTENT_BYTES: u64 = 64 * 1024;
const MAX_REPLACED_INPUTS_BYTES: u64 = 4 * 1024 * 1024;
const MAX_REPLACED_INPUT_RECORDS: usize = 16_384;
const MAX_REPLACE_EPOCH_BYTES: u64 = 64;
pub(crate) const MAX_REPLACE_FAN_IN_FILES: usize = 16;
const MAX_REPLACE_INTENT_INPUTS: usize = MAX_REPLACE_FAN_IN_FILES;
const MAX_REPLACE_INTENT_FILENAME_BYTES: usize = 256;
const MAX_REPLACE_INTENT_SHA256_BYTES: usize = 128;
const MAX_REPLACE_INTENT_DECODED_BYTES: usize = 20 * 1024;
const ARC_HEADER_BYTES: usize = 2 * std::mem::size_of::<usize>();

/// Default maximum number of pinned Parquet files returned in one snapshot.
pub const MAX_SNAPSHOT_PARQUET_FILES: usize = 16_384;
/// Default maximum retained metadata heap bytes returned in one snapshot (`4 MiB`).
pub const MAX_SNAPSHOT_METADATA_BYTES: usize = 4 * 1024 * 1024;

const SHA256_K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

pub(crate) struct Sha256 {
    state: [u32; 8],
    total_len: u64,
    block: [u8; 64],
    block_len: usize,
}

impl Sha256 {
    pub(crate) fn new() -> Self {
        Self {
            state: [
                0x6a09_e667,
                0xbb67_ae85,
                0x3c6e_f372,
                0xa54f_f53a,
                0x510e_527f,
                0x9b05_688c,
                0x1f83_d9ab,
                0x5be0_cd19,
            ],
            total_len: 0,
            block: [0_u8; 64],
            block_len: 0,
        }
    }

    fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
        let mut w = [0_u32; 64];
        for (idx, chunk) in block.as_chunks::<4>().0.iter().enumerate() {
            w[idx] = u32::from_be_bytes(*chunk);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (dst, add) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *dst = dst.wrapping_add(add);
        }
    }

    pub(crate) fn update(&mut self, mut data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        if self.block_len > 0 {
            let take = (64 - self.block_len).min(data.len());
            self.block[self.block_len..self.block_len + take].copy_from_slice(&data[..take]);
            self.block_len += take;
            data = &data[take..];
            if self.block_len == 64 {
                let blk = self.block;
                Self::compress(&mut self.state, &blk);
                self.block_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut blk = [0_u8; 64];
            blk.copy_from_slice(&data[..64]);
            Self::compress(&mut self.state, &blk);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.block[..data.len()].copy_from_slice(data);
            self.block_len = data.len();
        }
    }

    pub(crate) fn finish_hex(mut self) -> String {
        let total_bits = self.total_len.wrapping_mul(8);
        self.block[self.block_len] = 0x80;
        self.block_len += 1;
        if self.block_len > 56 {
            self.block[self.block_len..].fill(0);
            let blk = self.block;
            Self::compress(&mut self.state, &blk);
            self.block_len = 0;
        }
        self.block[self.block_len..56].fill(0);
        self.block[56..64].copy_from_slice(&total_bits.to_be_bytes());
        let blk = self.block;
        Self::compress(&mut self.state, &blk);
        let mut hex = String::with_capacity(64);
        for word in self.state {
            use std::fmt::Write as _;
            let _ = write!(&mut hex, "{word:08x}");
        }
        hex
    }
}

struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
    written: u64,
    limit: Option<u64>,
    refused: Arc<AtomicBool>,
}

#[derive(Debug)]
struct StagingLimit;

impl std::fmt::Display for StagingLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("staged parquet byte limit exceeded")
    }
}
impl std::error::Error for StagingLimit {}

fn is_staging_limit(err: &parquet::errors::ParquetError) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = current {
        if e.downcast_ref::<StagingLimit>().is_some()
            || e.downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::get_ref)
                .is_some_and(|inner| inner.downcast_ref::<StagingLimit>().is_some())
        {
            return true;
        }
        current = e.source();
    }
    false
}

impl<W: Write> HashingWriter<W> {
    fn new(inner: W, limit: Option<u64>, refused: Arc<AtomicBool>) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            written: 0,
            limit,
            refused,
        }
    }

    fn finish(self) -> (W, u64, String) {
        (self.inner, self.written, self.hasher.finish_hex())
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let len = u64::try_from(buf.len()).unwrap_or(u64::MAX);
        if self
            .limit
            .is_some_and(|max| self.written.checked_add(len).is_none_or(|n| n > max))
        {
            self.refused.store(true, Ordering::Relaxed);
            return Err(std::io::Error::other(StagingLimit));
        }
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.written = self.written.saturating_add(n as u64);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

pub(crate) fn sha256_regular_file(file: &File) -> Result<(u64, String), String> {
    let mut reader = file
        .try_clone()
        .map_err(|e| format!("failed cloning file fd for sha256: {e}"))?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|e| format!("failed seeking file fd for sha256: {e}"))?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buf = [0_u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("failed reading file fd for sha256: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total = total.saturating_add(n as u64);
    }
    Ok((total, hasher.finish_hex()))
}

pub(crate) fn sha256_bytes_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finish_hex()
}

fn deserialize_bounded_str_max<'de, D>(
    deserializer: D,
    max_len: usize,
    label: &'static str,
) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct BoundedStrVisitor {
        max_len: usize,
        label: &'static str,
    }
    impl<'de> serde::de::Visitor<'de> for BoundedStrVisitor {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "a string of at most {} bytes", self.max_len)
        }
        fn visit_borrowed_str<E: serde::de::Error>(self, v: &'de str) -> Result<Self::Value, E> {
            if v.len() > self.max_len {
                return Err(E::custom(format!(
                    "{} exceeds {} bytes",
                    self.label, self.max_len
                )));
            }
            Ok(v.to_owned())
        }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
            if v.len() > self.max_len {
                return Err(E::custom(format!(
                    "{} exceeds {} bytes",
                    self.label, self.max_len
                )));
            }
            Ok(v.to_owned())
        }
        fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
            if v.len() > self.max_len {
                return Err(E::custom(format!(
                    "{} exceeds {} bytes",
                    self.label, self.max_len
                )));
            }
            Ok(v)
        }
    }
    deserializer.deserialize_str(BoundedStrVisitor { max_len, label })
}

struct BoundedFilename(String);

impl<'de> serde::Deserialize<'de> for BoundedFilename {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserialize_bounded_str_max(
            deserializer,
            MAX_REPLACE_INTENT_FILENAME_BYTES,
            "replace intent filename",
        )
        .map(Self)
    }
}

fn deserialize_bounded_filename<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    BoundedFilename::deserialize(deserializer).map(|b| b.0)
}

fn deserialize_bounded_optional_filename<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<BoundedFilename>::deserialize(deserializer).map(|opt| opt.map(|b| b.0))
}

fn deserialize_bounded_optional_sha256<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct BoundedSha(String);
    impl<'de> serde::Deserialize<'de> for BoundedSha {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserialize_bounded_str_max(
                deserializer,
                MAX_REPLACE_INTENT_SHA256_BYTES,
                "file identity sha256",
            )
            .map(Self)
        }
    }
    Option::<BoundedSha>::deserialize(deserializer).map(|opt| opt.map(|b| b.0))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileIdentity {
    pub dev: u64,
    pub ino: u64,
    #[serde(default)]
    pub len: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_bounded_optional_sha256")]
    pub sha256: Option<String>,
}

impl FileIdentity {
    fn matches_stat(&self, dev: u64, ino: u64, len: u64) -> bool {
        self.dev == dev && self.ino == ino && self.len.is_none_or(|exp_len| exp_len == len)
    }
}

fn verify_open_file_matches_identity(
    file: &File,
    meta: &fs::Metadata,
    expected: &FileIdentity,
) -> Result<bool, String> {
    if !expected.matches_stat(meta.dev(), meta.ino(), meta.len()) {
        return Ok(false);
    }
    if let Some(ref exp_sha) = expected.sha256 {
        let (actual_len, actual_sha) = sha256_regular_file(file)?;
        if expected.len.is_some_and(|exp_len| exp_len != actual_len) || &actual_sha != exp_sha {
            return Ok(false);
        }
    }
    Ok(true)
}

fn deserialize_bounded_intent_inputs<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Vec<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "a sequence of at most {MAX_REPLACE_INTENT_INPUTS} bounded filenames"
            )
        }
        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let cap = seq.size_hint().unwrap_or(0).min(MAX_REPLACE_INTENT_INPUTS);
            let mut out = Vec::with_capacity(cap);
            let mut seen = 0_usize;
            while let Some(item) = seq.next_element::<BoundedFilename>()? {
                seen = seen.saturating_add(1);
                if seen > MAX_REPLACE_INTENT_INPUTS {
                    return Err(serde::de::Error::custom(format!(
                        "replace intent input count exceeds limit {MAX_REPLACE_INTENT_INPUTS}"
                    )));
                }
                if out.iter().any(|existing| existing == &item.0) {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate entry '{}' in replace intent inputs",
                        item.0
                    )));
                }
                out.push(item.0);
            }
            Ok(out)
        }
    }
    deserializer.deserialize_seq(Visitor)
}

fn deserialize_bounded_intent_input_ids<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, FileIdentity>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = BTreeMap<String, FileIdentity>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "a map of at most {MAX_REPLACE_INTENT_INPUTS} bounded input identities"
            )
        }
        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut out = BTreeMap::new();
            let mut seen = 0_usize;
            while let Some(k) = map.next_key::<BoundedFilename>()? {
                seen = seen.saturating_add(1);
                if seen > MAX_REPLACE_INTENT_INPUTS {
                    return Err(serde::de::Error::custom(format!(
                        "replace intent input_ids count exceeds limit {MAX_REPLACE_INTENT_INPUTS}"
                    )));
                }
                if out.contains_key(&k.0) {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate key '{}' in replace intent input_ids",
                        k.0
                    )));
                }
                let v = map.next_value::<FileIdentity>()?;
                out.insert(k.0, v);
            }
            Ok(out)
        }
    }
    deserializer.deserialize_map(Visitor)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ReplaceIntent {
    #[serde(deserialize_with = "deserialize_bounded_filename")]
    replacement: String,
    #[serde(default)]
    replacement_id: Option<FileIdentity>,
    #[serde(deserialize_with = "deserialize_bounded_intent_inputs")]
    inputs: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_bounded_intent_input_ids")]
    input_ids: BTreeMap<String, FileIdentity>,
    #[serde(default)]
    poisoned: bool,
    #[serde(default, deserialize_with = "deserialize_bounded_optional_filename")]
    staged_source: Option<String>,
}

fn btree_nodes_bytes<K, V>(len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    // Rust std BTree allocates nodes of 11 (K, V) slots + 12 edge pointers (B = 6).
    let entry_bytes = std::mem::size_of::<K>().saturating_add(std::mem::size_of::<V>());
    let node_bytes = 64_usize
        .saturating_add(11_usize.saturating_mul(entry_bytes))
        .saturating_add(12_usize.saturating_mul(std::mem::size_of::<usize>()));
    let node_count = len.div_ceil(5).saturating_add(1);
    node_count.saturating_mul(node_bytes)
}

impl ReplaceIntent {
    fn retained_heap_bytes(&self) -> usize {
        let mut total = std::mem::size_of::<Self>()
            .saturating_add(self.replacement.capacity())
            .saturating_add(
                self.replacement_id
                    .as_ref()
                    .and_then(|id| id.sha256.as_ref())
                    .map_or(0, String::capacity),
            )
            .saturating_add(
                self.inputs
                    .capacity()
                    .saturating_mul(std::mem::size_of::<String>()),
            )
            .saturating_add(self.staged_source.as_ref().map_or(0, String::capacity))
            .saturating_add(btree_nodes_bytes::<String, FileIdentity>(
                self.input_ids.len(),
            ));
        for input in &self.inputs {
            total = total.saturating_add(input.capacity());
        }
        for (k, v) in &self.input_ids {
            total = total
                .saturating_add(k.capacity())
                .saturating_add(v.sha256.as_ref().map_or(0, String::capacity));
        }
        total
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Ord, PartialOrd, Eq, PartialEq)]
struct PersistedUncommittedFile {
    name: String,
    #[serde(default)]
    watermark: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Ord, PartialOrd, Eq, PartialEq)]
struct PersistedPartitionRef {
    domain: String,
    day: String,
    #[serde(default)]
    files: Vec<PersistedUncommittedFile>,
}

/// Syncs directory metadata to disk after atomic file publication or removal.
pub(crate) fn sync_directory(dir: &Path) -> Result<(), String> {
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| format!("failed syncing dir {}: {e}", dir.display()))
}

pub(crate) fn reject_symlink_path(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(format!("refusing symlink path {}", path.display()));
            }
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("failed inspecting {}: {e}", path.display())),
    }
}

pub(crate) fn open_anchor_data_dir(layout: &StorageLayout) -> Result<File, String> {
    reject_symlink_path(layout.data_dir())?;
    let data_dir =
        crate::validate_non_tmp_path_on_disk(layout.data_dir()).map_err(|e| e.to_string())?;
    reject_symlink_path(&data_dir)?;
    let path_meta = fs::symlink_metadata(&data_dir)
        .map_err(|e| format!("failed inspecting data dir {}: {e}", data_dir.display()))?;
    if path_meta.file_type().is_symlink() || !path_meta.is_dir() {
        return Err(format!("invalid data dir {}", data_dir.display()));
    }
    let dir_file = OpenOptions::new()
        .read(true)
        .custom_flags(O_DIRECTORY_FLAG | O_NOFOLLOW_FLAG | O_NONBLOCK_FLAG)
        .open(&data_dir)
        .map_err(|e| {
            if is_eloop_error(&e) {
                format!("refusing symlinked data dir {}", data_dir.display())
            } else {
                format!("failed opening data dir {}: {e}", data_dir.display())
            }
        })?;
    let fd_meta = dir_file
        .metadata()
        .map_err(|e| format!("failed inspecting data dir fd {}: {e}", data_dir.display()))?;
    if fd_meta.file_type().is_symlink()
        || !fd_meta.is_dir()
        || fd_meta.dev() != path_meta.dev()
        || fd_meta.ino() != path_meta.ino()
    {
        return Err(format!(
            "data dir {} changed concurrently during open",
            data_dir.display()
        ));
    }
    Ok(dir_file)
}

pub(crate) fn open_anchor_data_dir_if_exists(
    layout: &StorageLayout,
) -> Result<Option<File>, String> {
    reject_symlink_path(layout.data_dir())?;
    let data_dir =
        crate::validate_non_tmp_path_on_disk(layout.data_dir()).map_err(|e| e.to_string())?;
    match fs::symlink_metadata(&data_dir) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        _ => open_anchor_data_dir(layout).map(Some),
    }
}

pub(crate) fn open_standalone_dir_nofollow(
    dir: &Path,
    label: &str,
) -> Result<Option<File>, String> {
    let path_meta = match fs::symlink_metadata(dir) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!("failed inspecting {label} {}: {e}", dir.display()));
        }
        Ok(meta) => meta,
    };
    if path_meta.file_type().is_symlink() {
        return Err(format!("refusing symlinked {label} {}", dir.display()));
    }
    if !path_meta.is_dir() {
        return Err(format!("non-directory {label} {}", dir.display()));
    }
    let dir_file = match OpenOptions::new()
        .read(true)
        .custom_flags(O_DIRECTORY_FLAG | O_NOFOLLOW_FLAG | O_NONBLOCK_FLAG)
        .open(dir)
    {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) if is_eloop_error(&e) => {
            return Err(format!("refusing symlinked {label} {}", dir.display()));
        }
        Err(e) if is_enotdir_error(&e) => {
            if matches!(
                fs::symlink_metadata(dir),
                Ok(meta) if meta.file_type().is_symlink()
            ) {
                return Err(format!("refusing symlinked {label} {}", dir.display()));
            }
            return Err(format!("non-directory {label} {}", dir.display()));
        }
        Err(e) => return Err(format!("failed opening {label} {}: {e}", dir.display())),
    };
    let fd_meta = dir_file
        .metadata()
        .map_err(|e| format!("failed inspecting {label} fd {}: {e}", dir.display()))?;
    if fd_meta.file_type().is_symlink() {
        return Err(format!("refusing symlinked {label} {}", dir.display()));
    }
    if !fd_meta.is_dir() {
        return Err(format!("non-directory {label} {}", dir.display()));
    }
    if fd_meta.dev() != path_meta.dev() || fd_meta.ino() != path_meta.ino() {
        return Err(format!(
            "{label} {} changed concurrently during open",
            dir.display()
        ));
    }
    Ok(Some(dir_file))
}

pub(crate) fn open_olap_domain_dir_nofollow(
    layout: &StorageLayout,
    domain: &str,
) -> Result<Option<File>, String> {
    let domain = validate_olap_domain(domain)?;
    let Some(data_fd) = open_anchor_data_dir_if_exists(layout)? else {
        return Ok(None);
    };
    let Some(olap_fd) = open_child_dir_nofollow(&data_fd, "olap", "olap dir")? else {
        return Ok(None);
    };
    open_child_dir_nofollow(&olap_fd, domain, "domain dir")
}

pub(crate) fn open_olap_partition_dir_nofollow(
    layout: &StorageLayout,
    domain: &str,
    day: NaiveDate,
) -> Result<Option<File>, String> {
    let Some(domain_fd) = open_olap_domain_dir_nofollow(layout, domain)? else {
        return Ok(None);
    };
    let part_name = format!("dt={}", day.format("%Y-%m-%d"));
    open_child_dir_nofollow(&domain_fd, &part_name, "partition dir")
}

pub(crate) fn open_regular_file_in_dir_nofollow(
    dir_fd: &File,
    filename: &str,
    label: &str,
) -> Result<Option<(File, fs::Metadata)>, String> {
    let child_proc = proc_fd_child_path(dir_fd, filename, label)?;
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW_FLAG | O_NONBLOCK_FLAG)
        .open(&child_proc)
    {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) if is_eloop_error(&e) => {
            return Err(format!("refusing symlinked {label} {filename}"));
        }
        Err(e) => return Err(format!("failed opening {label} {filename}: {e}")),
    };
    let fd_meta = file
        .metadata()
        .map_err(|e| format!("failed inspecting fd for {label} {filename}: {e}"))?;
    if fd_meta.file_type().is_symlink() {
        return Err(format!("refusing symlinked {label} {filename}"));
    }
    if !fd_meta.is_file() {
        return Err(format!("non-regular {label} {filename}"));
    }
    let path_meta = match fs::symlink_metadata(&child_proc) {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(format!(
                "{label} {filename} changed concurrently during open"
            ));
        }
        Err(e) => return Err(format!("failed inspecting {label} {filename}: {e}")),
    };
    if path_meta.file_type().is_symlink() {
        return Err(format!("refusing symlinked {label} {filename}"));
    }
    if !path_meta.is_file() {
        return Err(format!("non-regular {label} {filename}"));
    }
    if path_meta.dev() != fd_meta.dev() || path_meta.ino() != fd_meta.ino() {
        return Err(format!(
            "{label} {filename} changed concurrently during open"
        ));
    }
    Ok(Some((file, fd_meta)))
}

pub(crate) fn is_regular_file_in_dir_no_symlink(
    dir_fd: &File,
    filename: &str,
    label: &str,
) -> Result<bool, String> {
    let child_proc = proc_fd_child_path(dir_fd, filename, label)?;
    match fs::symlink_metadata(&child_proc) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("failed inspecting {label} {filename}: {e}")),
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(format!("refusing symlinked {label} {filename}"))
        }
        Ok(meta) => Ok(meta.is_file()),
    }
}

pub(crate) fn inspect_regular_file_identity_in_dir_no_symlink(
    dir_fd: &File,
    filename: &str,
    label: &str,
) -> Result<Option<FileIdentity>, String> {
    let child_proc = proc_fd_child_path(dir_fd, filename, label)?;
    match fs::symlink_metadata(&child_proc) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("failed inspecting {label} {filename}: {e}")),
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(format!("refusing symlinked {label} {filename}"))
        }
        Ok(meta) if !meta.is_file() => Err(format!("non-regular {label} {filename}")),
        Ok(meta) => Ok(Some(FileIdentity {
            dev: meta.dev(),
            ino: meta.ino(),
            len: Some(meta.len()),
            sha256: None,
        })),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SupersededUnlinkOutcome {
    Removed,
    AlreadyAbsent,
    IdentityMismatch,
}

/// Unlinks `filename` under `dir_fd` only after verifying `symlink_metadata`, `O_NOFOLLOW`,
/// `(dev, ino, len)`, and optional `sha256` match, distinguishing missing files from live
/// identity/digest mismatches so fail-closed reconciliation preserves intents on substitution.
pub(crate) fn unlink_superseded_file_in_dir(
    dir_fd: &File,
    filename: &str,
    expected: Option<&FileIdentity>,
    label: &str,
) -> Result<SupersededUnlinkOutcome, String> {
    let child_proc = proc_fd_child_path(dir_fd, filename, label)?;
    let Some((pinned_file, fd_meta)) = open_regular_file_in_dir_nofollow(dir_fd, filename, label)?
    else {
        return Ok(SupersededUnlinkOutcome::AlreadyAbsent);
    };
    if let Some(exp) = expected
        && !verify_open_file_matches_identity(&pinned_file, &fd_meta, exp)?
    {
        return Ok(SupersededUnlinkOutcome::IdentityMismatch);
    }
    match fs::symlink_metadata(&child_proc) {
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Ok(SupersededUnlinkOutcome::AlreadyAbsent);
        }
        Err(e) => return Err(format!("failed inspecting {label} {filename}: {e}")),
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(format!("refusing symlinked {label} {filename}"));
        }
        Ok(meta) if !meta.is_file() => {
            return Err(format!("non-regular {label} {filename}"));
        }
        Ok(meta)
            if meta.dev() != fd_meta.dev()
                || meta.ino() != fd_meta.ino()
                || meta.len() != fd_meta.len() =>
        {
            return Ok(SupersededUnlinkOutcome::IdentityMismatch);
        }
        Ok(_) => {}
    }
    let outcome = match fs::remove_file(&child_proc) {
        Ok(()) => Ok(SupersededUnlinkOutcome::Removed),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(SupersededUnlinkOutcome::AlreadyAbsent),
        Err(e) => Err(format!("failed removing {label} {filename}: {e}")),
    };
    drop(pinned_file);
    outcome
}

/// Unlinks `filename` under `dir_fd` only after verifying `symlink_metadata`, `O_NOFOLLOW`,
/// `(dev, ino, len)`, and optional `sha256` match. All callers deleting partition or sidecar
/// files under active maintenance hold an exclusive `MaintenanceLockGuard`, while POSIX/Linux
/// offers no `funlink(fd)` syscall against external uncooperative directory writers.
pub(crate) fn unlink_regular_file_matching_identity_in_dir(
    dir_fd: &File,
    filename: &str,
    expected: Option<&FileIdentity>,
    label: &str,
) -> Result<bool, String> {
    Ok(matches!(
        unlink_superseded_file_in_dir(dir_fd, filename, expected, label)?,
        SupersededUnlinkOutcome::Removed
    ))
}

pub(crate) fn unlink_regular_file_in_dir_if_present(
    dir_fd: &File,
    filename: &str,
    label: &str,
) -> Result<bool, String> {
    unlink_regular_file_matching_identity_in_dir(dir_fd, filename, None, label)
}

fn arc_alloc_bytes(payload_bytes: usize) -> usize {
    ARC_HEADER_BYTES
        .saturating_add(payload_bytes)
        .next_multiple_of(std::mem::size_of::<usize>())
}

fn arc_file_heap_bytes() -> usize {
    arc_alloc_bytes(std::mem::size_of::<File>())
}

#[derive(Debug)]
pub(crate) struct PinnedDomainPartition {
    domain_dir_fd: Arc<File>,
    part_name: Arc<str>,
    part_dev: u64,
    part_ino: u64,
}

impl PinnedDomainPartition {
    fn retained_heap_bytes(&self) -> usize {
        arc_alloc_bytes(std::mem::size_of::<Self>())
            .saturating_add(arc_alloc_bytes(self.part_name.len()))
    }
}

#[derive(Debug, Clone)]
pub(crate) enum PinnedDirAnchor {
    Domain(Arc<PinnedDomainPartition>),
    Partition { part_dir_fd: Arc<File> },
}

#[derive(Debug, Clone)]
pub(crate) struct PinnedAuxiliaryFile {
    pub name: String,
    pub dev: u64,
    pub ino: u64,
    pub len: u64,
    pub sha256: Option<String>,
    pub _file: Arc<File>,
}

#[derive(Debug, Clone)]
struct PinnedParquetFileInner {
    anchor: PinnedDirAnchor,
    file_name: Arc<str>,
    display_path: Arc<Path>,
    dev: u64,
    ino: u64,
    len: u64,
    sha256: Option<Arc<str>>,
    _retained_fd: Option<Arc<File>>,
}

/// Pinned Parquet partition entry anchored by a verified domain or partition descriptor.
#[derive(Debug, Clone)]
pub struct PinnedParquetFile {
    inner: Arc<PinnedParquetFileInner>,
}

struct SnapshotAnchorTracker {
    part_stack: [usize; 64],
    part_stack_len: usize,
    part_overflow: Option<Vec<usize>>,
    fd_stack: [usize; 8],
    fd_stack_len: usize,
    fd_overflow: Option<Vec<usize>>,
}

impl SnapshotAnchorTracker {
    fn new() -> Self {
        Self {
            part_stack: [0; 64],
            part_stack_len: 0,
            part_overflow: None,
            fd_stack: [0; 8],
            fd_stack_len: 0,
            fd_overflow: None,
        }
    }

    fn insert_part(&mut self, ptr: usize) -> bool {
        if self.part_stack[..self.part_stack_len].contains(&ptr) {
            return false;
        }
        if self.part_stack_len < self.part_stack.len() {
            self.part_stack[self.part_stack_len] = ptr;
            self.part_stack_len += 1;
            return true;
        }
        let overflow = self.part_overflow.get_or_insert_with(Vec::new);
        if overflow.contains(&ptr) {
            return false;
        }
        overflow.push(ptr);
        true
    }

    fn insert_fd(&mut self, ptr: usize) -> bool {
        if self.fd_stack[..self.fd_stack_len].contains(&ptr) {
            return false;
        }
        if self.fd_stack_len < self.fd_stack.len() {
            self.fd_stack[self.fd_stack_len] = ptr;
            self.fd_stack_len += 1;
            return true;
        }
        let overflow = self.fd_overflow.get_or_insert_with(Vec::new);
        if overflow.contains(&ptr) {
            return false;
        }
        overflow.push(ptr);
        true
    }

    fn account_anchor(&mut self, anchor: &PinnedDirAnchor) -> usize {
        match anchor {
            PinnedDirAnchor::Domain(part) => {
                let mut bytes = 0_usize;
                if self.insert_part(Arc::as_ptr(part) as usize) {
                    bytes = bytes.saturating_add(part.retained_heap_bytes());
                }
                if self.insert_fd(Arc::as_ptr(&part.domain_dir_fd) as usize) {
                    bytes = bytes.saturating_add(arc_file_heap_bytes());
                }
                bytes
            }
            PinnedDirAnchor::Partition { part_dir_fd } => {
                if self.insert_fd(Arc::as_ptr(part_dir_fd) as usize) {
                    arc_file_heap_bytes()
                } else {
                    0
                }
            }
        }
    }
}

/// Returns the total retained heap bytes across `files`, counting each shared
/// per-partition domain anchor and directory `Arc<File>` once.
#[must_use]
pub fn snapshot_retained_heap_bytes(files: &[PinnedParquetFile]) -> usize {
    let mut tracker = SnapshotAnchorTracker::new();
    let mut total = 0_usize;
    for file in files {
        total = total
            .saturating_add(file.retained_heap_bytes())
            .saturating_add(tracker.account_anchor(&file.inner.anchor));
    }
    total
}

/// Returns the total retained heap bytes across partitioned `Arc<[Arc<[PinnedParquetFile]>]>`
/// slices, including outer/inner slice headers, shared anchors, and `Arc<File>` handles.
#[must_use]
pub fn snapshot_partitions_retained_heap_bytes(partitions: &[Arc<[PinnedParquetFile]>]) -> usize {
    if partitions.is_empty() {
        return 0;
    }
    let mut tracker = SnapshotAnchorTracker::new();
    let mut total = arc_alloc_bytes(
        partitions
            .len()
            .saturating_mul(std::mem::size_of::<Arc<[PinnedParquetFile]>>()),
    );
    for chunk in partitions {
        total = total.saturating_add(ARC_HEADER_BYTES);
        for file in chunk.iter() {
            total = total
                .saturating_add(file.retained_heap_bytes())
                .saturating_add(tracker.account_anchor(&file.inner.anchor));
        }
    }
    total
}

impl PinnedParquetFile {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_pinned_in_partition_with_file(
        part_dir_fd: Arc<File>,
        file_name: String,
        display_path: PathBuf,
        dev: u64,
        ino: u64,
        len: u64,
        sha256: Option<String>,
        file: Arc<File>,
    ) -> Self {
        Self {
            inner: Arc::new(PinnedParquetFileInner {
                anchor: PinnedDirAnchor::Partition { part_dir_fd },
                file_name: Arc::from(file_name),
                display_path: Arc::from(display_path.into_boxed_path()),
                dev,
                ino,
                len,
                sha256: sha256.map(Arc::from),
                _retained_fd: Some(file),
            }),
        }
    }

    fn estimate_retained_heap_bytes(
        file_name_len: usize,
        display_path_len: usize,
        sha256_len: Option<usize>,
        has_retained_fd: bool,
    ) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(arc_alloc_bytes(
                std::mem::size_of::<PinnedParquetFileInner>(),
            ))
            .saturating_add(arc_alloc_bytes(file_name_len))
            .saturating_add(arc_alloc_bytes(display_path_len))
            .saturating_add(sha256_len.map_or(0, arc_alloc_bytes))
            .saturating_add(if has_retained_fd {
                arc_file_heap_bytes()
            } else {
                0
            })
    }

    /// Returns the exact per-entry retained heap byte footprint.
    #[must_use]
    pub fn retained_heap_bytes(&self) -> usize {
        Self::estimate_retained_heap_bytes(
            self.inner.file_name.len(),
            self.inner.display_path.as_os_str().len(),
            self.inner.sha256.as_deref().map(str::len),
            self.inner._retained_fd.is_some(),
        )
    }

    /// Returns the display path for diagnostics and sorting.
    #[must_use]
    pub fn display_path(&self) -> &Path {
        &self.inner.display_path
    }

    /// Returns the validated filename inside the pinned partition directory.
    #[must_use]
    pub fn file_name(&self) -> &str {
        &self.inner.file_name
    }

    pub(crate) fn file_identity(&self) -> FileIdentity {
        FileIdentity {
            dev: self.inner.dev,
            ino: self.inner.ino,
            len: Some(self.inner.len),
            sha256: self.inner.sha256.as_deref().map(str::to_owned),
        }
    }

    /// Opens the pinned Parquet file non-blockingly without following symlinks and verifies
    /// `(dev, ino, len)` and optional `sha256` identity against the snapshot.
    pub fn open_verified(&self) -> Result<File, String> {
        let opened = match &self.inner.anchor {
            PinnedDirAnchor::Domain(part) => {
                let Some(part_fd) =
                    open_child_dir_nofollow(&part.domain_dir_fd, &part.part_name, "partition dir")?
                else {
                    return Err(format!(
                        "NotFound: partition dir for {} disappeared during open",
                        self.inner.display_path.display()
                    ));
                };
                let part_meta = part_fd.metadata().map_err(|e| {
                    format!(
                        "failed inspecting partition dir for {}: {e}",
                        self.inner.display_path.display()
                    )
                })?;
                if part_meta.dev() != part.part_dev || part_meta.ino() != part.part_ino {
                    return Err(format!(
                        "partition dir for {} changed concurrently during open",
                        self.inner.display_path.display()
                    ));
                }
                open_regular_file_in_dir_nofollow(&part_fd, &self.inner.file_name, "parquet file")?
            }
            PinnedDirAnchor::Partition { part_dir_fd } => open_regular_file_in_dir_nofollow(
                part_dir_fd,
                &self.inner.file_name,
                "parquet file",
            )?,
        };
        match opened {
            None => Err(format!(
                "NotFound: parquet file {} disappeared during open",
                self.inner.display_path.display()
            )),
            Some((file, meta)) => {
                if meta.dev() != self.inner.dev
                    || meta.ino() != self.inner.ino
                    || meta.len() != self.inner.len
                {
                    return Err(format!(
                        "parquet file {} changed concurrently during open",
                        self.inner.display_path.display()
                    ));
                }
                if let Some(ref exp_sha) = self.inner.sha256 {
                    let (actual_len, actual_sha) = sha256_regular_file(&file)?;
                    if actual_len != self.inner.len || actual_sha.as_str() != exp_sha.as_ref() {
                        return Err(format!(
                            "parquet file {} content digest changed concurrently during open",
                            self.inner.display_path.display()
                        ));
                    }
                }
                Ok(file)
            }
        }
    }
}

pub(crate) fn proc_fd_dir_path(dir_fd: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", dir_fd.as_raw_fd()))
}

pub(crate) fn proc_fd_child_path(
    dir_fd: &File,
    child_name: &str,
    label: &str,
) -> Result<PathBuf, String> {
    validate_single_normal_filename(child_name, label)?;
    Ok(PathBuf::from(format!(
        "/proc/self/fd/{}/{child_name}",
        dir_fd.as_raw_fd()
    )))
}

pub(crate) fn open_child_dir_nofollow(
    parent_fd: &File,
    child_name: &str,
    label: &str,
) -> Result<Option<File>, String> {
    let child_path = proc_fd_child_path(parent_fd, child_name, label)?;
    let dir_file = match OpenOptions::new()
        .read(true)
        .custom_flags(O_DIRECTORY_FLAG | O_NOFOLLOW_FLAG | O_NONBLOCK_FLAG)
        .open(&child_path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) if is_eloop_error(&e) => {
            return Err(format!("refusing symlinked {label} {child_name}"));
        }
        Err(e) if is_enotdir_error(&e) => {
            if matches!(
                fs::symlink_metadata(&child_path),
                Ok(meta) if meta.file_type().is_symlink()
            ) {
                return Err(format!("refusing symlinked {label} {child_name}"));
            }
            return Err(format!("non-directory {label} {child_name}"));
        }
        Err(e) => return Err(format!("failed opening {label} {child_name}: {e}")),
    };
    let meta = dir_file
        .metadata()
        .map_err(|e| format!("failed inspecting {label} {child_name} fd: {e}"))?;
    if meta.file_type().is_symlink() {
        return Err(format!("refusing symlinked {label} {child_name}"));
    }
    if !meta.is_dir() {
        return Err(format!("non-directory {label} {child_name}"));
    }
    Ok(Some(dir_file))
}

pub(crate) fn is_regular_file_no_symlink(path: &Path, label: &str) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("failed inspecting {label} {}: {e}", path.display())),
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(format!("refusing symlinked {label} {}", path.display()))
        }
        Ok(meta) => Ok(meta.is_file()),
    }
}

pub(crate) fn validate_olap_domain(domain: &str) -> Result<&'static str, String> {
    OLAP_DOMAINS
        .iter()
        .copied()
        .find(|&allowed| allowed == domain)
        .ok_or_else(|| format!("unsupported or invalid OLAP domain {domain:?}"))
}

fn validate_single_normal_filename(name: &str, label: &str) -> Result<(), String> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        return Err(format!("invalid {label} filename {name:?}"));
    }
    let mut comps = Path::new(name).components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(seg)), None) if seg == std::ffi::OsStr::new(name) => Ok(()),
        _ => Err(format!("invalid {label} filename {name:?}")),
    }
}

pub(crate) fn validate_parquet_filename(name: &str, label: &str) -> Result<(), String> {
    validate_single_normal_filename(name, label)?;
    if !name.ends_with(".parquet") {
        return Err(format!("invalid {label} filename {name:?}"));
    }
    Ok(())
}

fn validate_replace_input_filename(name: &str, label: &str) -> Result<(), String> {
    validate_single_normal_filename(name, label)?;
    if name.ends_with(".parquet") {
        return Ok(());
    }
    if let Some(inner) = name
        .strip_prefix(".rollup-counts-")
        .and_then(|s| s.strip_suffix(".json"))
        && validate_parquet_filename(inner, label).is_ok()
    {
        return Ok(());
    }
    Err(format!("invalid {label} filename {name:?}"))
}

fn validate_staged_source_filename(name: &str, label: &str) -> Result<(), String> {
    validate_single_normal_filename(name, label)?;
    if !name.starts_with(".replace-source-") || !name.ends_with(".staged") {
        return Err(format!("invalid {label} filename {name:?}"));
    }
    Ok(())
}

fn cleanup_orphan_staged_sources_on_fd(part_fd: &File) -> Result<(), String> {
    let mut removed_any = false;
    for entry in fs::read_dir(proc_fd_dir_path(part_fd)).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if validate_staged_source_filename(&name, "orphan staged source").is_ok()
            && unlink_regular_file_in_dir_if_present(part_fd, &name, "orphan staged source")?
        {
            removed_any = true;
        }
    }
    if removed_any {
        part_fd
            .sync_all()
            .map_err(|e| format!("failed syncing orphan staged source cleanup: {e}"))?;
    }
    Ok(())
}

pub(crate) fn reject_symlinks_below_stop(stop: Option<&Path>, dir: &Path) -> Result<(), String> {
    let Some(stop) = stop else {
        return Ok(());
    };
    if dir
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(format!(
            "refusing non-normalized path component in {}",
            dir.display()
        ));
    }
    if !dir.starts_with(stop) {
        return Err(format!(
            "path {} is outside stop {}",
            dir.display(),
            stop.display()
        ));
    }
    let mut cur = dir;
    loop {
        reject_symlink_path(cur)?;
        if cur == stop {
            break;
        }
        let Some(parent) = cur.parent() else {
            return Err(format!(
                "path {} escaped stop {}",
                dir.display(),
                stop.display()
            ));
        };
        cur = parent;
    }
    Ok(())
}

fn build_durable_sync_chain(resolved: &Path, stop_resolved: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs_to_sync: Vec<PathBuf> = Vec::new();
    let mut cursor: Option<&Path> = Some(resolved);
    while let Some(p) = cursor {
        if p.as_os_str().is_empty() {
            break;
        }
        if p == Path::new("/") {
            if !dirs_to_sync.is_empty() {
                dirs_to_sync.push(PathBuf::from("/"));
            }
            break;
        }
        dirs_to_sync.push(p.to_path_buf());
        if let Some(stop) = stop_resolved
            && p == stop
            && p.exists()
            && p != resolved
        {
            break;
        }
        cursor = p.parent();
    }
    dirs_to_sync.into_iter().rev().collect()
}

fn ensure_durable_non_tmp_dir_with(
    sync_stop: Option<&Path>,
    dir: &Path,
    mut sync_fn: impl FnMut(&Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    reject_symlinks_below_stop(sync_stop, dir)?;
    let stop_resolved = match sync_stop {
        Some(stop) => Some(crate::validate_non_tmp_path_on_disk(stop).map_err(|e| e.to_string())?),
        None => None,
    };
    let resolved = crate::validate_non_tmp_path_on_disk(dir).map_err(|e| e.to_string())?;
    let chain = build_durable_sync_chain(&resolved, stop_resolved.as_deref());

    for (idx, dir_path) in chain.iter().enumerate() {
        if dir_path != Path::new("/") {
            let mut builder = fs::DirBuilder::new();
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            let created = match builder.create(dir_path) {
                Ok(()) => true,
                Err(e) if e.kind() == ErrorKind::AlreadyExists => false,
                Err(e) => return Err(format!("failed creating {}: {e}", dir_path.display())),
            };
            reject_symlink_path(dir_path)?;
            let is_custody_dir = created
                || (sync_stop.is_some()
                    && stop_resolved
                        .as_deref()
                        .is_some_and(|stop| dir_path != stop))
                || (dir_path == resolved.as_path()
                    && !matches!(
                        dir_path.to_str(),
                        Some("/" | "/data" | "/data/tmp" | "/var/tmp")
                    )
                    && dir_path != std::env::temp_dir().as_path());
            if is_custody_dir {
                crate::atomic_fd::enforce_private_dir_path_mode(dir_path, "custody dir")?;
            }
        }
        if idx > 0 {
            sync_fn(&chain[idx - 1])?;
        }
    }
    if let Some(last) = chain.last() {
        sync_fn(last)?;
    }
    reject_symlinks_below_stop(sync_stop, dir)?;
    Ok(resolved)
}

/// Validates symlinks/non-tmp placement, creates `dir`, and unconditionally syncs
/// every ancestor directory top-down from `sync_stop` (or `/`, when `sync_stop` is `None`) to `dir`.
pub fn ensure_durable_non_tmp_dir(sync_stop: Option<&Path>, dir: &Path) -> Result<PathBuf, String> {
    ensure_durable_non_tmp_dir_with(sync_stop, dir, sync_directory)
}

/// Ensures `subdir` resides cleanly inside `layout.data_dir()` with no symlinks at or below
/// `data_dir` and with every directory ancestor durably synced through `/`.
pub fn ensure_durable_layout_subdir(
    layout: &StorageLayout,
    subdir: &Path,
) -> Result<PathBuf, String> {
    if subdir
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(format!(
            "refusing non-normalized path component in {}",
            subdir.display()
        ));
    }
    let data_dir = layout.data_dir();
    if !subdir.starts_with(data_dir) {
        return Err(format!(
            "subdir {} is outside storage root {}",
            subdir.display(),
            data_dir.display()
        ));
    }
    reject_symlink_path(data_dir)?;
    let resolved_root = ensure_durable_non_tmp_dir(None, data_dir)?;
    reject_symlink_path(data_dir)?;
    let root_meta_before = fs::symlink_metadata(&resolved_root).map_err(|e| {
        format!(
            "failed inspecting storage root {}: {e}",
            resolved_root.display()
        )
    })?;
    if root_meta_before.file_type().is_symlink() || !root_meta_before.is_dir() {
        return Err(format!(
            "invalid non-directory storage root {}",
            resolved_root.display()
        ));
    }
    if subdir == data_dir {
        return Ok(resolved_root);
    }
    let rel = subdir
        .strip_prefix(data_dir)
        .map_err(|e| format!("invalid subdir {}: {e}", subdir.display()))?;
    for comp in rel.components() {
        match comp {
            Component::Normal(seg) if !seg.is_empty() && seg != "." && seg != ".." => {}
            _ => {
                return Err(format!(
                    "invalid relative subdir component in {}",
                    subdir.display()
                ));
            }
        }
    }
    let resolved_target = resolved_root.join(rel);
    reject_symlinks_below_stop(Some(data_dir), subdir)?;
    reject_symlinks_below_stop(Some(&resolved_root), &resolved_target)?;
    let final_resolved = ensure_durable_non_tmp_dir(Some(&resolved_root), &resolved_target)?;
    if final_resolved != resolved_root && !final_resolved.starts_with(&resolved_root) {
        return Err(format!(
            "resolved subdir {} escaped storage root {}",
            final_resolved.display(),
            resolved_root.display()
        ));
    }
    reject_symlinks_below_stop(Some(&resolved_root), &final_resolved)?;
    let root_meta_after = fs::symlink_metadata(&resolved_root).map_err(|e| {
        format!(
            "failed re-inspecting storage root {}: {e}",
            resolved_root.display()
        )
    })?;
    if root_meta_after.file_type().is_symlink()
        || !root_meta_after.is_dir()
        || root_meta_after.dev() != root_meta_before.dev()
        || root_meta_after.ino() != root_meta_before.ino()
    {
        return Err(format!(
            "storage root changed concurrently during subdir creation {}",
            resolved_root.display()
        ));
    }
    Ok(final_resolved)
}

#[derive(Clone, Copy)]
enum LockMode {
    ExclusiveWait,
    ExclusiveTry,
    SharedWait,
    ExclusiveProbe,
}

fn open_and_lock_state_file(
    layout: &StorageLayout,
    lock_filename: &str,
    mode: LockMode,
) -> Result<Option<File>, String> {
    validate_single_normal_filename(lock_filename, "lockfile")?;
    for _ in 0..4 {
        let data_fd = open_anchor_data_dir(layout)?;
        let Some(state_fd) = open_child_dir_nofollow(&data_fd, "state", "state dir")? else {
            return Err(format!(
                "missing state dir {}",
                layout.state_dir().display()
            ));
        };
        let lock_proc = proc_fd_child_path(&state_fd, lock_filename, "lockfile")?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(O_NOFOLLOW_FLAG | O_NONBLOCK_FLAG)
            .open(&lock_proc)
            .map_err(|e| {
                if is_eloop_error(&e) {
                    format!("refusing symlinked lockfile {lock_filename}")
                } else {
                    format!("failed opening lockfile {lock_filename}: {e}")
                }
            })?;
        let fd_meta = file
            .metadata()
            .map_err(|e| format!("failed inspecting lockfile fd {lock_filename}: {e}"))?;
        if fd_meta.file_type().is_symlink() {
            return Err(format!("refusing symlink lockfile {lock_filename}"));
        }
        if !fd_meta.is_file() {
            return Err(format!("non-regular lockfile {lock_filename}"));
        }
        match mode {
            LockMode::ExclusiveWait => {
                file.lock()
                    .map_err(|e| format!("failed locking {lock_filename}: {e}"))?;
            }
            LockMode::ExclusiveTry => match file.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => {
                    return Err(format!(
                        "storage writer lock busy on {lock_filename} (another fabric daemon or writer is active)"
                    ));
                }
                Err(TryLockError::Error(e)) => {
                    return Err(format!("failed locking {lock_filename}: {e}"));
                }
            },
            LockMode::SharedWait => {
                file.lock_shared()
                    .map_err(|e| format!("failed shared-locking {lock_filename}: {e}"))?;
            }
            LockMode::ExclusiveProbe => match file.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => return Ok(None),
                Err(TryLockError::Error(e)) => {
                    return Err(format!("failed probing lock {lock_filename}: {e}"));
                }
            },
        }
        let path_meta = match fs::symlink_metadata(&lock_proc) {
            Ok(meta) => meta,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(format!("failed inspecting lockfile {lock_filename}: {e}"));
            }
        };
        if path_meta.file_type().is_symlink() {
            return Err(format!("refusing symlink lockfile {lock_filename}"));
        }
        if !path_meta.is_file() {
            return Err(format!("non-regular lockfile {lock_filename}"));
        }
        if path_meta.dev() == fd_meta.dev() && path_meta.ino() == fd_meta.ino() {
            return Ok(Some(file));
        }
    }
    Err(format!(
        "failed acquiring non-symlinked lock on {lock_filename}"
    ))
}

/// Exclusive writer lock guard pairing `state/daemon.lock` with a shared live-writer signal on
/// `state/.uncommitted.lock` so out-of-process maintenance respects uncommitted flush batches.
#[derive(Debug)]
pub struct WriterLockGuard {
    _daemon_lock: File,
    _uncommitted_lock: File,
}

/// Acquires an exclusive non-blocking `flock` preventing multiple active daemons from sharing a `data_dir`.
pub fn try_acquire_writer_lock(layout: &StorageLayout) -> Result<WriterLockGuard, String> {
    let _state_dir = ensure_durable_layout_subdir(layout, &layout.state_dir())?;
    let daemon_lock = open_and_lock_state_file(layout, "daemon.lock", LockMode::ExclusiveTry)?
        .ok_or_else(|| "failed acquiring daemon writer lock".to_owned())?;
    let maint = acquire_maintenance_lock(layout)?;
    let Some(uncommitted_lock) =
        open_and_lock_state_file(layout, ".uncommitted.lock", LockMode::ExclusiveProbe)?
    else {
        return Err(
            "storage writer lock busy on .uncommitted.lock (another live flush plan is active)"
                .to_owned(),
        );
    };
    reclaim_stale_uncommitted_unlocked(layout, &maint)?;
    uncommitted_lock
        .lock_shared()
        .map_err(|e| format!("failed downgrading .uncommitted.lock: {e}"))?;
    drop(maint);
    Ok(WriterLockGuard {
        _daemon_lock: daemon_lock,
        _uncommitted_lock: uncommitted_lock,
    })
}

/// Exclusive proof token holding `state/.olap-maintenance.lock` for partition mutations.
#[derive(Debug)]
pub(crate) struct MaintenanceLockGuard(#[allow(dead_code)] File);

pub(crate) fn acquire_maintenance_lock(
    layout: &StorageLayout,
) -> Result<MaintenanceLockGuard, String> {
    let _ = ensure_durable_layout_subdir(layout, &layout.state_dir())?;
    open_and_lock_state_file(layout, ".olap-maintenance.lock", LockMode::ExclusiveWait)?
        .map(MaintenanceLockGuard)
        .ok_or_else(|| "failed acquiring maintenance lock".to_owned())
}

type UncommittedRefMap = BTreeMap<(String, NaiveDate), BTreeSet<PersistedUncommittedFile>>;

fn open_state_dir_fd_for_write(layout: &StorageLayout) -> Result<(PathBuf, File), String> {
    let state_dir = ensure_durable_layout_subdir(layout, &layout.state_dir())?;
    let data_fd = open_anchor_data_dir(layout)?;
    let state_fd = open_child_dir_nofollow(&data_fd, "state", "state dir")?
        .ok_or_else(|| format!("missing state dir {}", state_dir.display()))?;
    Ok((state_dir, state_fd))
}

fn read_uncommitted_refs_on_fd(
    state_fd: &File,
    display_state_dir: &Path,
) -> Result<UncommittedRefMap, String> {
    let display_path = display_state_dir.join(UNCOMMITTED_PARTITIONS_FILE);
    let Some(bytes) = read_non_symlink_bytes_in_dir_bounded_if_present(
        state_fd,
        UNCOMMITTED_PARTITIONS_FILE,
        "uncommitted partitions",
        MAX_UNCOMMITTED_PARTITIONS_BYTES,
    )?
    else {
        return Ok(BTreeMap::new());
    };
    let entries: Vec<PersistedPartitionRef> = serde_json::from_slice(&bytes)
        .map_err(|e| format!("corrupt {}: {e}", display_path.display()))?;
    let mut out: UncommittedRefMap = BTreeMap::new();
    for entry in entries {
        let domain = validate_olap_domain(&entry.domain)?.to_owned();
        let day = NaiveDate::parse_from_str(&entry.day, "%Y-%m-%d")
            .map_err(|e| format!("invalid partition day in {}: {e}", display_path.display()))?;
        let set = out.entry((domain, day)).or_default();
        for file in entry.files {
            validate_parquet_filename(&file.name, "uncommitted parquet")?;
            set.insert(file);
        }
    }
    Ok(out)
}

fn write_uncommitted_refs_on_fd(state_fd: &File, refs: &UncommittedRefMap) -> Result<(), String> {
    if refs.is_empty() {
        if unlink_regular_file_in_dir_if_present(
            state_fd,
            UNCOMMITTED_PARTITIONS_FILE,
            "uncommitted partitions marker",
        )? {
            state_fd
                .sync_all()
                .map_err(|e| format!("failed syncing state dir: {e}"))?;
        }
        return Ok(());
    }
    let entries: Vec<PersistedPartitionRef> = refs
        .iter()
        .map(|((domain, day), files)| PersistedPartitionRef {
            domain: domain.clone(),
            day: day.format("%Y-%m-%d").to_string(),
            files: files.iter().cloned().collect(),
        })
        .collect();
    write_atomic_json_in_dir_fd_bounded(
        state_fd,
        UNCOMMITTED_PARTITIONS_FILE,
        &entries,
        MAX_UNCOMMITTED_PARTITIONS_BYTES,
    )
}

fn merge_uncommitted_plan_unlocked(
    layout: &StorageLayout,
    plan: &OlapFlushPlan,
) -> Result<(), String> {
    if plan.uncommitted_partitions().is_empty() && plan.planned_files.is_empty() {
        return Ok(());
    }
    let (state_dir, state_fd) = open_state_dir_fd_for_write(layout)?;
    let mut map = read_uncommitted_refs_on_fd(&state_fd, &state_dir)?;
    for &(domain, day) in plan.uncommitted_partitions() {
        let domain = validate_olap_domain(domain)?;
        map.entry((domain.to_owned(), day)).or_default();
    }
    for (domain, day, file_name) in &plan.planned_files {
        let domain = validate_olap_domain(domain)?;
        validate_parquet_filename(file_name, "planned parquet")?;
        map.entry((domain.to_owned(), *day))
            .or_default()
            .insert(PersistedUncommittedFile {
                name: file_name.clone(),
                watermark: plan.spindle_watermark,
            });
    }
    write_uncommitted_refs_on_fd(&state_fd, &map)
}

/// Atomically merges the active writer's uncommitted `(domain, day)` partition protection set
/// under the maintenance lock while preserving any tracked uncommitted Parquet filenames.
pub fn merge_uncommitted_partitions(
    layout: &StorageLayout,
    partitions: &BTreeSet<(&str, NaiveDate)>,
) -> Result<(), String> {
    if partitions.is_empty() {
        return Ok(());
    }
    for &(domain, _) in partitions {
        validate_olap_domain(domain)?;
    }
    let _maint = acquire_maintenance_lock(layout)?;
    let (state_dir, state_fd) = open_state_dir_fd_for_write(layout)?;
    let mut map = read_uncommitted_refs_on_fd(&state_fd, &state_dir)?;
    for &(domain, day) in partitions {
        let domain = validate_olap_domain(domain)?;
        map.entry((domain.to_owned(), day)).or_default();
    }
    write_uncommitted_refs_on_fd(&state_fd, &map)
}

/// Removes committed (and watermark-untagged) uncommitted-file markers up to `committed_watermark`,
/// retaining any entries whose watermark exceeds `committed_watermark` or whose `(domain, day)`
/// remains present in `keep_partitions`.
pub fn clear_committed_uncommitted(
    layout: &StorageLayout,
    committed_watermark: Option<u64>,
    keep_partitions: &BTreeSet<(&str, NaiveDate)>,
) -> Result<(), String> {
    for &(domain, _) in keep_partitions {
        validate_olap_domain(domain)?;
    }
    let _maint = acquire_maintenance_lock(layout)?;
    let (state_dir, state_fd) = open_state_dir_fd_for_write(layout)?;
    let mut map = read_uncommitted_refs_on_fd(&state_fd, &state_dir)?;
    for files in map.values_mut() {
        files.retain(|f| match f.watermark {
            None => false,
            Some(fw) => committed_watermark.is_none_or(|cw| fw > cw),
        });
    }
    map.retain(|(domain, day), files| {
        !files.is_empty() || keep_partitions.contains(&(domain.as_str(), *day))
    });
    for &(domain, day) in keep_partitions {
        let domain = validate_olap_domain(domain)?;
        map.entry((domain.to_owned(), day)).or_default();
    }
    write_uncommitted_refs_on_fd(&state_fd, &map)
}

/// Atomically persists or clears the active writer's uncommitted `(domain, day)` partitions
/// under the maintenance lock.
pub fn save_uncommitted_partitions(
    layout: &StorageLayout,
    partitions: &BTreeSet<(&str, NaiveDate)>,
) -> Result<(), String> {
    if partitions.is_empty() {
        clear_committed_uncommitted(layout, None, partitions)
    } else {
        merge_uncommitted_partitions(layout, partitions)
    }
}

fn reclaim_stale_uncommitted_unlocked(
    layout: &StorageLayout,
    _maint: &MaintenanceLockGuard,
) -> Result<(), String> {
    let Some(data_fd) = open_anchor_data_dir_if_exists(layout)? else {
        return Ok(());
    };
    let Some(state_fd) = open_child_dir_nofollow(&data_fd, "state", "state dir")? else {
        return Ok(());
    };
    if !is_regular_file_in_dir_no_symlink(
        &state_fd,
        UNCOMMITTED_PARTITIONS_FILE,
        "uncommitted partitions marker",
    )? {
        return Ok(());
    }
    let map = read_uncommitted_refs_on_fd(&state_fd, &layout.state_dir())?;
    let needs_wm = map.values().flatten().any(|f| f.watermark.is_some());
    let committed_wm = if needs_wm {
        crate::catalog::CatalogStore::read_watermark_for_recovery(layout, "spindle_watermark_id")?
    } else {
        0
    };
    if let Some(olap_fd) = open_child_dir_nofollow(&data_fd, "olap", "olap dir")? {
        for ((domain, day), files) in &map {
            let domain = validate_olap_domain(domain)?;
            let domain_fd = match open_child_dir_nofollow(&olap_fd, domain, "domain dir") {
                Ok(Some(fd)) => fd,
                Ok(None) => continue,
                Err(e) if e.starts_with("non-directory ") => continue,
                Err(e) => return Err(e),
            };
            let part_name = format!("dt={}", day.format("%Y-%m-%d"));
            let part_fd = match open_child_dir_nofollow(&domain_fd, &part_name, "partition dir") {
                Ok(Some(fd)) => fd,
                Ok(None) => continue,
                Err(e) if e.starts_with("non-directory ") => continue,
                Err(e) => return Err(e),
            };
            let mut has_rollback_entry = false;
            for f in files {
                if let Some(req_wm) = f.watermark
                    && committed_wm < req_wm
                {
                    has_rollback_entry = true;
                    validate_parquet_filename(&f.name, "uncommitted parquet")?;
                    unlink_regular_file_in_dir_if_present(
                        &part_fd,
                        &f.name,
                        "uncommitted parquet",
                    )?;
                }
            }
            if has_rollback_entry {
                part_fd
                    .sync_all()
                    .map_err(|e| format!("failed syncing partition {part_name}: {e}"))?;
            }
        }
    }
    unlink_regular_file_in_dir_if_present(
        &state_fd,
        UNCOMMITTED_PARTITIONS_FILE,
        "uncommitted partitions marker",
    )?;
    state_fd
        .sync_all()
        .map_err(|e| format!("failed syncing state dir: {e}"))?;
    Ok(())
}

pub(crate) fn load_live_writer_uncommitted_partitions_unlocked(
    layout: &StorageLayout,
    maint: &MaintenanceLockGuard,
) -> Result<BTreeSet<(String, NaiveDate)>, String> {
    let Some(data_fd) = open_anchor_data_dir_if_exists(layout)? else {
        return Ok(BTreeSet::new());
    };
    let Some(state_fd) = open_child_dir_nofollow(&data_fd, "state", "state dir")? else {
        return Ok(BTreeSet::new());
    };
    if !is_regular_file_in_dir_no_symlink(
        &state_fd,
        UNCOMMITTED_PARTITIONS_FILE,
        "uncommitted partitions marker",
    )? {
        return Ok(BTreeSet::new());
    }
    let state_dir = layout.state_dir();
    if !is_regular_file_in_dir_no_symlink(
        &state_fd,
        ".uncommitted.lock",
        "uncommitted writer lock",
    )? {
        reclaim_stale_uncommitted_unlocked(layout, maint)?;
        return Ok(BTreeSet::new());
    }
    match open_and_lock_state_file(layout, ".uncommitted.lock", LockMode::ExclusiveProbe)? {
        None => Ok(read_uncommitted_refs_on_fd(&state_fd, &state_dir)?
            .into_keys()
            .collect()),
        Some(_guard) => {
            reclaim_stale_uncommitted_unlocked(layout, maint)?;
            Ok(BTreeSet::new())
        }
    }
}

struct BoundedWriter<'a, W: Write> {
    inner: &'a mut W,
    written: u64,
    max_bytes: u64,
}

impl<W: Write> Write for BoundedWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let next = self.written.checked_add(buf.len() as u64).ok_or_else(|| {
            std::io::Error::new(ErrorKind::InvalidInput, "serialized byte count overflow")
        })?;
        if next > self.max_bytes {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "serialized JSON exceeds max size ({next} > {} bytes)",
                    self.max_bytes
                ),
            ));
        }
        let n = self.inner.write(buf)?;
        self.written = self.written.saturating_add(n as u64);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn write_atomic_with_in_dir_fd_seq_pinned<F>(
    dir_fd: &File,
    filename: &str,
    seq: u64,
    writer: F,
) -> Result<(File, FileIdentity), String>
where
    F: FnOnce(&mut File, &str) -> Result<(), String>,
{
    validate_single_normal_filename(filename, "atomic target")?;
    let tmp_name = format!("{filename}.tmp.{}.{seq}", std::process::id());
    let tmp_proc = proc_fd_child_path(dir_fd, &tmp_name, "atomic tmp")?;
    let check_final = || -> Result<(), String> {
        let _ = inspect_regular_file_identity_in_dir_no_symlink(dir_fd, filename, "atomic target")?;
        Ok(())
    };
    check_final()?;
    let mut f = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW_FLAG | O_NONBLOCK_FLAG)
        .open(&tmp_proc)
        .map_err(|e| format!("failed creating {tmp_name}: {e}"))?;
    if let Err(err) = crate::atomic_fd::enforce_private_file_mode(&f, &tmp_name) {
        drop(f);
        let _ = unlink_regular_file_in_dir_if_present(dir_fd, &tmp_name, "atomic tmp");
        return Err(err);
    }
    let tmp_meta = f
        .metadata()
        .map_err(|e| format!("failed inspecting {tmp_name} fd: {e}"))?;
    let base_id = FileIdentity {
        dev: tmp_meta.dev(),
        ino: tmp_meta.ino(),
        len: None,
        sha256: None,
    };
    let cleanup_tmp = |id: &FileIdentity| {
        let _ =
            unlink_regular_file_matching_identity_in_dir(dir_fd, &tmp_name, Some(id), "atomic tmp");
    };
    if let Err(err) = writer(&mut f, &tmp_name) {
        cleanup_tmp(&base_id);
        return Err(err);
    }
    if let Err(err) = f.sync_all() {
        cleanup_tmp(&base_id);
        return Err(format!("failed syncing {tmp_name}: {err}"));
    }
    let (actual_len, actual_sha) = match sha256_regular_file(&f) {
        Ok(res) => res,
        Err(err) => {
            cleanup_tmp(&base_id);
            return Err(err);
        }
    };
    let tmp_id = FileIdentity {
        dev: base_id.dev,
        ino: base_id.ino,
        len: Some(actual_len),
        sha256: Some(actual_sha),
    };
    if let Err(err) = check_final() {
        cleanup_tmp(&tmp_id);
        return Err(err);
    }
    match open_regular_file_in_dir_nofollow(dir_fd, &tmp_name, "atomic tmp") {
        Ok(Some((pre_f, pre_meta))) => {
            match verify_open_file_matches_identity(&pre_f, &pre_meta, &tmp_id) {
                Ok(true) => {}
                Ok(false) => {
                    cleanup_tmp(&tmp_id);
                    return Err(format!(
                        "atomic temp {tmp_name} changed concurrently before publish"
                    ));
                }
                Err(err) => {
                    cleanup_tmp(&tmp_id);
                    return Err(err);
                }
            }
        }
        Ok(None) => {
            cleanup_tmp(&tmp_id);
            return Err(format!("atomic temp {tmp_name} disappeared before publish"));
        }
        Err(err) => {
            cleanup_tmp(&tmp_id);
            return Err(err);
        }
    }
    if let Err(err) = rustix::fs::renameat(dir_fd, &tmp_name, dir_fd, filename) {
        cleanup_tmp(&tmp_id);
        return Err(format!("failed renaming {tmp_name} -> {filename}: {err}"));
    }
    match open_regular_file_in_dir_nofollow(dir_fd, filename, "atomic target")? {
        Some((post_f, post_meta))
            if verify_open_file_matches_identity(&post_f, &post_meta, &tmp_id)? => {}
        _ => {
            return Err(format!(
                "atomic target {filename} identity or digest mismatch after publish"
            ));
        }
    }
    dir_fd
        .sync_all()
        .map_err(|e| format!("failed syncing directory for {filename}: {e}"))?;
    Ok((f, tmp_id))
}

pub(crate) fn write_atomic_with_in_dir_fd_pinned<F>(
    dir_fd: &File,
    filename: &str,
    writer: F,
) -> Result<(File, FileIdentity), String>
where
    F: FnOnce(&mut File, &str) -> Result<(), String>,
{
    let seq = PART_SEQ.fetch_add(1, Ordering::Relaxed);
    write_atomic_with_in_dir_fd_seq_pinned(dir_fd, filename, seq, writer)
}

fn write_atomic_with_in_dir_fd<F>(dir_fd: &File, filename: &str, writer: F) -> Result<(), String>
where
    F: FnOnce(&mut File, &str) -> Result<(), String>,
{
    write_atomic_with_in_dir_fd_pinned(dir_fd, filename, writer).map(|_| ())
}

pub(crate) fn write_atomic_file_in_dir_fd(
    dir_fd: &File,
    filename: &str,
    bytes: &[u8],
) -> Result<(), String> {
    write_atomic_with_in_dir_fd(dir_fd, filename, |f, tmp_name| {
        f.write_all(bytes)
            .map_err(|e| format!("failed writing {tmp_name}: {e}"))
    })
}

pub(crate) fn write_atomic_json_in_dir_fd_bounded_pinned<T: Serialize + ?Sized>(
    dir_fd: &File,
    filename: &str,
    value: &T,
    max_bytes: u64,
) -> Result<(File, FileIdentity), String> {
    write_atomic_with_in_dir_fd_pinned(dir_fd, filename, |f, tmp_name| {
        let mut bw = BufWriter::new(BoundedWriter {
            inner: f,
            written: 0,
            max_bytes,
        });
        serde_json::to_writer(&mut bw, value)
            .map_err(|e| format!("failed serializing {tmp_name}: {e}"))?;
        bw.flush()
            .map_err(|e| format!("failed flushing {tmp_name}: {e}"))
    })
}

pub(crate) fn write_atomic_json_in_dir_fd_bounded<T: Serialize + ?Sized>(
    dir_fd: &File,
    filename: &str,
    value: &T,
    max_bytes: u64,
) -> Result<(), String> {
    write_atomic_json_in_dir_fd_bounded_pinned(dir_fd, filename, value, max_bytes).map(|_| ())
}

#[cfg(test)]
pub(crate) fn write_atomic_file_in_dir(
    dir: &Path,
    filename: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let dir_fd = open_standalone_dir_nofollow(dir, "target dir")?
        .ok_or_else(|| format!("missing target dir {}", dir.display()))?;
    write_atomic_file_in_dir_fd(&dir_fd, filename, bytes)
}

#[cfg(test)]
pub(crate) fn write_atomic_json_in_dir_bounded<T: Serialize + ?Sized>(
    dir: &Path,
    filename: &str,
    value: &T,
    max_bytes: u64,
) -> Result<(), String> {
    let dir_fd = open_standalone_dir_nofollow(dir, "target dir")?
        .ok_or_else(|| format!("missing target dir {}", dir.display()))?;
    write_atomic_json_in_dir_fd_bounded(&dir_fd, filename, value, max_bytes)
}

pub(crate) fn read_non_symlink_bytes_in_dir_bounded_if_present(
    dir_fd: &File,
    filename: &str,
    label: &str,
    max_bytes: u64,
) -> Result<Option<Vec<u8>>, String> {
    let Some((file, fd_meta)) = open_regular_file_in_dir_nofollow(dir_fd, filename, label)? else {
        return Ok(None);
    };
    if fd_meta.len() > max_bytes {
        return Err(format!(
            "{label} {filename} size {} exceeds {max_bytes} bytes",
            fd_meta.len()
        ));
    }
    let pre_cap = usize::try_from(fd_meta.len().min(max_bytes))
        .unwrap_or(0)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(pre_cap);
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| format!("failed reading {label} {filename}: {e}"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "{label} {filename} read size {} exceeds {max_bytes} bytes",
            bytes.len()
        ));
    }
    Ok(Some(bytes))
}

pub(crate) fn read_non_symlink_bytes_bounded_if_present(
    path: &Path,
    label: &str,
    max_bytes: u64,
) -> Result<Option<Vec<u8>>, String> {
    let Some(parent) = path.parent() else {
        return Err(format!(
            "missing parent directory for {label} {}",
            path.display()
        ));
    };
    let Some(filename) = path.file_name().and_then(|s| s.to_str()) else {
        return Err(format!("invalid {label} filename {}", path.display()));
    };
    let Some(dir_fd) = open_standalone_dir_nofollow(parent, "parent dir")? else {
        return Ok(None);
    };
    read_non_symlink_bytes_in_dir_bounded_if_present(&dir_fd, filename, label, max_bytes)
}

fn load_replace_intent_on_fd(
    part_fd: &File,
    display_part_dir: &Path,
) -> Result<Option<ReplaceIntent>, String> {
    Ok(
        load_replace_intent_on_fd_measured(part_fd, display_part_dir, usize::MAX)?
            .map(|(intent, _)| intent),
    )
}

fn load_replace_intent_on_fd_measured(
    part_fd: &File,
    display_part_dir: &Path,
    available_budget: usize,
) -> Result<Option<(ReplaceIntent, usize)>, String> {
    let Some((file, fd_meta)) =
        open_regular_file_in_dir_nofollow(part_fd, REPLACE_INTENT_FILE, "replace intent")?
    else {
        return Ok(None);
    };
    if fd_meta.len() > MAX_REPLACE_INTENT_BYTES {
        return Err(format!(
            "replace intent {REPLACE_INTENT_FILE} size {} exceeds {MAX_REPLACE_INTENT_BYTES} bytes",
            fd_meta.len()
        ));
    }
    let file_bytes = usize::try_from(fd_meta.len())
        .map_err(|_| format!("replace intent {REPLACE_INTENT_FILE} size overflows usize"))?;
    let pre_cap = file_bytes.saturating_add(1);
    let worst_parse_peak = pre_cap
        .checked_mul(3)
        .and_then(|raw| raw.checked_add(MAX_REPLACE_INTENT_DECODED_BYTES));
    if worst_parse_peak.is_none_or(|need| need > available_budget) {
        return Err(format!(
            "Resources exhausted: replace intent parse budget exceeds available {available_budget} bytes in {}",
            display_part_dir.display()
        ));
    }
    let mut bytes = Vec::with_capacity(pre_cap);
    file.take(u64::try_from(pre_cap).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|e| format!("failed reading replace intent {REPLACE_INTENT_FILE}: {e}"))?;
    if bytes.len() != file_bytes {
        return Err(format!(
            "replace intent {REPLACE_INTENT_FILE} changed concurrently during open (expected {file_bytes} bytes, got {})",
            bytes.len()
        ));
    }
    if bytes.len() as u64 > MAX_REPLACE_INTENT_BYTES {
        return Err(format!(
            "replace intent {REPLACE_INTENT_FILE} read size {} exceeds {MAX_REPLACE_INTENT_BYTES} bytes",
            bytes.len()
        ));
    }
    let raw_cap = bytes.capacity();
    if raw_cap
        .saturating_mul(3)
        .saturating_add(MAX_REPLACE_INTENT_DECODED_BYTES)
        > available_budget
    {
        return Err(format!(
            "Resources exhausted: replace intent buffer {raw_cap} + decoded peak exceeds available {available_budget} bytes in {}",
            display_part_dir.display()
        ));
    }
    let intent_display = display_part_dir.join(REPLACE_INTENT_FILE);
    let intent: ReplaceIntent = serde_json::from_slice(&bytes)
        .map_err(|e| format!("corrupt {}: {e}", intent_display.display()))?;
    drop(bytes);
    validate_parquet_filename(&intent.replacement, "replace intent replacement")?;
    for input in &intent.inputs {
        validate_replace_input_filename(input, "replace intent input")?;
    }
    for input in intent.input_ids.keys() {
        validate_replace_input_filename(input, "replace intent input")?;
    }
    if let Some(ref staged_src) = intent.staged_source {
        validate_staged_source_filename(staged_src, "replace intent staged source")?;
    }
    let retained = intent.retained_heap_bytes();
    let parse_peak = raw_cap.saturating_mul(3).saturating_add(retained);
    if parse_peak > available_budget {
        return Err(format!(
            "Resources exhausted: replace intent parse memory exceeds available {available_budget} bytes in {}",
            display_part_dir.display()
        ));
    }
    Ok(Some((intent, parse_peak)))
}

fn load_replace_epoch_on_fd(
    part_fd: &File,
    display_part_dir: &Path,
) -> Result<Option<u64>, String> {
    let Some(bytes) = read_non_symlink_bytes_in_dir_bounded_if_present(
        part_fd,
        REPLACE_EPOCH_FILE,
        "replace epoch",
        MAX_REPLACE_EPOCH_BYTES,
    )?
    else {
        return Ok(None);
    };
    let epoch_display = display_part_dir.join(REPLACE_EPOCH_FILE);
    let text = std::str::from_utf8(&bytes)
        .map_err(|e| format!("corrupt {}: {e}", epoch_display.display()))?
        .trim();
    text.parse::<u64>()
        .map(Some)
        .map_err(|e| format!("corrupt {}: {e}", epoch_display.display()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReplacedInputRecord {
    pub name: String,
    pub id: FileIdentity,
}

fn load_replaced_inputs_on_fd(
    part_fd: &File,
    display_part_dir: &Path,
) -> Result<Vec<ReplacedInputRecord>, String> {
    let Some(bytes) = read_non_symlink_bytes_in_dir_bounded_if_present(
        part_fd,
        REPLACED_INPUTS_FILE,
        "replaced inputs ledger",
        MAX_REPLACED_INPUTS_BYTES,
    )?
    else {
        return Ok(Vec::new());
    };
    let display_path = display_part_dir.join(REPLACED_INPUTS_FILE);
    let records: Vec<ReplacedInputRecord> = serde_json::from_slice(&bytes)
        .map_err(|e| format!("corrupt {}: {e}", display_path.display()))?;
    for rec in &records {
        validate_parquet_filename(&rec.name, "replaced input ledger entry")?;
    }
    Ok(records)
}

pub(crate) fn record_replaced_parquet_inputs_on_fd(
    part_fd: &File,
    display_part_dir: &Path,
    new_records: &[ReplacedInputRecord],
    _maint: &MaintenanceLockGuard,
) -> Result<(), String> {
    if new_records.is_empty() {
        return Ok(());
    }
    for rec in new_records {
        validate_parquet_filename(&rec.name, "replaced input ledger entry")?;
    }
    let mut records = load_replaced_inputs_on_fd(part_fd, display_part_dir)?;
    let mut changed = false;
    for rec in new_records {
        if !records.iter().any(|existing| existing == rec) {
            records.push(rec.clone());
            changed = true;
        }
    }
    if !changed {
        return Ok(());
    }
    if records.len() > MAX_REPLACED_INPUT_RECORDS {
        let drop_count = records.len() - MAX_REPLACED_INPUT_RECORDS;
        records.drain(..drop_count);
    }
    write_atomic_json_in_dir_fd_bounded(
        part_fd,
        REPLACED_INPUTS_FILE,
        &records,
        MAX_REPLACED_INPUTS_BYTES,
    )
}

fn was_file_durably_replaced_on_fd(
    part_fd: &File,
    display_part_dir: &Path,
    name: &str,
    id: &FileIdentity,
) -> Result<bool, String> {
    let records = load_replaced_inputs_on_fd(part_fd, display_part_dir)?;
    Ok(records.iter().any(|r| r.name == name && &r.id == id))
}

pub(crate) fn advance_replace_epoch_on_fd(
    part_fd: &File,
    display_part_dir: &Path,
    _maint: &MaintenanceLockGuard,
) -> Result<u64, String> {
    let next_epoch = load_replace_epoch_on_fd(part_fd, display_part_dir)?
        .unwrap_or(0)
        .saturating_add(1);
    write_atomic_file_in_dir_fd(
        part_fd,
        REPLACE_EPOCH_FILE,
        next_epoch.to_string().as_bytes(),
    )?;
    Ok(next_epoch)
}

fn uncommitted_intent_superseded_on_fd_with_budget(
    part_fd: &File,
    display_part_dir: &Path,
    intent: &ReplaceIntent,
    available_budget: usize,
) -> Result<bool, String> {
    match open_regular_file_in_dir_nofollow(part_fd, &intent.replacement, "replacement parquet") {
        Ok(Some(_)) => return Ok(true),
        Ok(None) => {}
        Err(e) if is_concurrent_open_change(&e) => return Ok(true),
        Err(e) => return Err(e),
    }
    match load_replace_intent_on_fd_measured(part_fd, display_part_dir, available_budget) {
        Ok(cur) if cur.as_ref().map(|(i, _)| i) != Some(intent) => Ok(true),
        Ok(_) => Ok(false),
        Err(e) if is_concurrent_open_change(&e) => Ok(true),
        Err(e) => Err(e),
    }
}

fn verify_uncommitted_intent_inputs_on_fd(
    part_fd: &File,
    display_part_dir: &Path,
    intent: &ReplaceIntent,
) -> Result<bool, String> {
    verify_uncommitted_intent_inputs_on_fd_with_budget(
        part_fd,
        display_part_dir,
        intent,
        usize::MAX,
    )
}

fn verify_uncommitted_intent_inputs_on_fd_with_budget(
    part_fd: &File,
    display_part_dir: &Path,
    intent: &ReplaceIntent,
    available_budget: usize,
) -> Result<bool, String> {
    if let Some(ref src_name) = intent.staged_source {
        let opened_src = match open_regular_file_in_dir_nofollow(
            part_fd,
            src_name,
            "replace intent staged source",
        ) {
            Ok(res) => res,
            Err(e) if is_concurrent_open_change(&e) => return Ok(false),
            Err(e) => return Err(e),
        };
        let Some((src_f, src_meta)) = opened_src else {
            if uncommitted_intent_superseded_on_fd_with_budget(
                part_fd,
                display_part_dir,
                intent,
                available_budget,
            )? {
                return Ok(false);
            }
            return Err(format!(
                "replace intent replacement {} disappeared after consuming staged source {src_name} in {}",
                intent.replacement,
                display_part_dir.display()
            ));
        };
        if let Some(ref exp_rep) = intent.replacement_id
            && !verify_open_file_matches_identity(&src_f, &src_meta, exp_rep)?
        {
            return Err(format!(
                "uncommitted replace intent staged source {src_name} in {} failed identity/digest verification",
                display_part_dir.display()
            ));
        }
    }
    for input in &intent.inputs {
        if input == &intent.replacement {
            continue;
        }
        let opened = match open_regular_file_in_dir_nofollow(part_fd, input, "replace intent input")
        {
            Ok(res) => res,
            Err(e) if is_concurrent_open_change(&e) => return Ok(false),
            Err(e) => return Err(e),
        };
        let Some((in_f, in_meta)) = opened else {
            if uncommitted_intent_superseded_on_fd_with_budget(
                part_fd,
                display_part_dir,
                intent,
                available_budget,
            )? {
                return Ok(false);
            }
            return Err(format!(
                "uncommitted replace intent input {input} missing from {}",
                display_part_dir.display()
            ));
        };
        if let Some(exp_in) = intent.input_ids.get(input)
            && !verify_open_file_matches_identity(&in_f, &in_meta, exp_in)?
        {
            return Err(format!(
                "uncommitted replace intent input {input} in {} failed identity/digest verification",
                display_part_dir.display()
            ));
        }
    }
    Ok(true)
}

/// Completes or rolls back any interrupted crash-atomic replacement inside a pinned partition FD.
pub(crate) fn reconcile_partition_intent_on_fd(
    part_fd: &File,
    display_part_dir: &Path,
    _maint: &MaintenanceLockGuard,
) -> Result<(), String> {
    let Some(intent) = load_replace_intent_on_fd(part_fd, display_part_dir)? else {
        cleanup_orphan_staged_sources_on_fd(part_fd)?;
        return Ok(());
    };
    if intent.poisoned {
        return Err(format!(
            "poisoned replace intent in {} after post-rename verification failure",
            display_part_dir.display()
        ));
    }
    let committed = match open_regular_file_in_dir_nofollow(
        part_fd,
        &intent.replacement,
        "replacement parquet",
    )? {
        Some((rep_file, rep_meta)) => match intent.replacement_id.as_ref() {
            Some(exp) => {
                if !verify_open_file_matches_identity(&rep_file, &rep_meta, exp)? {
                    return Err(format!(
                        "replace intent replacement {} in {} failed identity/digest verification",
                        intent.replacement,
                        display_part_dir.display()
                    ));
                }
                true
            }
            None => true,
        },
        None => {
            if !verify_uncommitted_intent_inputs_on_fd(part_fd, display_part_dir, &intent)? {
                return Err(format!(
                    "concurrent change verifying uncommitted inputs in {}",
                    display_part_dir.display()
                ));
            }
            false
        }
    };
    if committed {
        // Durability barrier MUST precede recording or deleting superseded input files.
        part_fd
            .sync_all()
            .map_err(|e| format!("failed syncing {}: {e}", display_part_dir.display()))?;
        let replaced_records: Vec<ReplacedInputRecord> = intent
            .inputs
            .iter()
            .filter(|input| {
                input.as_str() != intent.replacement.as_str()
                    && Path::new(input)
                        .extension()
                        .is_some_and(|ext| ext == "parquet")
            })
            .filter_map(|input| {
                intent.input_ids.get(input).map(|id| ReplacedInputRecord {
                    name: input.clone(),
                    id: id.clone(),
                })
            })
            .collect();
        record_replaced_parquet_inputs_on_fd(part_fd, display_part_dir, &replaced_records, _maint)?;
        let mut mismatch_input: Option<String> = None;
        for input in &intent.inputs {
            if input != &intent.replacement {
                let expected = intent.input_ids.get(input);
                match unlink_superseded_file_in_dir(part_fd, input, expected, "superseded parquet")?
                {
                    SupersededUnlinkOutcome::Removed | SupersededUnlinkOutcome::AlreadyAbsent => {}
                    SupersededUnlinkOutcome::IdentityMismatch => {
                        if mismatch_input.is_none() {
                            mismatch_input = Some(input.clone());
                        }
                    }
                }
            }
        }
        part_fd
            .sync_all()
            .map_err(|e| format!("failed syncing {}: {e}", display_part_dir.display()))?;
        if let Some(bad) = mismatch_input {
            return Err(format!(
                "superseded input {bad} in {} failed identity/digest verification during reconciliation",
                display_part_dir.display()
            ));
        }
        let _ = advance_replace_epoch_on_fd(part_fd, display_part_dir, _maint)?;
        unlink_regular_file_in_dir_if_present(part_fd, REPLACE_INTENT_FILE, "replace intent")?;
        if let Some(ref src_name) = intent.staged_source {
            let _ = unlink_superseded_file_in_dir(
                part_fd,
                src_name,
                intent.replacement_id.as_ref(),
                "partition staged source",
            )?;
        }
        part_fd
            .sync_all()
            .map_err(|e| format!("failed syncing {}: {e}", display_part_dir.display()))?;
    } else {
        unlink_regular_file_in_dir_if_present(part_fd, REPLACE_INTENT_FILE, "replace intent")?;
        part_fd
            .sync_all()
            .map_err(|e| format!("failed syncing {}: {e}", display_part_dir.display()))?;
        if let Some(ref src_name) = intent.staged_source {
            let _ = unlink_superseded_file_in_dir(
                part_fd,
                src_name,
                intent.replacement_id.as_ref(),
                "partition staged source",
            )?;
            part_fd
                .sync_all()
                .map_err(|e| format!("failed syncing {}: {e}", display_part_dir.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
fn load_replace_epoch(part_dir: &Path) -> Result<Option<u64>, String> {
    let Some(part_fd) = open_standalone_dir_nofollow(part_dir, "partition dir")? else {
        return Ok(None);
    };
    load_replace_epoch_on_fd(&part_fd, part_dir)
}

/// Completes or rolls back any interrupted crash-atomic replacement in `part_dir`.
#[cfg(test)]
pub(crate) fn reconcile_partition_intent(
    layout: &StorageLayout,
    part_dir: &Path,
) -> Result<(), String> {
    let maint = acquire_maintenance_lock(layout)?;
    let Some(part_fd) = open_standalone_dir_nofollow(part_dir, "partition dir")? else {
        return Ok(());
    };
    reconcile_partition_intent_on_fd(&part_fd, part_dir, &maint)
}

/// Supported partitioned OLAP domains beneath `<data_dir>/olap`.
pub const OLAP_DOMAINS: &[&str] = &["metrics", "logs", "traces", "fs_events", "edges"];

static PART_SEQ: AtomicU64 = AtomicU64::new(0);

pub(crate) fn next_part_seq() -> u64 {
    PART_SEQ.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone)]
struct PublishedBatchState {
    part_id: FileIdentity,
    file_id: FileIdentity,
    epoch_at_publish: u64,
}

#[derive(Debug, Clone)]
struct PlannedPartitionWrite {
    domain: &'static str,
    day: NaiveDate,
    final_name: String,
    batch: RecordBatch,
    published: Option<PublishedBatchState>,
}

enum PublishStageError {
    PreRename(String),
    PostRenameVerify(String),
    PostRenameSync(String),
}

#[cfg(test)]
thread_local! {
    static TEST_ALLOCATION_REFUSAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static TEST_POST_RENAME_PARTITION_SYNC_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

enum StagingOutcome {
    ScratchRefused(String),
    Empty,
    Staged(StagedParquetFile),
    ExceededMaxBytes,
}

pub(crate) enum CompactionOutcome {
    Complete(Option<PathBuf>),
    ScratchRefused(String),
}

enum ReplaceOutcome {
    ScratchRefused(String),
    Empty,
    Published(PathBuf, u64),
    ExceededMaxBytes,
}

struct StagedParquetFile {
    display_part_dir: PathBuf,
    staging_dir_fd: File,
    staging_name: String,
    final_name: String,
    _file: File,
    dev: u64,
    ino: u64,
    len: u64,
    sha256: String,
    published: bool,
    intent_recorded: bool,
    partition_source: Option<(Arc<File>, String)>,
}

impl StagedParquetFile {
    fn file_identity(&self) -> FileIdentity {
        FileIdentity {
            dev: self.dev,
            ino: self.ino,
            len: Some(self.len),
            sha256: Some(self.sha256.clone()),
        }
    }

    fn verify_source_proc_path(&self) -> Result<PathBuf, String> {
        let child_proc =
            proc_fd_child_path(&self.staging_dir_fd, &self.staging_name, "staging parquet")?;
        let meta = fs::symlink_metadata(&child_proc).map_err(|e| {
            format!(
                "failed inspecting staging parquet {}: {e}",
                self.staging_name
            )
        })?;
        if meta.file_type().is_symlink() {
            return Err(format!(
                "refusing symlinked staging parquet {}",
                self.staging_name
            ));
        }
        if !meta.is_file() {
            return Err(format!("non-regular staging parquet {}", self.staging_name));
        }
        if meta.dev() != self.dev || meta.ino() != self.ino || meta.len() != self.len {
            return Err(format!(
                "staging parquet {} changed concurrently before publish",
                self.staging_name
            ));
        }
        Ok(child_proc)
    }

    fn mark_published(&mut self) {
        self.published = true;
    }

    fn cleanup(&self) {
        let exp = FileIdentity {
            dev: self.dev,
            ino: self.ino,
            len: Some(self.len),
            sha256: Some(self.sha256.clone()),
        };
        let (dir_fd, name, label) = match &self.partition_source {
            Some((part_fd, src_name)) => (
                part_fd.as_ref(),
                src_name.as_str(),
                "partition staged source",
            ),
            None => (
                &self.staging_dir_fd,
                self.staging_name.as_str(),
                "staging parquet",
            ),
        };
        if let Ok(removed) =
            unlink_regular_file_matching_identity_in_dir(dir_fd, name, Some(&exp), label)
            && removed
        {
            let _ = dir_fd.sync_all();
        }
    }
}

impl Drop for StagedParquetFile {
    fn drop(&mut self) {
        if !self.published && !self.intent_recorded {
            self.cleanup();
        }
    }
}

/// Crash-tolerant, idempotent per-`(domain, day)` flush plan with sticky Parquet filenames.
#[derive(Debug, Default, Clone)]
pub struct OlapFlushPlan {
    pending: VecDeque<PlannedPartitionWrite>,
    uncommitted: BTreeSet<(&'static str, NaiveDate)>,
    spindle_watermark: Option<u64>,
    planned_files: Vec<(&'static str, NaiveDate, String)>,
    uncommitted_lock: Option<Arc<File>>,
}

impl OlapFlushPlan {
    /// Associates the Spindle custody watermark covered by this flush plan so uncommitted
    /// Parquet files can be rolled back atomically on crash recovery before watermark commit.
    #[must_use]
    pub fn with_spindle_watermark(mut self, watermark: Option<u64>) -> Self {
        self.spindle_watermark = watermark;
        self
    }

    /// Releases any held shared `.uncommitted.lock` once a standalone caller has finished
    /// committing its custody watermark.
    pub fn release_uncommitted_lock(&mut self) {
        self.uncommitted_lock = None;
    }

    /// Returns `true` when every planned partition has persisted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Total row count still queued across pending partitions.
    #[must_use]
    pub fn total_rows(&self) -> usize {
        self.pending.iter().map(|p| p.batch.num_rows()).sum()
    }

    /// Returns the set of `(domain, day)` partitions still pending in this plan.
    #[must_use]
    pub fn pending_partitions(&self) -> BTreeSet<(&'static str, NaiveDate)> {
        self.pending.iter().map(|p| (p.domain, p.day)).collect()
    }

    /// Returns all `(domain, day)` partitions touched by this flush batch until its watermark commits.
    #[must_use]
    pub fn uncommitted_partitions(&self) -> &BTreeSet<(&'static str, NaiveDate)> {
        &self.uncommitted
    }
}

fn next_part_filename() -> Result<String, String> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let seq = next_part_seq();
    Ok(format!(
        "part-{now_ms}-{}-{seq}.parquet",
        std::process::id()
    ))
}

fn group_batches_by_day<T: Clone, F, B>(
    rows: &[T],
    timestamp_fn: F,
    to_batch: B,
) -> Result<Vec<(NaiveDate, RecordBatch)>, String>
where
    F: Fn(&T) -> i64,
    B: Fn(&[T]) -> Result<RecordBatch, String>,
{
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut by_day: std::collections::BTreeMap<NaiveDate, Vec<T>> =
        std::collections::BTreeMap::new();
    for row in rows {
        by_day
            .entry(day_from_ms(timestamp_fn(row)))
            .or_default()
            .push(row.clone());
    }
    let mut out = Vec::with_capacity(by_day.len());
    for (day, group) in by_day {
        out.push((day, to_batch(&group)?));
    }
    Ok(out)
}

fn append_spindle_provenance(
    batch: RecordBatch,
    provenance: StringArray,
) -> Result<RecordBatch, String> {
    let schema = batch.schema();
    let mut fields = schema.fields().to_vec();
    fields.push(Arc::new(Field::new(
        SPINDLE_PROVENANCE_COLUMN,
        DataType::Utf8,
        true,
    )));
    let mut columns = batch.columns().to_vec();
    columns.push(Arc::new(provenance));
    RecordBatch::try_new(
        Arc::new(Schema::new_with_metadata(fields, schema.metadata().clone())),
        columns,
    )
    .map_err(|e| e.to_string())
}

fn spindle_projection_schema(domain: &str) -> Option<&'static SchemaRef> {
    match domain {
        "metrics" => Some(&METRICS_SCHEMA),
        "logs" => Some(&LOGS_SCHEMA),
        "traces" => Some(&TRACES_SCHEMA),
        _ => None,
    }
}

/// Atomic ZSTD Parquet writer & date-partitioned locator.
#[derive(Debug, Clone)]
pub struct OlapParquetStore {
    layout: StorageLayout,
    data_dir_durable: Arc<AtomicBool>,
}

impl OlapParquetStore {
    /// Creates an OLAP Parquet store handle using an existing validated `StorageLayout`.
    #[must_use]
    pub fn new(layout: StorageLayout) -> Self {
        Self {
            layout,
            data_dir_durable: Arc::new(AtomicBool::new(false)),
        }
    }

    fn ensure_store_root_durable(&self) -> Result<(), String> {
        reject_symlink_path(self.layout.data_dir())?;
        if !self.data_dir_durable.load(Ordering::Acquire) {
            ensure_durable_non_tmp_dir(None, self.layout.data_dir())?;
            reject_symlink_path(self.layout.data_dir())?;
            self.data_dir_durable.store(true, Ordering::Release);
        }
        Ok(())
    }

    #[must_use]
    pub fn layout(&self) -> &StorageLayout {
        &self.layout
    }

    fn write_domain_rows<T: Clone>(
        &self,
        domain: &str,
        rows: &[T],
        timestamp_fn: impl Fn(&T) -> i64,
        to_batch: impl Fn(&[T]) -> Result<RecordBatch, String>,
    ) -> Result<Vec<PathBuf>, String> {
        let grouped = group_batches_by_day(rows, timestamp_fn, to_batch)?;
        let mut written = Vec::with_capacity(grouped.len());
        for (day, batch) in grouped {
            written.push(self.write_batch_to_partition(domain, day, &batch)?);
        }
        Ok(written)
    }

    fn push_planned_domain<T: Clone>(
        pending: &mut VecDeque<PlannedPartitionWrite>,
        domain: &'static str,
        rows: &[T],
        timestamp_fn: impl Fn(&T) -> i64,
        to_batch: impl Fn(&[T]) -> Result<RecordBatch, String>,
    ) -> Result<(), String> {
        for (day, batch) in group_batches_by_day(rows, timestamp_fn, to_batch)? {
            pending.push_back(PlannedPartitionWrite {
                domain,
                day,
                final_name: next_part_filename()?,
                batch,
                published: None,
            });
        }
        Ok(())
    }

    fn push_planned_spindle_domain<T: Clone>(
        pending: &mut VecDeque<PlannedPartitionWrite>,
        domain: &'static str,
        rows: &[(Option<SpindleProvenance>, T)],
        timestamp_fn: impl Fn(&T) -> i64,
        to_batch: impl Fn(&[T]) -> Result<RecordBatch, String>,
    ) -> Result<(), String> {
        let rows: Vec<_> = rows.iter().map(|(key, row)| (key.as_ref(), row)).collect();
        Self::push_planned_domain(
            pending,
            domain,
            &rows,
            |(_, row)| timestamp_fn(row),
            |group| {
                let rows: Vec<_> = group.iter().map(|(_, row)| (*row).clone()).collect();
                let batch = to_batch(&rows)?;
                if group.iter().all(|(key, _)| key.is_none()) {
                    return Ok(batch);
                }
                let keys = group
                    .iter()
                    .map(|(key, _)| key.map(serde_json::to_string).transpose())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                append_spindle_provenance(batch, StringArray::from(keys))
            },
        )
    }

    /// Writes `MetricPoint` rows grouped by UTC date (`dt=YYYY-MM-DD`) in ZSTD Parquet.
    pub fn write_metrics(&self, rows: &[MetricPoint]) -> Result<Vec<PathBuf>, String> {
        self.write_domain_rows("metrics", rows, |r| r.timestamp_ms, metrics_to_record_batch)
    }

    /// Writes `LogRecord` rows grouped by UTC date in ZSTD Parquet.
    pub fn write_logs(&self, rows: &[LogRecord]) -> Result<Vec<PathBuf>, String> {
        self.write_domain_rows("logs", rows, |r| r.timestamp_ms, logs_to_record_batch)
    }

    /// Writes `TraceSpanRecord` rows grouped by UTC date in ZSTD Parquet.
    pub fn write_traces(&self, rows: &[TraceSpanRecord]) -> Result<Vec<PathBuf>, String> {
        self.write_domain_rows("traces", rows, |r| r.timestamp_ms, traces_to_record_batch)
    }

    /// Writes `FsEventRecord` rows grouped by UTC date in ZSTD Parquet.
    pub fn write_fs_events(&self, rows: &[FsEventRecord]) -> Result<Vec<PathBuf>, String> {
        self.write_domain_rows(
            "fs_events",
            rows,
            |r| r.timestamp_ms,
            fs_events_to_record_batch,
        )
    }

    /// Writes `GraphEdgeRecord` rows grouped by UTC date in ZSTD Parquet.
    pub fn write_edges(&self, rows: &[GraphEdgeRecord]) -> Result<Vec<PathBuf>, String> {
        self.write_domain_rows(
            "edges",
            rows,
            |r| r.timestamp_ms,
            graph_edges_to_record_batch,
        )
    }

    /// Groups multi-domain telemetry by `(domain, day)` with sticky filenames for idempotent retry.
    pub fn plan_flush(
        &self,
        metrics: &[MetricPoint],
        logs: &[LogRecord],
        traces: &[TraceSpanRecord],
        fs_events: &[FsEventRecord],
        edges: &[GraphEdgeRecord],
    ) -> Result<OlapFlushPlan, String> {
        let mut pending = VecDeque::new();
        Self::push_planned_domain(
            &mut pending,
            "metrics",
            metrics,
            |r| r.timestamp_ms,
            metrics_to_record_batch,
        )?;
        Self::push_planned_domain(
            &mut pending,
            "logs",
            logs,
            |r| r.timestamp_ms,
            logs_to_record_batch,
        )?;
        Self::push_planned_domain(
            &mut pending,
            "traces",
            traces,
            |r| r.timestamp_ms,
            traces_to_record_batch,
        )?;
        Self::push_planned_domain(
            &mut pending,
            "fs_events",
            fs_events,
            |r| r.timestamp_ms,
            fs_events_to_record_batch,
        )?;
        Self::push_planned_domain(
            &mut pending,
            "edges",
            edges,
            |r| r.timestamp_ms,
            graph_edges_to_record_batch,
        )?;
        Ok(Self::finish_flush_plan(pending))
    }

    /// Plans a Spindle-only raw flush with provenance kept outside user data columns.
    pub fn plan_spindle_flush(
        &self,
        metrics: &[(Option<SpindleProvenance>, MetricPoint)],
        logs: &[(Option<SpindleProvenance>, LogRecord)],
        traces: &[(Option<SpindleProvenance>, TraceSpanRecord)],
    ) -> Result<OlapFlushPlan, String> {
        let mut pending = VecDeque::new();
        Self::push_planned_spindle_domain(
            &mut pending,
            "metrics",
            metrics,
            |r| r.timestamp_ms,
            metrics_to_record_batch,
        )?;
        Self::push_planned_spindle_domain(
            &mut pending,
            "logs",
            logs,
            |r| r.timestamp_ms,
            logs_to_record_batch,
        )?;
        Self::push_planned_spindle_domain(
            &mut pending,
            "traces",
            traces,
            |r| r.timestamp_ms,
            traces_to_record_batch,
        )?;
        Ok(Self::finish_flush_plan(pending))
    }

    fn finish_flush_plan(pending: VecDeque<PlannedPartitionWrite>) -> OlapFlushPlan {
        let uncommitted = pending.iter().map(|p| (p.domain, p.day)).collect();
        let planned_files = pending
            .iter()
            .map(|p| (p.domain, p.day, p.final_name.clone()))
            .collect();
        OlapFlushPlan {
            pending,
            uncommitted,
            spindle_watermark: None,
            planned_files,
            uncommitted_lock: None,
        }
    }

    /// Executes pending `(domain, day)` writes, popping each entry only after durable publish.
    pub fn execute_flush_plan(&self, plan: &mut OlapFlushPlan) -> Result<(), String> {
        if plan.pending.is_empty() {
            if plan.spindle_watermark.is_none() {
                plan.uncommitted_lock = None;
            }
            return Ok(());
        }
        if plan.uncommitted_lock.is_none() {
            let _state_dir = ensure_durable_layout_subdir(&self.layout, &self.layout.state_dir())?;
            let lock_file =
                open_and_lock_state_file(&self.layout, ".uncommitted.lock", LockMode::SharedWait)?
                    .ok_or_else(|| "failed acquiring uncommitted writer lock".to_owned())?;
            plan.uncommitted_lock = Some(Arc::new(lock_file));
        }
        let maint_guard = acquire_maintenance_lock(&self.layout)?;
        merge_uncommitted_plan_unlocked(&self.layout, plan)?;
        while let Some(item) = plan.pending.front_mut() {
            let _ = self.write_planned_partition(item, &maint_guard)?;
            let _ = plan.pending.pop_front();
        }
        if plan.spindle_watermark.is_none() {
            plan.uncommitted_lock = None;
        }
        Ok(())
    }

    fn has_live_regular_parquet_on_fd(part_fd: &File) -> Result<bool, String> {
        for entry in fs::read_dir(proc_fd_dir_path(part_fd)).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let fname = entry.file_name();
            let Some(name) = fname.to_str() else {
                continue;
            };
            if Path::new(name)
                .extension()
                .is_some_and(|ext| ext == "parquet")
                && validate_parquet_filename(name, "parquet file").is_ok()
                && inspect_regular_file_identity_in_dir_no_symlink(part_fd, name, "parquet file")?
                    .is_some()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn verify_parquet_file_matches_batch(
        file: &File,
        expected: &RecordBatch,
    ) -> Result<bool, String> {
        let mut reader_file = file
            .try_clone()
            .map_err(|e| format!("failed cloning parquet fd for verification: {e}"))?;
        reader_file
            .seek(SeekFrom::Start(0))
            .map_err(|e| format!("failed seeking parquet fd for verification: {e}"))?;
        let reader = match open_verified_parquet_batch_reader_from_file(
            reader_file,
            Some(&expected.schema()),
            4096,
        ) {
            Ok(r) => r,
            Err(_) => return Ok(false),
        };
        if reader.schema().fields() != expected.schema().fields() {
            return Ok(false);
        }
        let mut offset = 0usize;
        for batch_res in reader {
            let batch = batch_res.map_err(|e| format!("failed reading parquet batch: {e}"))?;
            let n = batch.num_rows();
            if n == 0 {
                continue;
            }
            if offset.saturating_add(n) > expected.num_rows() {
                return Ok(false);
            }
            let expected_slice = expected.slice(offset, n);
            if batch.columns() != expected_slice.columns() {
                return Ok(false);
            }
            offset += n;
        }
        Ok(offset == expected.num_rows())
    }

    fn publish_staging_to_partition_on_fd_with_hooks<F>(
        part_fd: &File,
        staged: &mut StagedParquetFile,
        mut staging_sync: F,
    ) -> Result<PathBuf, PublishStageError>
    where
        F: FnMut(&File) -> Result<(), String>,
    {
        validate_parquet_filename(&staged.final_name, "parquet file")
            .map_err(PublishStageError::PreRename)?;
        let final_proc = proc_fd_child_path(part_fd, &staged.final_name, "parquet file")
            .map_err(PublishStageError::PreRename)?;
        match inspect_regular_file_identity_in_dir_no_symlink(
            part_fd,
            &staged.final_name,
            "parquet file",
        ) {
            Ok(Some(_)) => {
                return Err(PublishStageError::PreRename(format!(
                    "target parquet {} already exists before cutover",
                    staged.display_part_dir.join(&staged.final_name).display()
                )));
            }
            Ok(None) => {}
            Err(err) => return Err(PublishStageError::PreRename(err)),
        }
        let staging_proc = staged
            .verify_source_proc_path()
            .map_err(PublishStageError::PreRename)?;
        fs::rename(&staging_proc, &final_proc)
            .map_err(|e| PublishStageError::PreRename(format!("parquet rename: {e}")))?;
        staged.mark_published();
        let expected_id = staged.file_identity();
        match open_regular_file_in_dir_nofollow(part_fd, &staged.final_name, "published parquet") {
            Ok(Some((pub_f, pub_meta))) => {
                let matched = verify_open_file_matches_identity(&pub_f, &pub_meta, &expected_id)
                    .map_err(PublishStageError::PostRenameVerify)?;
                if !matched {
                    return Err(PublishStageError::PostRenameVerify(format!(
                        "published parquet {} identity mismatch after cutover",
                        staged.display_part_dir.join(&staged.final_name).display()
                    )));
                }
            }
            Ok(None) => {
                return Err(PublishStageError::PostRenameVerify(format!(
                    "published parquet {} disappeared after cutover",
                    staged.display_part_dir.join(&staged.final_name).display()
                )));
            }
            Err(err) => return Err(PublishStageError::PostRenameVerify(err)),
        }
        part_fd.sync_all().map_err(|e| {
            PublishStageError::PostRenameSync(format!(
                "failed syncing {}: {e}",
                staged.display_part_dir.display()
            ))
        })?;
        staging_sync(&staged.staging_dir_fd).map_err(PublishStageError::PostRenameSync)?;
        Ok(staged.display_part_dir.join(&staged.final_name))
    }

    fn publish_staging_to_partition_on_fd(
        part_fd: &File,
        staged: &mut StagedParquetFile,
    ) -> Result<PathBuf, PublishStageError> {
        Self::publish_staging_to_partition_on_fd_with_hooks(part_fd, staged, |dir_fd| {
            dir_fd
                .sync_all()
                .map_err(|e| format!("failed syncing staging dir: {e}"))
        })
    }

    fn persist_replace_intent_preserving_witness_on_fd<F>(
        part_fd: &File,
        display_part_dir: &Path,
        staged: &mut StagedParquetFile,
        intent: &ReplaceIntent,
        write_intent: F,
    ) -> Result<(), String>
    where
        F: FnOnce(&File, &ReplaceIntent) -> Result<(), String>,
    {
        // Claim durable witness ownership BEFORE attempting to publish `.replace-intent.json`
        // so an error after `fs::rename` (e.g. `dir_fd.sync_all()`) never drops the witness
        // while `.replace-intent.json` is already live on disk.
        staged.intent_recorded = true;
        if let Err(err) = write_intent(part_fd, intent) {
            if matches!(
                load_replace_intent_on_fd(part_fd, display_part_dir),
                Ok(None)
            ) {
                staged.intent_recorded = false;
            }
            return Err(err);
        }
        Ok(())
    }

    fn stage_into_partition_source_on_fd(
        part_fd: &Arc<File>,
        staged: &mut StagedParquetFile,
        source_name: &str,
    ) -> Result<(), String> {
        validate_staged_source_filename(source_name, "partition staged source")?;
        if inspect_regular_file_identity_in_dir_no_symlink(
            part_fd,
            source_name,
            "partition staged source",
        )?
        .is_some()
        {
            return Err(format!(
                "partition staged source {} already exists before publish",
                staged.display_part_dir.join(source_name).display()
            ));
        }
        let staging_proc = staged.verify_source_proc_path()?;
        let source_proc = proc_fd_child_path(part_fd, source_name, "partition staged source")?;
        fs::rename(&staging_proc, &source_proc)
            .map_err(|e| format!("partition source rename: {e}"))?;
        staged.partition_source = Some((Arc::clone(part_fd), source_name.to_owned()));
        let expected_id = staged.file_identity();
        match open_regular_file_in_dir_nofollow(part_fd, source_name, "partition staged source")? {
            Some((src_f, src_meta)) => {
                if !verify_open_file_matches_identity(&src_f, &src_meta, &expected_id)? {
                    return Err(format!(
                        "partition staged source {} identity mismatch after staging",
                        staged.display_part_dir.join(source_name).display()
                    ));
                }
            }
            None => {
                return Err(format!(
                    "partition staged source {} disappeared after staging",
                    staged.display_part_dir.join(source_name).display()
                ));
            }
        }
        part_fd
            .sync_all()
            .map_err(|e| format!("failed syncing {}: {e}", staged.display_part_dir.display()))?;
        staged
            .staging_dir_fd
            .sync_all()
            .map_err(|e| format!("failed syncing staging dir: {e}"))?;
        Ok(())
    }

    fn publish_partition_staged_source_on_fd(
        part_fd: &File,
        staged: &mut StagedParquetFile,
        source_name: &str,
    ) -> Result<PathBuf, PublishStageError> {
        validate_parquet_filename(&staged.final_name, "parquet file")
            .map_err(PublishStageError::PreRename)?;
        validate_staged_source_filename(source_name, "partition staged source")
            .map_err(PublishStageError::PreRename)?;
        let source_proc = proc_fd_child_path(part_fd, source_name, "partition staged source")
            .map_err(PublishStageError::PreRename)?;
        let final_proc = proc_fd_child_path(part_fd, &staged.final_name, "parquet file")
            .map_err(PublishStageError::PreRename)?;
        match inspect_regular_file_identity_in_dir_no_symlink(
            part_fd,
            &staged.final_name,
            "parquet file",
        ) {
            Ok(Some(_)) => {
                return Err(PublishStageError::PreRename(format!(
                    "target parquet {} already exists before cutover",
                    staged.display_part_dir.join(&staged.final_name).display()
                )));
            }
            Ok(None) => {}
            Err(err) => return Err(PublishStageError::PreRename(err)),
        }
        let expected_id = staged.file_identity();
        match open_regular_file_in_dir_nofollow(part_fd, source_name, "partition staged source") {
            Ok(Some((src_f, src_meta))) => {
                let matched = verify_open_file_matches_identity(&src_f, &src_meta, &expected_id)
                    .map_err(PublishStageError::PreRename)?;
                if !matched {
                    return Err(PublishStageError::PreRename(format!(
                        "partition staged source {} changed concurrently before cutover",
                        staged.display_part_dir.join(source_name).display()
                    )));
                }
            }
            Ok(None) => {
                return Err(PublishStageError::PreRename(format!(
                    "partition staged source {} missing before cutover",
                    staged.display_part_dir.join(source_name).display()
                )));
            }
            Err(err) => return Err(PublishStageError::PreRename(err)),
        }
        fs::rename(&source_proc, &final_proc)
            .map_err(|e| PublishStageError::PreRename(format!("parquet rename: {e}")))?;
        staged.mark_published();
        match open_regular_file_in_dir_nofollow(part_fd, &staged.final_name, "published parquet") {
            Ok(Some((pub_f, pub_meta))) => {
                let matched = verify_open_file_matches_identity(&pub_f, &pub_meta, &expected_id)
                    .map_err(PublishStageError::PostRenameVerify)?;
                if !matched {
                    return Err(PublishStageError::PostRenameVerify(format!(
                        "published parquet {} identity mismatch after cutover",
                        staged.display_part_dir.join(&staged.final_name).display()
                    )));
                }
            }
            Ok(None) => {
                return Err(PublishStageError::PostRenameVerify(format!(
                    "published parquet {} disappeared after cutover",
                    staged.display_part_dir.join(&staged.final_name).display()
                )));
            }
            Err(err) => return Err(PublishStageError::PostRenameVerify(err)),
        }
        #[cfg(test)]
        if TEST_POST_RENAME_PARTITION_SYNC_FAILURE.with(|flag| flag.replace(false)) {
            return Err(PublishStageError::PostRenameSync(
                "injected post-rename partition directory sync failure".to_owned(),
            ));
        }
        part_fd.sync_all().map_err(|e| {
            PublishStageError::PostRenameSync(format!(
                "failed syncing {}: {e}",
                staged.display_part_dir.display()
            ))
        })?;
        Ok(staged.display_part_dir.join(&staged.final_name))
    }

    fn write_planned_partition_with_staging_sync<F>(
        &self,
        item: &mut PlannedPartitionWrite,
        _maint: &MaintenanceLockGuard,
        mut staging_sync: F,
    ) -> Result<PathBuf, String>
    where
        F: FnMut(&File) -> Result<(), String>,
    {
        self.ensure_store_root_durable()?;
        let part_dir = self
            .layout
            .olap_domain_dir(item.domain)
            .join(format!("dt={}", item.day.format("%Y-%m-%d")));
        ensure_durable_layout_subdir(&self.layout, &part_dir)?;
        let part_fd_before = open_olap_partition_dir_nofollow(&self.layout, item.domain, item.day)?
            .ok_or_else(|| format!("missing partition dir {}", part_dir.display()))?;
        let before_meta = part_fd_before.metadata().map_err(|e| {
            format!(
                "failed inspecting partition dir {}: {e}",
                part_dir.display()
            )
        })?;
        let before_part_id = FileIdentity {
            dev: before_meta.dev(),
            ino: before_meta.ino(),
            len: None,
            sha256: None,
        };
        let final_path = part_dir.join(&item.final_name);
        if let Some((existing_f, existing_meta)) =
            open_regular_file_in_dir_nofollow(&part_fd_before, &item.final_name, "parquet file")?
        {
            let verified_id = if let Some(ref pub_state) = item.published {
                if !verify_open_file_matches_identity(
                    &existing_f,
                    &existing_meta,
                    &pub_state.file_id,
                )? {
                    return Err(format!(
                        "existing sticky parquet {} does not match recorded identity",
                        final_path.display()
                    ));
                }
                pub_state.file_id.clone()
            } else {
                if !Self::verify_parquet_file_matches_batch(&existing_f, &item.batch)? {
                    return Err(format!(
                        "existing sticky parquet {} does not match planned batch",
                        final_path.display()
                    ));
                }
                let (actual_len, actual_sha) = sha256_regular_file(&existing_f)?;
                FileIdentity {
                    dev: existing_meta.dev(),
                    ino: existing_meta.ino(),
                    len: Some(actual_len),
                    sha256: Some(actual_sha),
                }
            };
            let epoch_now = load_replace_epoch_on_fd(&part_fd_before, &part_dir)?.unwrap_or(0);
            part_fd_before
                .sync_all()
                .map_err(|e| format!("failed syncing {}: {e}", part_dir.display()))?;
            if let Some(staging_fd) = self.open_staging_dir_if_exists()? {
                staging_sync(&staging_fd)?;
            }
            item.published = Some(PublishedBatchState {
                part_id: before_part_id,
                file_id: verified_id,
                epoch_at_publish: epoch_now,
            });
            return Ok(final_path);
        }
        if let Some(ref pub_state) = item.published {
            let current_epoch = load_replace_epoch_on_fd(&part_fd_before, &part_dir)?.unwrap_or(0);
            if before_part_id == pub_state.part_id
                && current_epoch > pub_state.epoch_at_publish
                && Self::has_live_regular_parquet_on_fd(&part_fd_before)?
                && was_file_durably_replaced_on_fd(
                    &part_fd_before,
                    &part_dir,
                    &item.final_name,
                    &pub_state.file_id,
                )?
            {
                part_fd_before
                    .sync_all()
                    .map_err(|e| format!("failed syncing {}: {e}", part_dir.display()))?;
                if let Some(staging_fd) = self.open_staging_dir_if_exists()? {
                    staging_sync(&staging_fd)?;
                }
                return Ok(final_path);
            }
            item.published = None;
        }
        let Some(mut staged) = self.write_batches_to_staging(
            item.domain,
            item.day,
            Some(&item.final_name),
            std::iter::once(Ok(item.batch.clone())),
            true,
        )?
        else {
            return Err("empty planned batch".to_owned());
        };
        let part_fd_after = open_olap_partition_dir_nofollow(&self.layout, item.domain, item.day)?
            .ok_or_else(|| {
                format!(
                    "partition dir {} disappeared during publish",
                    part_dir.display()
                )
            })?;
        let after_meta = part_fd_after.metadata().map_err(|e| {
            format!(
                "failed inspecting partition dir {}: {e}",
                part_dir.display()
            )
        })?;
        if after_meta.dev() != before_meta.dev() || after_meta.ino() != before_meta.ino() {
            return Err(format!(
                "partition dir {} changed concurrently during publish",
                part_dir.display()
            ));
        }
        let after_part_id = FileIdentity {
            dev: after_meta.dev(),
            ino: after_meta.ino(),
            len: None,
            sha256: None,
        };
        let epoch_before = load_replace_epoch_on_fd(&part_fd_after, &part_dir)?.unwrap_or(0);
        let staged_id = staged.file_identity();
        match Self::publish_staging_to_partition_on_fd_with_hooks(
            &part_fd_after,
            &mut staged,
            staging_sync,
        ) {
            Ok(out) => {
                item.published = Some(PublishedBatchState {
                    part_id: after_part_id,
                    file_id: staged_id,
                    epoch_at_publish: epoch_before,
                });
                Ok(out)
            }
            Err(PublishStageError::PostRenameSync(err)) => {
                item.published = Some(PublishedBatchState {
                    part_id: after_part_id,
                    file_id: staged_id,
                    epoch_at_publish: epoch_before,
                });
                Err(err)
            }
            Err(PublishStageError::PreRename(err) | PublishStageError::PostRenameVerify(err)) => {
                Err(err)
            }
        }
    }

    fn open_staging_dir_if_exists(&self) -> Result<Option<File>, String> {
        let Some(data_fd) = open_anchor_data_dir_if_exists(&self.layout)? else {
            return Ok(None);
        };
        match open_child_dir_nofollow(&data_fd, "staging", "staging dir") {
            Ok(fd) => Ok(fd),
            Err(e) if e.starts_with("non-directory ") => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn write_planned_partition(
        &self,
        item: &mut PlannedPartitionWrite,
        maint: &MaintenanceLockGuard,
    ) -> Result<PathBuf, String> {
        self.write_planned_partition_with_staging_sync(item, maint, |dir_fd| {
            dir_fd
                .sync_all()
                .map_err(|e| format!("failed syncing staging dir: {e}"))
        })
    }

    /// Writes one `RecordBatch` atomically to `<data_dir>/olap/<domain>/dt=YYYY-MM-DD/part-*.parquet`.
    pub fn write_batch_to_partition(
        &self,
        domain: &str,
        day: NaiveDate,
        batch: &RecordBatch,
    ) -> Result<PathBuf, String> {
        let domain = validate_olap_domain(domain)?;
        self.write_batches_to_partition(
            domain,
            day,
            None,
            std::iter::once(Ok(batch.clone())),
            true,
        )?
        .ok_or_else(|| "empty parquet batch iterator".to_owned())
    }

    /// Streams record batches into a single ZSTD Parquet file inside `<data_dir>/olap/<domain>/dt=YYYY-MM-DD/`.
    /// Peak memory is bounded by a single `RecordBatch`.
    pub fn compact_parquet_files(
        &self,
        domain: &str,
        day: NaiveDate,
        inputs: &[PathBuf],
    ) -> Result<Option<PathBuf>, String> {
        let domain = validate_olap_domain(domain)?;
        if inputs.len() < 2 {
            return Ok(None);
        }
        let maint_guard = acquire_maintenance_lock(&self.layout)?;
        let live_uncommitted =
            load_live_writer_uncommitted_partitions_unlocked(&self.layout, &maint_guard)?;
        if live_uncommitted.contains(&(domain.to_owned(), day)) {
            return Ok(None);
        }
        self.compact_parquet_files_unlocked(domain, day, inputs, &maint_guard)
    }

    fn validate_partition_compaction_inputs_on_fd(
        part_fd: &Arc<File>,
        display_part_dir: &Path,
        inputs: &[PathBuf],
    ) -> Result<Vec<PinnedParquetFile>, String> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        if inputs.len() > MAX_REPLACE_FAN_IN_FILES {
            return Err(format!(
                "compaction fan-in {} exceeds max {MAX_REPLACE_FAN_IN_FILES}",
                inputs.len()
            ));
        }
        let part_meta = part_fd.metadata().map_err(|e| {
            format!(
                "failed inspecting partition dir {}: {e}",
                display_part_dir.display()
            )
        })?;
        if part_meta.file_type().is_symlink() || !part_meta.is_dir() {
            return Err(format!(
                "invalid partition dir {}",
                display_part_dir.display()
            ));
        }
        let mut seen = BTreeSet::new();
        let mut validated = Vec::with_capacity(inputs.len());
        for input in inputs {
            let Some(name) = input.file_name().and_then(|s| s.to_str()) else {
                return Err(format!("invalid compaction input {}", input.display()));
            };
            validate_parquet_filename(name, "compaction input")?;
            if !seen.insert(name.to_owned()) {
                return Err(format!(
                    "duplicate compaction input {name} in {}",
                    display_part_dir.display()
                ));
            }
            let Some(parent) = input.parent() else {
                return Err(format!(
                    "compaction input {} missing parent directory",
                    input.display()
                ));
            };
            let parent_meta = fs::symlink_metadata(parent).map_err(|e| {
                format!(
                    "failed inspecting compaction input parent {}: {e}",
                    parent.display()
                )
            })?;
            if parent_meta.file_type().is_symlink()
                || !parent_meta.is_dir()
                || parent_meta.dev() != part_meta.dev()
                || parent_meta.ino() != part_meta.ino()
            {
                return Err(format!(
                    "compaction input {} is outside target partition {}",
                    input.display(),
                    display_part_dir.display()
                ));
            }
            let Some((file, child_meta)) =
                open_regular_file_in_dir_nofollow(part_fd, name, "compaction input")?
            else {
                return Err(format!(
                    "missing or invalid compaction input {}",
                    display_part_dir.join(name).display()
                ));
            };
            let (sha_len, sha256) = sha256_regular_file(&file)?;
            if sha_len != child_meta.len() {
                return Err(format!(
                    "compaction input {} changed length while hashing",
                    display_part_dir.join(name).display()
                ));
            }
            validated.push(PinnedParquetFile::new_pinned_in_partition_with_file(
                Arc::clone(part_fd),
                name.to_owned(),
                display_part_dir.join(name),
                child_meta.dev(),
                child_meta.ino(),
                child_meta.len(),
                Some(sha256),
                Arc::new(file),
            ));
        }
        Ok(validated)
    }

    pub(crate) fn verified_file_num_rows(
        file: &File,
        display_path: &Path,
    ) -> Result<usize, String> {
        let (_, _, reader_meta) =
            load_verified_parquet_metadata_from_file(file, None).map_err(|e| {
                format!(
                    "failed reading parquet metadata for {}: {e}",
                    display_path.display()
                )
            })?;
        Self::verified_metadata_num_rows(
            reader_meta.metadata().file_metadata().num_rows(),
            display_path,
        )
    }

    fn verified_metadata_num_rows(raw_rows: i64, display_path: &Path) -> Result<usize, String> {
        if raw_rows < 0 {
            return Err(format!(
                "invalid negative parquet row count {raw_rows} in {}",
                display_path.display()
            ));
        }
        usize::try_from(raw_rows).map_err(|e| {
            format!(
                "parquet row count overflow in {}: {e}",
                display_path.display()
            )
        })
    }

    fn pinned_parquet_compaction_info(
        pinned: &PinnedParquetFile,
        projected_schema: Option<&SchemaRef>,
    ) -> Result<(usize, bool), String> {
        let file = pinned.open_verified()?;
        let (_, _, reader_meta) =
            load_verified_parquet_metadata_from_file(&file, None).map_err(|e| {
                format!(
                    "failed reading parquet metadata for {}: {e}",
                    pinned.display_path().display()
                )
            })?;
        let rows = Self::verified_metadata_num_rows(
            reader_meta.metadata().file_metadata().num_rows(),
            pinned.display_path(),
        )?;
        let has_provenance = if let Some(expected) = projected_schema
            && reader_meta
                .schema()
                .field_with_name(SPINDLE_PROVENANCE_COLUMN)
                .is_ok()
        {
            validate_provenance_compatible_parquet_schema(&reader_meta, expected)?
        } else {
            false
        };
        Ok((rows, has_provenance))
    }

    pub(crate) fn compact_parquet_files_unlocked(
        &self,
        domain: &str,
        day: NaiveDate,
        inputs: &[PathBuf],
        maint: &MaintenanceLockGuard,
    ) -> Result<Option<PathBuf>, String> {
        self.compact_parquet_files_unlocked_with_max_rows(
            domain,
            day,
            inputs,
            MAX_COMPACTED_FILE_ROWS,
            maint,
        )
    }

    pub(crate) fn compact_parquet_files_unlocked_with_max_rows(
        &self,
        domain: &str,
        day: NaiveDate,
        inputs: &[PathBuf],
        max_file_rows: usize,
        maint: &MaintenanceLockGuard,
    ) -> Result<Option<PathBuf>, String> {
        self.compact_parquet_files_unlocked_with_limits(
            domain,
            day,
            inputs,
            max_file_rows,
            MAX_COMPACT_SEALED_FILE_BYTES,
            MAX_COMPACT_GROUP_BYTES,
            MAX_COMPACT_OUTPUT_BYTES,
            maint,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn compact_parquet_files_unlocked_with_limits(
        &self,
        domain: &str,
        day: NaiveDate,
        inputs: &[PathBuf],
        max_file_rows: usize,
        max_sealed_bytes: u64,
        max_group_bytes: u64,
        max_output_bytes: u64,
        maint: &MaintenanceLockGuard,
    ) -> Result<Option<PathBuf>, String> {
        match self.compact_parquet_files_classified(
            domain,
            day,
            inputs,
            max_file_rows,
            max_sealed_bytes,
            max_group_bytes,
            max_output_bytes,
            maint,
        )? {
            CompactionOutcome::Complete(path) => Ok(path),
            CompactionOutcome::ScratchRefused(error) => Err(error),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn compact_parquet_files_classified(
        &self,
        domain: &str,
        day: NaiveDate,
        inputs: &[PathBuf],
        max_file_rows: usize,
        max_sealed_bytes: u64,
        max_group_bytes: u64,
        max_output_bytes: u64,
        maint: &MaintenanceLockGuard,
    ) -> Result<CompactionOutcome, String> {
        let domain = validate_olap_domain(domain)?;
        if inputs.len() < 2 {
            return Ok(CompactionOutcome::Complete(None));
        }
        self.ensure_store_root_durable()?;
        let part_dir = self
            .layout
            .olap_domain_dir(domain)
            .join(format!("dt={}", day.format("%Y-%m-%d")));
        let mut seen_names = BTreeSet::new();
        for input in inputs {
            let Some(name) = input.file_name().and_then(|s| s.to_str()) else {
                return Err(format!("invalid compaction input {}", input.display()));
            };
            validate_parquet_filename(name, "compaction input")?;
            if !seen_names.insert(name.to_owned()) {
                return Err(format!(
                    "duplicate compaction input {name} in {}",
                    part_dir.display()
                ));
            }
        }
        let part_fd = Arc::new(
            open_olap_partition_dir_nofollow(&self.layout, domain, day)?
                .ok_or_else(|| format!("missing partition dir {}", part_dir.display()))?,
        );
        let mut queue: VecDeque<PathBuf> = inputs.iter().cloned().collect();
        let mut last_out = None;
        let mut fan_in_cap = MAX_REPLACE_FAN_IN_FILES;
        let projected_schema = spindle_projection_schema(domain);
        while queue.len() >= 2 {
            reconcile_partition_intent_on_fd(&part_fd, &part_dir, maint)?;
            let peek_count = queue.len().min(fan_in_cap).min(MAX_REPLACE_FAN_IN_FILES);
            let candidates: Vec<PathBuf> = queue.iter().take(peek_count).cloned().collect();
            let validated_candidates =
                Self::validate_partition_compaction_inputs_on_fd(&part_fd, &part_dir, &candidates)?;
            let mut take_count = 0usize;
            let mut chunk_rows = 0usize;
            let mut chunk_bytes = 0u64;
            let mut keep_provenance = false;
            for (idx, pinned) in validated_candidates.iter().enumerate() {
                let (rows, has_provenance) =
                    Self::pinned_parquet_compaction_info(pinned, projected_schema)?;
                if idx == 0 {
                    take_count = 1;
                    chunk_rows = rows;
                    chunk_bytes = pinned.inner.len;
                    keep_provenance = has_provenance;
                    if rows >= max_file_rows || pinned.inner.len >= max_sealed_bytes {
                        break;
                    }
                } else if chunk_rows.saturating_add(rows) <= max_file_rows
                    && chunk_bytes.saturating_add(pinned.inner.len) <= max_group_bytes
                {
                    chunk_rows = chunk_rows.saturating_add(rows);
                    chunk_bytes = chunk_bytes.saturating_add(pinned.inner.len);
                    take_count += 1;
                    keep_provenance |= has_provenance;
                } else {
                    break;
                }
            }
            if take_count < 2 {
                let _sealed = queue.pop_front();
                fan_in_cap = MAX_REPLACE_FAN_IN_FILES;
                continue;
            }
            drop(queue.drain(..take_count));
            let validated_inputs = &validated_candidates[..take_count];
            let batch_iter = StreamingParquetBatches::new(validated_inputs, 4096).map(|batch| {
                let batch = batch?;
                if keep_provenance
                    && batch
                        .schema()
                        .field_with_name(SPINDLE_PROVENANCE_COLUMN)
                        .is_err()
                {
                    let row_count = batch.num_rows();
                    append_spindle_provenance(batch, StringArray::new_null(row_count))
                } else {
                    Ok(batch)
                }
            });
            match self.replace_batches_in_partition_with_validated_capped(
                domain,
                day,
                Some(&part_fd),
                validated_inputs,
                None,
                &[],
                batch_iter,
                Some(max_output_bytes),
                maint,
            )? {
                ReplaceOutcome::Published(out, out_len) => {
                    last_out = Some(out.clone());
                    fan_in_cap = MAX_REPLACE_FAN_IN_FILES;
                    if !queue.is_empty() && chunk_rows < max_file_rows && out_len < max_sealed_bytes
                    {
                        queue.push_back(out);
                    }
                }
                ReplaceOutcome::ExceededMaxBytes => {
                    if take_count > 2 {
                        for p in candidates[..take_count].iter().rev() {
                            queue.push_front(p.clone());
                        }
                        fan_in_cap = take_count - 1;
                    } else {
                        for p in candidates[1..take_count].iter().rev() {
                            queue.push_front(p.clone());
                        }
                        fan_in_cap = MAX_REPLACE_FAN_IN_FILES;
                    }
                }
                ReplaceOutcome::ScratchRefused(error) => {
                    return Ok(CompactionOutcome::ScratchRefused(error));
                }
                ReplaceOutcome::Empty => {
                    fan_in_cap = MAX_REPLACE_FAN_IN_FILES;
                }
            }
        }
        Ok(CompactionOutcome::Complete(last_out))
    }

    pub(crate) fn write_batches_to_partition<I>(
        &self,
        domain: &str,
        day: NaiveDate,
        final_file_name: Option<&str>,
        batches: I,
        write_if_empty: bool,
    ) -> Result<Option<PathBuf>, String>
    where
        I: IntoIterator<Item = Result<RecordBatch, String>>,
    {
        let domain = validate_olap_domain(domain)?;
        let _maint_guard = acquire_maintenance_lock(&self.layout)?;
        let Some(mut staged) =
            self.write_batches_to_staging(domain, day, final_file_name, batches, write_if_empty)?
        else {
            return Ok(None);
        };
        let part_fd =
            open_olap_partition_dir_nofollow(&self.layout, domain, day)?.ok_or_else(|| {
                format!(
                    "partition dir {} disappeared during publish",
                    staged.display_part_dir.display()
                )
            })?;
        Self::publish_staging_to_partition_on_fd(&part_fd, &mut staged)
            .map(Some)
            .map_err(|e| match e {
                PublishStageError::PreRename(msg)
                | PublishStageError::PostRenameVerify(msg)
                | PublishStageError::PostRenameSync(msg) => msg,
            })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn replace_batches_in_pinned_partition<I>(
        &self,
        domain: &str,
        day: NaiveDate,
        part_fd: &Arc<File>,
        validated_inputs: &[PinnedParquetFile],
        final_file_name: Option<&str>,
        extra_remove: &[PinnedAuxiliaryFile],
        batches: I,
        maint: &MaintenanceLockGuard,
    ) -> Result<Option<PathBuf>, String>
    where
        I: IntoIterator<Item = Result<RecordBatch, String>>,
    {
        let domain = validate_olap_domain(domain)?;
        self.ensure_store_root_durable()?;
        let pre_part_dir = self
            .layout
            .olap_domain_dir(domain)
            .join(format!("dt={}", day.format("%Y-%m-%d")));
        reconcile_partition_intent_on_fd(part_fd, &pre_part_dir, maint)?;
        self.replace_batches_in_partition_with_validated(
            domain,
            day,
            Some(part_fd),
            validated_inputs,
            final_file_name,
            extra_remove,
            batches,
            maint,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn replace_batches_in_partition_with_validated<I>(
        &self,
        domain: &str,
        day: NaiveDate,
        pre_part_fd: Option<&Arc<File>>,
        validated_inputs: &[PinnedParquetFile],
        final_file_name: Option<&str>,
        extra_remove: &[PinnedAuxiliaryFile],
        batches: I,
        maint: &MaintenanceLockGuard,
    ) -> Result<Option<PathBuf>, String>
    where
        I: IntoIterator<Item = Result<RecordBatch, String>>,
    {
        match self.replace_batches_in_partition_with_validated_capped(
            domain,
            day,
            pre_part_fd,
            validated_inputs,
            final_file_name,
            extra_remove,
            batches,
            None,
            maint,
        )? {
            ReplaceOutcome::Published(path, _) => Ok(Some(path)),
            ReplaceOutcome::Empty | ReplaceOutcome::ExceededMaxBytes => Ok(None),
            ReplaceOutcome::ScratchRefused(error) => Err(error),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn replace_batches_in_partition_with_validated_capped<I>(
        &self,
        domain: &str,
        day: NaiveDate,
        pre_part_fd: Option<&Arc<File>>,
        validated_inputs: &[PinnedParquetFile],
        final_file_name: Option<&str>,
        extra_remove: &[PinnedAuxiliaryFile],
        batches: I,
        max_staged_bytes: Option<u64>,
        maint: &MaintenanceLockGuard,
    ) -> Result<ReplaceOutcome, String>
    where
        I: IntoIterator<Item = Result<RecordBatch, String>>,
    {
        if validated_inputs.len().saturating_add(extra_remove.len()) > MAX_REPLACE_FAN_IN_FILES {
            return Err(format!(
                "replace fan-in {} exceeds max {MAX_REPLACE_FAN_IN_FILES}",
                validated_inputs.len().saturating_add(extra_remove.len())
            ));
        }
        let pre_meta = match pre_part_fd {
            Some(pre_fd) => Some(pre_fd.metadata().map_err(|e| {
                format!("failed inspecting pre-staging partition dir for {domain}/{day}: {e}")
            })?),
            None => None,
        };
        let mut staged = match self.write_batches_to_staging_bounded(
            domain,
            day,
            final_file_name,
            batches,
            false,
            max_staged_bytes,
        )? {
            StagingOutcome::ScratchRefused(error) => {
                return Ok(ReplaceOutcome::ScratchRefused(error));
            }
            StagingOutcome::Empty => return Ok(ReplaceOutcome::Empty),
            StagingOutcome::ExceededMaxBytes => return Ok(ReplaceOutcome::ExceededMaxBytes),
            StagingOutcome::Staged(s) => s,
        };
        let staged_len = staged.len;
        let part_dir = staged.display_part_dir.clone();
        let final_name = staged.final_name.clone();
        let part_fd =
            open_olap_partition_dir_nofollow(&self.layout, domain, day)?.ok_or_else(|| {
                format!(
                    "partition dir {} disappeared during replacement",
                    part_dir.display()
                )
            })?;
        let part_meta = part_fd.metadata().map_err(|e| {
            format!(
                "failed inspecting partition dir {}: {e}",
                part_dir.display()
            )
        })?;
        if let Some(ref pre) = pre_meta
            && (part_meta.dev() != pre.dev() || part_meta.ino() != pre.ino())
        {
            return Err(format!(
                "partition dir {} changed concurrently during replacement",
                part_dir.display()
            ));
        }
        let mut input_names: Vec<String> =
            Vec::with_capacity(validated_inputs.len().saturating_add(extra_remove.len()));
        let mut input_ids: BTreeMap<String, FileIdentity> = BTreeMap::new();
        let mut seen_names = BTreeSet::new();
        for p in validated_inputs {
            let exp_id = p.file_identity();
            let Some((in_f, in_meta)) = open_regular_file_in_dir_nofollow(
                &part_fd,
                &p.inner.file_name,
                "replacement input",
            )?
            else {
                return Err(format!(
                    "replacement input {} disappeared during cutover",
                    p.inner.display_path.display()
                ));
            };
            if !verify_open_file_matches_identity(&in_f, &in_meta, &exp_id)? {
                return Err(format!(
                    "replacement input {} changed concurrently during cutover",
                    p.inner.display_path.display()
                ));
            }
            if seen_names.insert(p.inner.file_name.to_string()) {
                input_names.push(p.inner.file_name.to_string());
                input_ids.insert(p.inner.file_name.to_string(), exp_id);
            }
        }
        for aux in extra_remove {
            validate_replace_input_filename(&aux.name, "replace intent input")?;
            let exp_id = FileIdentity {
                dev: aux.dev,
                ino: aux.ino,
                len: Some(aux.len),
                sha256: aux.sha256.clone(),
            };
            let Some((aux_f, aux_meta)) =
                open_regular_file_in_dir_nofollow(&part_fd, &aux.name, "replace intent input")?
            else {
                return Err(format!(
                    "replacement auxiliary input {} disappeared during cutover",
                    aux.name
                ));
            };
            if !verify_open_file_matches_identity(&aux_f, &aux_meta, &exp_id)? {
                return Err(format!(
                    "replacement auxiliary input {} changed concurrently during cutover",
                    aux.name
                ));
            }
            if seen_names.insert(aux.name.clone()) {
                input_names.push(aux.name.clone());
                input_ids.insert(aux.name.clone(), exp_id);
            }
        }
        validate_parquet_filename(&final_name, "replace intent replacement")?;
        if inspect_regular_file_identity_in_dir_no_symlink(
            &part_fd,
            &final_name,
            "replace intent replacement",
        )?
        .is_some()
        {
            return Err(format!(
                "replace intent replacement {final_name} already exists before cutover"
            ));
        }
        if seen_names.contains(&final_name) {
            return Err(format!(
                "replace intent replacement {final_name} collides with input"
            ));
        }
        for name in &input_names {
            validate_replace_input_filename(name, "replace intent input")?;
        }
        let staged_source_name = format!(".replace-source-{}.staged", staged.staging_name);
        let part_fd_arc = Arc::new(part_fd);
        Self::stage_into_partition_source_on_fd(&part_fd_arc, &mut staged, &staged_source_name)?;
        let intent = ReplaceIntent {
            replacement: final_name.clone(),
            replacement_id: Some(staged.file_identity()),
            inputs: input_names,
            input_ids,
            poisoned: false,
            staged_source: Some(staged_source_name.clone()),
        };
        Self::persist_replace_intent_preserving_witness_on_fd(
            &part_fd_arc,
            &part_dir,
            &mut staged,
            &intent,
            |fd, val| {
                write_atomic_json_in_dir_fd_bounded(
                    fd,
                    REPLACE_INTENT_FILE,
                    val,
                    MAX_REPLACE_INTENT_BYTES,
                )
                .map(|_| ())
            },
        )?;
        let published = match Self::publish_partition_staged_source_on_fd(
            &part_fd_arc,
            &mut staged,
            &staged_source_name,
        ) {
            Ok(path) => path,
            Err(PublishStageError::PostRenameVerify(err)) => {
                let mut poisoned_intent = intent;
                poisoned_intent.poisoned = true;
                if let Err(poison_err) = write_atomic_json_in_dir_fd_bounded(
                    &part_fd_arc,
                    REPLACE_INTENT_FILE,
                    &poisoned_intent,
                    MAX_REPLACE_INTENT_BYTES,
                ) {
                    return Err(format!(
                        "{err}; additionally failed persisting poisoned replace intent: {poison_err}"
                    ));
                }
                if let Err(sync_err) = part_fd_arc.sync_all() {
                    return Err(format!(
                        "{err}; additionally failed syncing poisoned replace intent: {sync_err}"
                    ));
                }
                return Err(err);
            }
            Err(PublishStageError::PostRenameSync(err)) => {
                // Rename succeeded and post-cutover verification passed; preserve intent so
                // recovery and snapshots complete sync safely instead of dropping lineage.
                return Err(err);
            }
            Err(PublishStageError::PreRename(err)) => {
                if let Err(rec_err) =
                    reconcile_partition_intent_on_fd(&part_fd_arc, &part_dir, maint)
                {
                    return Err(format!(
                        "{err}; rollback reconciliation also failed: {rec_err}"
                    ));
                }
                return Err(err);
            }
        };
        reconcile_partition_intent_on_fd(&part_fd_arc, &part_dir, maint)?;
        Ok(ReplaceOutcome::Published(published, staged_len))
    }

    fn write_batches_to_staging<I>(
        &self,
        domain: &str,
        day: NaiveDate,
        final_file_name: Option<&str>,
        batches: I,
        write_if_empty: bool,
    ) -> Result<Option<StagedParquetFile>, String>
    where
        I: IntoIterator<Item = Result<RecordBatch, String>>,
    {
        match self.write_batches_to_staging_bounded(
            domain,
            day,
            final_file_name,
            batches,
            write_if_empty,
            None,
        )? {
            StagingOutcome::Empty | StagingOutcome::ExceededMaxBytes => Ok(None),
            StagingOutcome::ScratchRefused(error) => Err(error),
            StagingOutcome::Staged(s) => Ok(Some(s)),
        }
    }

    #[cfg(test)]
    pub(crate) fn set_test_allocation_refusal(value: bool) {
        TEST_ALLOCATION_REFUSAL.with(|flag| flag.set(value));
    }

    #[cfg(test)]
    pub(crate) fn arm_test_post_rename_partition_sync_failure() {
        TEST_POST_RENAME_PARTITION_SYNC_FAILURE.with(|flag| flag.set(true));
    }

    #[cfg(test)]
    pub(crate) fn test_post_rename_partition_sync_failure_pending() -> bool {
        TEST_POST_RENAME_PARTITION_SYNC_FAILURE.with(std::cell::Cell::get)
    }

    fn write_batches_to_staging_bounded<I>(
        &self,
        domain: &str,
        day: NaiveDate,
        final_file_name: Option<&str>,
        batches: I,
        write_if_empty: bool,
        max_staged_bytes: Option<u64>,
    ) -> Result<StagingOutcome, String>
    where
        I: IntoIterator<Item = Result<RecordBatch, String>>,
    {
        self.write_batches_to_staging_with_allocator(
            domain,
            day,
            final_file_name,
            batches,
            write_if_empty,
            max_staged_bytes,
            |file, len| {
                #[cfg(test)]
                if TEST_ALLOCATION_REFUSAL.with(std::cell::Cell::get) {
                    return Err(std::io::Error::from_raw_os_error(28));
                }
                rustix::fs::fallocate(file, rustix::fs::FallocateFlags::empty(), 0, len)
                    .map_err(std::io::Error::from)
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn write_batches_to_staging_with_allocator<I, F>(
        &self,
        domain: &str,
        day: NaiveDate,
        final_file_name: Option<&str>,
        batches: I,
        write_if_empty: bool,
        max_staged_bytes: Option<u64>,
        allocate: F,
    ) -> Result<StagingOutcome, String>
    where
        I: IntoIterator<Item = Result<RecordBatch, String>>,
        F: FnOnce(&File, u64) -> std::io::Result<()>,
    {
        let domain = validate_olap_domain(domain)?;
        let mut iter = batches.into_iter();
        let first_batch = loop {
            match iter.next() {
                None => return Ok(StagingOutcome::Empty),
                Some(Err(e)) => return Err(e),
                Some(Ok(b)) => {
                    if b.num_rows() == 0 && !write_if_empty {
                        continue;
                    }
                    break b;
                }
            }
        };

        self.ensure_store_root_durable()?;
        let part_dir = self
            .layout
            .olap_domain_dir(domain)
            .join(format!("dt={}", day.format("%Y-%m-%d")));
        ensure_durable_layout_subdir(&self.layout, &part_dir)?;
        let staging_dir = ensure_durable_layout_subdir(&self.layout, &self.layout.staging_dir())?;
        let data_fd = open_anchor_data_dir(&self.layout)?;
        let staging_dir_fd = open_child_dir_nofollow(&data_fd, "staging", "staging dir")?
            .ok_or_else(|| format!("missing staging dir {}", staging_dir.display()))?;

        let staging_name = next_part_filename()?;
        validate_parquet_filename(&staging_name, "staging parquet")?;
        let staging_proc = proc_fd_child_path(&staging_dir_fd, &staging_name, "staging parquet")?;
        let final_name = final_file_name.unwrap_or(&staging_name).to_owned();
        validate_parquet_filename(&final_name, "parquet file")?;

        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(
                ZstdLevel::try_new(3).map_err(|e| e.to_string())?,
            ))
            .set_max_row_group_row_count(Some(PARQUET_ROW_GROUP_ROWS))
            .set_max_row_group_bytes(Some(MAX_PARQUET_ROW_GROUP_BYTES))
            .set_write_batch_size(4096)
            .build();

        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(O_NOFOLLOW_FLAG | O_NONBLOCK_FLAG)
            .open(&staging_proc)
            .map_err(|e| format!("failed creating staging parquet: {e}"))?;
        if let Err(err) = crate::atomic_fd::enforce_private_file_mode(&file, &staging_name) {
            let _ = unlink_regular_file_in_dir_if_present(
                &staging_dir_fd,
                &staging_name,
                "staging parquet",
            );
            let _ = staging_dir_fd.sync_all();
            drop(file);
            return Err(err);
        }
        let st_meta = match file.metadata() {
            Ok(meta) => meta,
            Err(e) => {
                let _ = unlink_regular_file_in_dir_if_present(
                    &staging_dir_fd,
                    &staging_name,
                    "staging parquet",
                );
                let _ = staging_dir_fd.sync_all();
                drop(file);
                return Err(format!("failed inspecting staging parquet fd: {e}"));
            }
        };
        let dev = st_meta.dev();
        let ino = st_meta.ino();
        let base_id = FileIdentity {
            dev,
            ino,
            len: None,
            sha256: None,
        };
        let writer_file = match file.try_clone() {
            Ok(f) => f,
            Err(e) => {
                let _ = unlink_regular_file_matching_identity_in_dir(
                    &staging_dir_fd,
                    &staging_name,
                    Some(&base_id),
                    "staging parquet",
                );
                let _ = staging_dir_fd.sync_all();
                drop(file);
                return Err(format!("failed cloning staging parquet fd: {e}"));
            }
        };
        if let Some(limit) = max_staged_bytes.filter(|limit| *limit != 0)
            && let Err(err) = allocate(&file, limit)
        {
            let _ = unlink_regular_file_matching_identity_in_dir(
                &staging_dir_fd,
                &staging_name,
                Some(&base_id),
                "staging parquet",
            );
            let _ = staging_dir_fd.sync_all();
            return Ok(StagingOutcome::ScratchRefused(format!(
                "staging allocation failed: {err}"
            )));
        }
        let refused = Arc::new(AtomicBool::new(false));
        let inner_res = (|| -> Result<Option<(u64, String)>, String> {
            macro_rules! parquet_write {
                ($op:expr) => {
                    match $op {
                        Ok(value) => value,
                        Err(err) if refused.load(Ordering::Relaxed) || is_staging_limit(&err) => {
                            return Ok(None)
                        }
                        Err(err) => return Err(err.to_string()),
                    }
                };
            }
            let mut writer = parquet_write!(ArrowWriter::try_new(
                HashingWriter::new(writer_file, max_staged_bytes, Arc::clone(&refused)),
                first_batch.schema(),
                Some(props),
            ));
            parquet_write!(writer.write(&first_batch));
            for next in iter {
                let batch = next?;
                if batch.num_rows() == 0 {
                    continue;
                }
                parquet_write!(writer.write(&batch));
            }
            let hashing = parquet_write!(writer.into_inner());
            let (mut synced_file, written_len, sha256) = hashing.finish();
            if max_staged_bytes.is_some() {
                synced_file
                    .set_len(written_len)
                    .map_err(|e| format!("staging truncate failed: {e}"))?;
            }
            synced_file.sync_all().map_err(|e| e.to_string())?;
            let post_meta = synced_file
                .metadata()
                .map_err(|e| format!("failed inspecting written staging parquet fd: {e}"))?;
            if post_meta.dev() != dev || post_meta.ino() != ino || post_meta.len() != written_len {
                return Err("staging parquet fd changed identity or length during write".to_owned());
            }
            if max_staged_bytes.is_some_and(|max| post_meta.len() > max) {
                return Ok(None);
            }
            if written_len < 12 {
                return Err(format!(
                    "written staging parquet too small ({written_len} bytes)"
                ));
            }
            synced_file
                .seek(SeekFrom::End(-8))
                .map_err(|e| format!("failed seeking staging parquet tail: {e}"))?;
            let mut tail = [0_u8; 8];
            synced_file
                .read_exact(&mut tail)
                .map_err(|e| format!("failed reading staging parquet tail: {e}"))?;
            if &tail[4..8] != b"PAR1" {
                return Err("written staging parquet missing trailing PAR1 magic".to_owned());
            }
            let footer_len = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]) as usize;
            if footer_len == 0
                || u64::try_from(footer_len)
                    .ok()
                    .and_then(|n| n.checked_add(12))
                    .is_none_or(|need| need > written_len)
                || footer_len > MAX_PARQUET_FOOTER_SERIALIZED_BYTES
            {
                return Err(format!(
                    "staging parquet footer length {footer_len} exceeds safe limit {MAX_PARQUET_FOOTER_SERIALIZED_BYTES}"
                ));
            }
            Ok(Some((post_meta.len(), sha256)))
        })();
        let (len, sha256) = match inner_res {
            Ok(Some(res)) => res,
            Ok(None) => {
                let _ = unlink_regular_file_matching_identity_in_dir(
                    &staging_dir_fd,
                    &staging_name,
                    Some(&base_id),
                    "staging parquet",
                );
                let _ = staging_dir_fd.sync_all();
                drop(file);
                return Ok(StagingOutcome::ExceededMaxBytes);
            }
            Err(err) => {
                let _ = unlink_regular_file_matching_identity_in_dir(
                    &staging_dir_fd,
                    &staging_name,
                    Some(&base_id),
                    "staging parquet",
                );
                let _ = staging_dir_fd.sync_all();
                drop(file);
                return Err(err);
            }
        };
        let staged = StagedParquetFile {
            display_part_dir: part_dir,
            staging_dir_fd,
            staging_name,
            final_name,
            _file: file,
            dev,
            ino,
            len,
            sha256,
            published: false,
            intent_recorded: false,
            partition_source: None,
        };
        staged
            .staging_dir_fd
            .sync_all()
            .map_err(|e| format!("failed syncing staging dir: {e}"))?;

        Ok(StagingOutcome::Staged(staged))
    }

    /// Discovers and descriptor-pins Parquet files for `domain`, pruning `dt=YYYY-MM-DD`
    /// directories older than `since_ms` while retaining only the single domain directory FD
    /// and enforcing default snapshot count and metadata byte bounds.
    pub fn snapshot_pinned_parquet_files(
        &self,
        domain: &str,
        since_ms: Option<i64>,
    ) -> Result<Vec<PinnedParquetFile>, String> {
        self.snapshot_pinned_parquet_files_bounded(
            domain,
            since_ms,
            MAX_SNAPSHOT_PARQUET_FILES,
            MAX_SNAPSHOT_METADATA_BYTES,
        )
    }

    /// Discovers and descriptor-pins Parquet files for `domain`, pruning `dt=YYYY-MM-DD`
    /// directories older than `since_ms` while incrementally enforcing `max_files` and
    /// `max_bytes` during `O_DIRECTORY | O_NOFOLLOW` iteration before growing unbounded buffers.
    pub fn snapshot_pinned_parquet_files_bounded(
        &self,
        domain: &str,
        since_ms: Option<i64>,
        max_files: usize,
        max_bytes: usize,
    ) -> Result<Vec<PinnedParquetFile>, String> {
        self.snapshot_selected_parquet_files(
            domain,
            since_ms.map(day_from_ms),
            None,
            max_files,
            max_bytes,
        )
    }

    /// Pins a bounded snapshot of just one known UTC date partition.
    pub fn snapshot_pinned_parquet_files_for_day(
        &self,
        domain: &str,
        day: NaiveDate,
        max_files: usize,
        max_bytes: usize,
    ) -> Result<Vec<PinnedParquetFile>, String> {
        self.snapshot_selected_parquet_files(domain, None, Some(day), max_files, max_bytes)
    }

    fn snapshot_selected_parquet_files(
        &self,
        domain: &str,
        min_day: Option<NaiveDate>,
        exact_day: Option<NaiveDate>,
        max_files: usize,
        max_bytes: usize,
    ) -> Result<Vec<PinnedParquetFile>, String> {
        let domain = validate_olap_domain(domain)?;
        let Some(data_fd) = open_anchor_data_dir_if_exists(&self.layout)? else {
            return Ok(Vec::new());
        };
        let olap_fd = match open_child_dir_nofollow(&data_fd, "olap", "olap dir") {
            Ok(Some(fd)) => fd,
            Ok(None) => return Ok(Vec::new()),
            Err(e) if e.starts_with("non-directory ") => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let domain_fd = match open_child_dir_nofollow(&olap_fd, domain, "domain dir") {
            Ok(Some(fd)) => fd,
            Ok(None) => return Ok(Vec::new()),
            Err(e) if e.starts_with("non-directory ") => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        drop(olap_fd);
        drop(data_fd);
        let domain_fd = Arc::new(domain_fd);
        let domain_dir = self.layout.olap_domain_dir(domain);
        let mut files = Vec::new();
        let mut total_bytes = 0_usize;
        let mut non_empty_partitions = 0_usize;

        let mut append_partition = |name: &str| -> Result<(), String> {
            let part_fd = match open_child_dir_nofollow(&domain_fd, name, "partition dir") {
                Ok(Some(fd)) => fd,
                Ok(None) => return Ok(()),
                Err(e) if e.starts_with("non-directory ") => return Ok(()),
                Err(e) => return Err(e),
            };
            let part_meta = part_fd
                .metadata()
                .map_err(|e| format!("failed inspecting partition dir {name}: {e}"))?;
            let part_anchor = Arc::new(PinnedDomainPartition {
                domain_dir_fd: Arc::clone(&domain_fd),
                part_name: Arc::from(name),
                part_dev: part_meta.dev(),
                part_ino: part_meta.ino(),
            });
            let anchor_bytes = part_anchor.retained_heap_bytes();
            let anchor = PinnedDirAnchor::Domain(part_anchor);
            let part_dir = domain_dir.join(name);
            let (mut partition_files, partition_bytes) =
                list_partition_parquet_snapshot_on_fd_bounded(
                    &part_fd,
                    &anchor,
                    &part_dir,
                    6,
                    files.len(),
                    files.capacity(),
                    non_empty_partitions,
                    total_bytes,
                    anchor_bytes,
                    max_files,
                    max_bytes,
                )?;
            if !partition_files.is_empty() {
                non_empty_partitions = non_empty_partitions.saturating_add(1);
                total_bytes = total_bytes.saturating_add(partition_bytes);
                files.append(&mut partition_files);
            }
            Ok(())
        };
        if let Some(day) = exact_day {
            append_partition(&format!("dt={}", day.format("%Y-%m-%d")))?;
        } else {
            for entry in fs::read_dir(proc_fd_dir_path(&domain_fd)).map_err(|e| e.to_string())? {
                let name = entry.map_err(|e| e.to_string())?.file_name();
                let Some(name) = name.to_str() else { continue };
                let Some(date) = name.strip_prefix("dt=") else {
                    continue;
                };
                let Ok(day) = NaiveDate::parse_from_str(date, "%Y-%m-%d") else {
                    continue;
                };
                if min_day.is_none_or(|cutoff| day >= cutoff) {
                    append_partition(name)?;
                }
            }
        }
        files.sort_by(|a, b| a.inner.display_path.cmp(&b.inner.display_path));
        Ok(files)
    }

    /// Discovers Parquet files for `domain`, pruning `dt=YYYY-MM-DD` directories older than `since_ms`.
    pub fn list_parquet_files(
        &self,
        domain: &str,
        since_ms: Option<i64>,
    ) -> Result<Vec<PathBuf>, String> {
        Ok(self
            .snapshot_pinned_parquet_files(domain, since_ms)?
            .into_iter()
            .map(|p| p.inner.display_path.to_path_buf())
            .collect())
    }
}

fn intent_filter_overhead(entries_len: usize, intent: &ReplaceIntent) -> usize {
    let kept_vec = entries_len.saturating_mul(std::mem::size_of::<PinnedParquetFile>());
    let set_nodes = if intent.inputs.is_empty() {
        0
    } else {
        btree_nodes_bytes::<&str, ()>(intent.inputs.len()).max(1024)
    };
    kept_vec.saturating_add(set_nodes)
}

fn apply_superseded_intent_filter_on_fd(
    part_fd: &File,
    display_part_dir: &Path,
    entries: &mut Vec<PinnedParquetFile>,
    intent: &ReplaceIntent,
) -> Result<bool, String> {
    let superseded: std::collections::BTreeSet<&str> = intent
        .inputs
        .iter()
        .filter(|n| n.as_str() != intent.replacement.as_str())
        .map(String::as_str)
        .collect();
    let mut kept = Vec::with_capacity(entries.len());
    for p in entries.iter() {
        if !superseded.contains(p.inner.file_name.as_ref()) {
            kept.push(p.clone());
            continue;
        }
        let Some(exp_in) = intent.input_ids.get(p.inner.file_name.as_ref()) else {
            // Legacy intent without input_ids: suppress by filename once replacement is verified.
            continue;
        };
        let opened = match open_regular_file_in_dir_nofollow(
            part_fd,
            &p.inner.file_name,
            "superseded parquet",
        ) {
            Ok(res) => res,
            Err(e) if is_concurrent_open_change(&e) => return Ok(false),
            Err(e) => return Err(e),
        };
        let Some((in_f, in_meta)) = opened else {
            return Ok(false);
        };
        if in_meta.dev() != exp_in.dev || in_meta.ino() != exp_in.ino {
            kept.push(p.clone());
        } else if !verify_open_file_matches_identity(&in_f, &in_meta, exp_in)? {
            return Err(format!(
                "superseded parquet {} failed digest verification against replace intent",
                display_part_dir.join(p.inner.file_name.as_ref()).display()
            ));
        }
    }
    *entries = kept;
    Ok(true)
}

fn all_parquet_entries_live_on_fd(
    part_fd: &File,
    entries: &[PinnedParquetFile],
) -> Result<bool, String> {
    for entry in entries {
        let child_proc = proc_fd_child_path(part_fd, &entry.inner.file_name, "parquet file")?;
        match fs::symlink_metadata(&child_proc) {
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
            Err(e) => {
                return Err(format!(
                    "failed inspecting parquet file {}: {e}",
                    entry.inner.display_path.display()
                ));
            }
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "refusing symlinked parquet file {}",
                    entry.inner.display_path.display()
                ));
            }
            Ok(meta) => {
                if !meta.is_file()
                    || meta.dev() != entry.inner.dev
                    || meta.ino() != entry.inner.ino
                    || meta.len() != entry.inner.len
                {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

fn is_concurrent_open_change(err: &str) -> bool {
    err.contains("changed concurrently during open")
}

#[allow(dead_code)]
fn list_partition_parquet_snapshot(part_dir: &Path) -> Result<Vec<PathBuf>, String> {
    list_partition_parquet_snapshot_with_attempts(part_dir, 6)
}

#[allow(dead_code)]
fn list_partition_parquet_snapshot_with_attempts(
    part_dir: &Path,
    max_attempts: usize,
) -> Result<Vec<PathBuf>, String> {
    let Some(part_fd) = open_standalone_dir_nofollow(part_dir, "partition dir")? else {
        return Err(format!(
            "failed reading {}: No such file or directory",
            part_dir.display()
        ));
    };
    let anchor = PinnedDirAnchor::Partition {
        part_dir_fd: Arc::new(part_fd),
    };
    let PinnedDirAnchor::Partition { ref part_dir_fd } = anchor else {
        unreachable!();
    };
    let pinned =
        list_partition_parquet_snapshot_on_fd(part_dir_fd, &anchor, part_dir, max_attempts)?;
    Ok(pinned
        .into_iter()
        .map(|p| p.inner.display_path.to_path_buf())
        .collect())
}

pub(crate) fn list_partition_parquet_snapshot_on_fd(
    part_fd: &File,
    anchor: &PinnedDirAnchor,
    display_part_dir: &Path,
    max_attempts: usize,
) -> Result<Vec<PinnedParquetFile>, String> {
    let (entries, _) = list_partition_parquet_snapshot_on_fd_bounded(
        part_fd,
        anchor,
        display_part_dir,
        max_attempts,
        0,
        0,
        0,
        0,
        0,
        MAX_SNAPSHOT_PARQUET_FILES,
        MAX_SNAPSHOT_METADATA_BYTES,
    )?;
    Ok(entries)
}

#[allow(clippy::too_many_arguments)]
fn list_partition_parquet_snapshot_on_fd_bounded(
    part_fd: &File,
    anchor: &PinnedDirAnchor,
    display_part_dir: &Path,
    max_attempts: usize,
    base_files: usize,
    files_cap: usize,
    non_empty_partitions: usize,
    base_bytes: usize,
    anchor_bytes: usize,
    max_files: usize,
    max_bytes: usize,
) -> Result<(Vec<PinnedParquetFile>, usize), String> {
    let slot_bytes = std::mem::size_of::<PinnedParquetFile>();
    let overflow_parts = non_empty_partitions.saturating_add(1).saturating_sub(64);
    let tracker_overflow_bytes = if overflow_parts == 0 {
        0
    } else {
        overflow_parts
            .max(4)
            .checked_next_power_of_two()
            .unwrap_or(usize::MAX)
            .saturating_mul(std::mem::size_of::<usize>())
    };
    let domain_fd_bytes = if base_files == 0 && matches!(anchor, PinnedDirAnchor::Domain(_)) {
        arc_file_heap_bytes()
    } else {
        0
    };
    for _attempt in 0..max_attempts {
        let epoch_before = match load_replace_epoch_on_fd(part_fd, display_part_dir) {
            Ok(v) => v,
            Err(e) if is_concurrent_open_change(&e) => continue,
            Err(e) => return Err(e),
        };
        let files_spare_bytes = files_cap
            .saturating_sub(base_files)
            .saturating_mul(slot_bytes);
        let intent_before_budget = max_bytes
            .saturating_sub(base_bytes)
            .saturating_sub(files_spare_bytes)
            .saturating_sub(tracker_overflow_bytes);
        let (intent_before, intent_before_retained, intent_before_peak) =
            match load_replace_intent_on_fd_measured(
                part_fd,
                display_part_dir,
                intent_before_budget,
            ) {
                Ok(Some((intent, peak))) => {
                    let retained = intent.retained_heap_bytes();
                    (Some(intent), retained, peak)
                }
                Ok(None) => (None, 0, 0),
                Err(e) if is_concurrent_open_change(&e) => continue,
                Err(e) => return Err(e),
            };
        if base_bytes
            .saturating_add(files_spare_bytes)
            .saturating_add(tracker_overflow_bytes)
            .saturating_add(intent_before_peak)
            > max_bytes
        {
            return Err(format!(
                "Resources exhausted: pinned parquet snapshot metadata + replace intent exceeds limit {max_bytes} bytes in {}",
                display_part_dir.display()
            ));
        }

        let mut entries = Vec::new();
        let mut entries_bytes = 0_usize;
        let mut saw_missing = false;
        for part in fs::read_dir(proc_fd_dir_path(part_fd)).map_err(|e| e.to_string())? {
            let part = part.map_err(|e| e.to_string())?;
            let file_name_os = part.file_name();
            let Some(file_name) = file_name_os.to_str() else {
                continue;
            };
            if Path::new(file_name)
                .extension()
                .is_some_and(|ext| ext == "parquet")
            {
                validate_parquet_filename(file_name, "parquet file")?;
                let child_proc = proc_fd_child_path(part_fd, file_name, "parquet file")?;
                match fs::symlink_metadata(&child_proc) {
                    Ok(meta) => {
                        if meta.file_type().is_symlink() {
                            return Err(format!(
                                "refusing symlinked parquet file {}",
                                display_part_dir.join(file_name).display()
                            ));
                        }
                        if meta.is_file() {
                            let next_files = base_files
                                .checked_add(entries.len())
                                .and_then(|n| n.checked_add(1));
                            let Some(total_count) = next_files.filter(|&n| n <= max_files) else {
                                return Err(format!(
                                    "Resources exhausted: pinned parquet snapshot file count exceeds limit {max_files} in {}",
                                    display_part_dir.display()
                                ));
                            };
                            let display_path = display_part_dir.join(file_name);
                            let entry_bytes = PinnedParquetFile::estimate_retained_heap_bytes(
                                file_name.len(),
                                display_path.as_os_str().len(),
                                None,
                                false,
                            );
                            let extra_anchor = if entries.is_empty() {
                                anchor_bytes.saturating_add(domain_fd_bytes)
                            } else {
                                0
                            };
                            let next_entries_len = entries.len().saturating_add(1);
                            let entries_next_cap = if entries.len() == entries.capacity() {
                                entries.len().saturating_mul(2).max(4)
                            } else {
                                entries.capacity()
                            };
                            let files_next_cap = if total_count > files_cap {
                                total_count.next_power_of_two()
                            } else {
                                files_cap
                            };
                            let concurrent_vec_overhead = files_next_cap
                                .saturating_add(entries_next_cap)
                                .saturating_sub(total_count)
                                .saturating_mul(slot_bytes);
                            let sort_and_tracker_overhead = files_next_cap
                                .saturating_mul(slot_bytes)
                                .saturating_add(tracker_overflow_bytes);
                            let filter_overhead = intent_before.as_ref().map_or(0, |intent| {
                                intent_filter_overhead(next_entries_len, intent)
                            });
                            let container_overhead = concurrent_vec_overhead
                                .saturating_add(filter_overhead)
                                .max(sort_and_tracker_overhead);
                            let added_bytes = extra_anchor.checked_add(entry_bytes);
                            let next_bytes = added_bytes.and_then(|d| {
                                base_bytes
                                    .checked_add(entries_bytes)?
                                    .checked_add(d)?
                                    .checked_add(container_overhead)?
                                    .checked_add(intent_before_retained)
                            });
                            if next_bytes.is_none_or(|b| b > max_bytes) {
                                return Err(format!(
                                    "Resources exhausted: pinned parquet snapshot metadata exceeds limit {max_bytes} bytes in {}",
                                    display_part_dir.display()
                                ));
                            }
                            entries_bytes =
                                entries_bytes.saturating_add(added_bytes.unwrap_or(usize::MAX));
                            entries.push(PinnedParquetFile {
                                inner: Arc::new(PinnedParquetFileInner {
                                    anchor: anchor.clone(),
                                    file_name: Arc::from(file_name),
                                    display_path: Arc::from(display_path.into_boxed_path()),
                                    dev: meta.dev(),
                                    ino: meta.ino(),
                                    len: meta.len(),
                                    sha256: None,
                                    _retained_fd: None,
                                }),
                            });
                        } else {
                            return Err(format!(
                                "non-regular parquet file {}",
                                display_part_dir.join(file_name).display()
                            ));
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::NotFound => {
                        saw_missing = true;
                    }
                    Err(e) => {
                        return Err(format!(
                            "failed inspecting parquet file {}: {e}",
                            display_part_dir.join(file_name).display()
                        ));
                    }
                }
            }
        }
        let total_count = base_files.saturating_add(entries.len());
        let files_next_cap = if total_count > files_cap {
            total_count.next_power_of_two()
        } else {
            files_cap
        };
        let active_container_overhead = files_next_cap
            .saturating_add(entries.capacity())
            .saturating_sub(total_count)
            .saturating_mul(slot_bytes)
            .max(
                files_next_cap
                    .saturating_mul(slot_bytes)
                    .saturating_add(tracker_overflow_bytes),
            );
        let intent_after_budget = max_bytes
            .saturating_sub(base_bytes)
            .saturating_sub(entries_bytes)
            .saturating_sub(active_container_overhead)
            .saturating_sub(intent_before_retained);
        let (intent_after, intent_after_retained, intent_after_peak) =
            match load_replace_intent_on_fd_measured(part_fd, display_part_dir, intent_after_budget)
            {
                Ok(Some((intent, peak))) => {
                    let retained = intent.retained_heap_bytes();
                    (Some(intent), retained, peak)
                }
                Ok(None) => (None, 0, 0),
                Err(e) if is_concurrent_open_change(&e) => continue,
                Err(e) => return Err(e),
            };
        if base_bytes
            .saturating_add(entries_bytes)
            .saturating_add(active_container_overhead)
            .saturating_add(intent_before_retained)
            .saturating_add(intent_after_peak)
            > max_bytes
        {
            return Err(format!(
                "Resources exhausted: pinned parquet snapshot metadata + replace intent exceeds limit {max_bytes} bytes in {}",
                display_part_dir.display()
            ));
        }
        let epoch_after = match load_replace_epoch_on_fd(part_fd, display_part_dir) {
            Ok(v) => v,
            Err(e) if is_concurrent_open_change(&e) => continue,
            Err(e) => return Err(e),
        };

        let same_epoch = epoch_before == epoch_after;
        let same_intent = intent_before == intent_after;
        drop(intent_before);

        if same_epoch && same_intent && !saw_missing {
            match intent_after.as_ref() {
                None => {
                    if all_parquet_entries_live_on_fd(part_fd, &entries)? {
                        let stable_epoch = match load_replace_epoch_on_fd(part_fd, display_part_dir)
                        {
                            Ok(v) => v == epoch_after,
                            Err(e) if is_concurrent_open_change(&e) => continue,
                            Err(e) => return Err(e),
                        };
                        let stable_intent_budget = max_bytes
                            .saturating_sub(base_bytes)
                            .saturating_sub(entries_bytes)
                            .saturating_sub(active_container_overhead);
                        let still_none = match load_replace_intent_on_fd_measured(
                            part_fd,
                            display_part_dir,
                            stable_intent_budget,
                        ) {
                            Ok(opt) => opt.is_none(),
                            Err(e) if is_concurrent_open_change(&e) => continue,
                            Err(e) => return Err(e),
                        };
                        if stable_epoch && still_none {
                            return Ok((entries, entries_bytes));
                        }
                    }
                }
                Some(intent) => {
                    if intent.poisoned {
                        return Err(format!(
                            "poisoned replace intent in {} after post-rename verification failure",
                            display_part_dir.display()
                        ));
                    }
                    let rep_opened = match open_regular_file_in_dir_nofollow(
                        part_fd,
                        &intent.replacement,
                        "replacement parquet",
                    ) {
                        Ok(res) => res,
                        Err(e) if is_concurrent_open_change(&e) => continue,
                        Err(e) => return Err(e),
                    };
                    match rep_opened {
                        Some((rep_f, rep_meta)) => {
                            let Some(rep_idx) = entries.iter().position(|p| {
                                p.inner.file_name.as_ref() == intent.replacement.as_str()
                            }) else {
                                continue;
                            };
                            if entries[rep_idx].inner.dev != rep_meta.dev()
                                || entries[rep_idx].inner.ino != rep_meta.ino()
                                || entries[rep_idx].inner.len != rep_meta.len()
                            {
                                continue;
                            }
                            if let Some(ref exp_rep) = intent.replacement_id {
                                if !verify_open_file_matches_identity(&rep_f, &rep_meta, exp_rep)? {
                                    return Err(format!(
                                        "replacement parquet {} failed identity/sha256 verification against replace intent",
                                        display_part_dir.join(&intent.replacement).display()
                                    ));
                                }
                                if let Some(ref sha) = exp_rep.sha256 {
                                    let old_bytes = entries[rep_idx].retained_heap_bytes();
                                    Arc::make_mut(&mut entries[rep_idx].inner).sha256 =
                                        Some(Arc::from(sha.as_str()));
                                    let new_bytes = entries[rep_idx].retained_heap_bytes();
                                    entries_bytes = entries_bytes
                                        .saturating_sub(old_bytes)
                                        .saturating_add(new_bytes);
                                }
                            }
                            let filter_overhead = intent_filter_overhead(entries.len(), intent);
                            let total_with_filter = base_bytes
                                .checked_add(entries_bytes)
                                .and_then(|b| b.checked_add(active_container_overhead))
                                .and_then(|b| b.checked_add(filter_overhead))
                                .and_then(|b| b.checked_add(intent_after_retained));
                            if total_with_filter.is_none_or(|b| b > max_bytes) {
                                return Err(format!(
                                    "Resources exhausted: pinned parquet snapshot metadata exceeds limit {max_bytes} bytes in {}",
                                    display_part_dir.display()
                                ));
                            }
                            if !apply_superseded_intent_filter_on_fd(
                                part_fd,
                                display_part_dir,
                                &mut entries,
                                intent,
                            )? {
                                continue;
                            }
                            entries_bytes = if entries.is_empty() {
                                0
                            } else {
                                anchor_bytes.saturating_add(domain_fd_bytes).saturating_add(
                                    entries
                                        .iter()
                                        .map(PinnedParquetFile::retained_heap_bytes)
                                        .fold(0_usize, usize::saturating_add),
                                )
                            };
                            if all_parquet_entries_live_on_fd(part_fd, &entries)? {
                                return Ok((entries, entries_bytes));
                            }
                        }
                        None => {
                            if entries
                                .iter()
                                .any(|p| p.inner.file_name.as_ref() == intent.replacement.as_str())
                            {
                                continue;
                            }
                            let stable_intent_budget = max_bytes
                                .saturating_sub(base_bytes)
                                .saturating_sub(entries_bytes)
                                .saturating_sub(active_container_overhead)
                                .saturating_sub(intent_after_retained);
                            let inputs_check = verify_uncommitted_intent_inputs_on_fd_with_budget(
                                part_fd,
                                display_part_dir,
                                intent,
                                stable_intent_budget,
                            );
                            if let Err(ref err) = inputs_check
                                && err.starts_with("Resources exhausted:")
                            {
                                return Err(err.clone());
                            }
                            if all_parquet_entries_live_on_fd(part_fd, &entries)? {
                                let stable_epoch =
                                    match load_replace_epoch_on_fd(part_fd, display_part_dir) {
                                        Ok(v) => v == epoch_after,
                                        Err(e) if is_concurrent_open_change(&e) => continue,
                                        Err(e) => return Err(e),
                                    };
                                let stable_intent = match load_replace_intent_on_fd_measured(
                                    part_fd,
                                    display_part_dir,
                                    stable_intent_budget,
                                ) {
                                    Ok(v) => v.map(|(i, _)| i).as_ref() == Some(intent),
                                    Err(e) if is_concurrent_open_change(&e) => continue,
                                    Err(e) => return Err(e),
                                };
                                if stable_epoch && stable_intent {
                                    match inputs_check {
                                        Ok(true) => return Ok((entries, entries_bytes)),
                                        Ok(false) => continue,
                                        Err(err) => return Err(err),
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Err(format!(
        "NotFound: unstable partition snapshot in {}",
        display_part_dir.display()
    ))
}

struct StreamingParquetBatches {
    files: Vec<PinnedParquetFile>,
    file_idx: usize,
    batch_size: usize,
    current_reader: Option<VerifiedParquetBatchReader>,
}

impl StreamingParquetBatches {
    fn new(files: &[PinnedParquetFile], batch_size: usize) -> Self {
        Self {
            files: files.to_vec(),
            file_idx: 0,
            batch_size,
            current_reader: None,
        }
    }
}

impl Iterator for StreamingParquetBatches {
    type Item = Result<RecordBatch, String>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(reader) = self.current_reader.as_mut() {
                match reader.next() {
                    Some(Ok(batch)) => return Some(Ok(batch)),
                    Some(Err(e)) => {
                        self.current_reader = None;
                        self.file_idx = self.files.len();
                        return Some(Err(e.to_string()));
                    }
                    None => {
                        self.current_reader = None;
                    }
                }
            }
            if self.file_idx >= self.files.len() {
                return None;
            }
            let pinned = &self.files[self.file_idx];
            self.file_idx += 1;
            let file = match pinned.open_verified() {
                Ok(f) => f,
                Err(e) => {
                    self.file_idx = self.files.len();
                    return Some(Err(e));
                }
            };
            match open_verified_parquet_batch_reader_from_file(file, None, self.batch_size) {
                Ok(r) => self.current_reader = Some(r),
                Err(e) => {
                    self.file_idx = self.files.len();
                    return Some(Err(e));
                }
            }
        }
    }
}

/// Converts Unix milliseconds into UTC calendar date.
#[must_use]
pub fn day_from_ms(ms: i64) -> NaiveDate {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_else(Utc::now)
        .date_naive()
}

fn scan_dir_disk_bytes_level(
    dir_fd: File,
    total: &mut u64,
) -> Result<(File, std::vec::IntoIter<String>), String> {
    let mut subdirs = Vec::new();
    for entry in fs::read_dir(proc_fd_dir_path(&dir_fd)).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let fname = entry.file_name();
        let Some(name) = fname.to_str() else {
            return Err("non-utf8 entry in directory tree".to_owned());
        };
        let child_proc = proc_fd_child_path(&dir_fd, name, "dir entry")?;
        let meta = match fs::symlink_metadata(&child_proc) {
            Ok(m) => m,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("failed inspecting {name}: {e}")),
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            subdirs.push(name.to_owned());
        } else if meta.is_file() {
            *total = total.saturating_add(meta.len());
        }
    }
    Ok((dir_fd, subdirs.into_iter()))
}

/// Computes total disk bytes under a directory tree without following symlinks using `O(depth)` FDs.
pub fn dir_disk_bytes(root: &Path) -> Result<u64, String> {
    let Some(root_fd) = open_standalone_dir_nofollow(root, "dir")? else {
        return Ok(0);
    };
    let mut total = 0u64;
    let mut stack = vec![scan_dir_disk_bytes_level(root_fd, &mut total)?];
    while let Some((dir_fd, mut subdirs)) = stack.pop() {
        let Some(name) = subdirs.next() else {
            continue;
        };
        let sub_res = open_child_dir_nofollow(&dir_fd, &name, "subdir");
        stack.push((dir_fd, subdirs));
        match sub_res {
            Ok(Some(sub_fd)) => stack.push(scan_dir_disk_bytes_level(sub_fd, &mut total)?),
            Ok(None) => {}
            Err(e) if e.starts_with("non-directory ") => {}
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use parquet::file::metadata::ParquetMetaDataReader;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    #[test]
    fn spindle_provenance_survives_mixed_legacy_compaction() {
        use arrow::array::Array;

        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let store = OlapParquetStore::new(catalog.layout().clone());
        let row = LogRecord {
            timestamp_ms: Utc::now().timestamp_millis(),
            level: "info".into(),
            source: "receipt".into(),
            unit: "test".into(),
            message: "observation".into(),
            attributes_json: r#"{"_spindle_provenance":"lookalike"}"#.into(),
        };
        let old_files = store.write_logs(std::slice::from_ref(&row)).unwrap();
        let key = SpindleProvenance {
            origin: "local-origin".into(),
            record_id: 9,
            producer: "receipt".into(),
            generation: "boot".into(),
            sequence: 4,
        };
        let mut plan = store
            .plan_spindle_flush(&[], &[(Some(key.clone()), row.clone())], &[])
            .unwrap();
        store.execute_flush_plan(&mut plan).unwrap();
        let day = day_from_ms(row.timestamp_ms);
        let pinned = store
            .snapshot_pinned_parquet_files_for_day("logs", day, 2, MAX_SNAPSHOT_METADATA_BYTES)
            .unwrap();
        let projected = pinned
            .iter()
            .find(|file| file.display_path() != old_files[0])
            .unwrap();
        let compacted = store
            .compact_parquet_files(
                "logs",
                day,
                &[old_files[0].clone(), projected.display_path().to_owned()],
            )
            .unwrap()
            .unwrap();
        let reader = open_verified_parquet_batch_reader_from_file(
            File::open(compacted).unwrap(),
            Some(&LOGS_SCHEMA),
            1,
        )
        .unwrap();
        let mut observed = Vec::new();
        for batch in reader {
            let batch = batch.unwrap();
            let provenance = batch
                .column_by_name(SPINDLE_PROVENANCE_COLUMN)
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            observed.extend((0..provenance.len()).map(|i| {
                (!provenance.is_null(i)).then(|| {
                    serde_json::from_str::<SpindleProvenance>(provenance.value(i)).unwrap()
                })
            }));
        }
        assert_eq!(observed, [None, Some(key)]);
    }

    fn non_tmp_dir() -> TempDir {
        if let Some(env_dir) = std::env::var_os("TMPDIR") {
            let p = PathBuf::from(env_dir);
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
    fn write_atomic_json_in_dir_bounded_rejects_oversized_and_cleans_tmp() {
        let dir = non_tmp_dir();
        let big = serde_json::json!({ "payload": "x".repeat(512) });
        let err = write_atomic_json_in_dir_bounded(dir.path(), "sidecar.json", &big, 128)
            .expect_err("oversized JSON must fail");
        assert!(err.contains("exceeds max size"), "unexpected err: {err}");
        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            entries.is_empty(),
            "aborted bounded write must not leave .tmp or final files: {entries:?}"
        );

        let small = serde_json::json!({ "ok": 1 });
        write_atomic_json_in_dir_bounded(dir.path(), "sidecar.json", &small, 128).unwrap();
        assert!(dir.path().join("sidecar.json").is_file());
    }

    #[test]
    fn rejects_symlinked_partition_and_parquet_on_write_and_read() {
        let dir = non_tmp_dir();
        let outside = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let day = NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        let ts = day
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2026-11-15");

        // 1. Symlinked dt=... partition dir rejected on write and read.
        symlink(outside.path(), &part_dir).unwrap();
        let write_err = olap
            .write_metrics(&[MetricPoint {
                timestamp_ms: ts,
                name: "sym.write".into(),
                value: 1.0,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "u".into(),
                pid: None,
                labels_json: "{}".into(),
            }])
            .expect_err("symlinked partition dir must fail on write");
        assert!(
            write_err.contains("symlink"),
            "unexpected write err: {write_err}"
        );

        let read_err = olap
            .list_parquet_files("metrics", None)
            .expect_err("symlinked partition dir must fail on read");
        assert!(
            read_err.contains("symlink"),
            "unexpected read err: {read_err}"
        );

        // 2. Real partition dir with a symlinked .parquet file rejected on read.
        fs::remove_file(&part_dir).unwrap();
        fs::create_dir_all(&part_dir).unwrap();
        let fake_target = outside.path().join("evil.parquet");
        fs::write(&fake_target, b"PAR1").unwrap();
        symlink(&fake_target, part_dir.join("evil.parquet")).unwrap();
        let parquet_err = olap
            .list_parquet_files("metrics", None)
            .expect_err("symlinked parquet file must fail on read");
        assert!(
            parquet_err.contains("symlink"),
            "unexpected parquet err: {parquet_err}"
        );
    }

    #[test]
    fn execute_flush_plan_dedups_post_rename_retry_and_bumps_replace_epoch() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let day1_ms = 1_700_000_000_000;
        let day2_ms = 1_700_086_400_000;
        let pts = vec![
            MetricPoint {
                timestamp_ms: day1_ms,
                name: "m1".into(),
                value: 1.0,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "u".into(),
                pid: None,
                labels_json: "{}".into(),
            },
            MetricPoint {
                timestamp_ms: day2_ms,
                name: "m2".into(),
                value: 2.0,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "u".into(),
                pid: None,
                labels_json: "{}".into(),
            },
        ];
        let mut plan = olap.plan_flush(&pts, &[], &[], &[], &[]).unwrap();
        assert_eq!(plan.total_rows(), 2);

        // Simulate retry after rename succeeded for entry 1 before sync/pop completed.
        let first_item = plan.pending.front().unwrap().clone();
        olap.write_batches_to_partition(
            first_item.domain,
            first_item.day,
            Some(&first_item.final_name),
            std::iter::once(Ok(first_item.batch.clone())),
            true,
        )
        .unwrap();

        olap.execute_flush_plan(&mut plan).unwrap();
        assert!(plan.is_empty());
        let files = olap.list_parquet_files("metrics", None).unwrap();
        assert_eq!(
            files.len(),
            2,
            "sticky filename retry must not duplicate day1"
        );

        // Compaction advances .replace-epoch cleanly.
        let day1_files = olap
            .list_parquet_files("metrics", Some(day1_ms))
            .unwrap()
            .into_iter()
            .filter(|p| p.parent().unwrap().ends_with("dt=2023-11-14"))
            .collect::<Vec<_>>();
        assert_eq!(day1_files.len(), 1);
        olap.write_metrics(&[pts[0].clone()]).unwrap();
        let d1_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        let before_compact = list_partition_parquet_snapshot(&d1_dir).unwrap();
        assert_eq!(before_compact.len(), 2);
        olap.compact_parquet_files("metrics", day_from_ms(day1_ms), &before_compact)
            .unwrap();
        assert_eq!(load_replace_epoch(&d1_dir).unwrap(), Some(1));
        assert_eq!(list_partition_parquet_snapshot(&d1_dir).unwrap().len(), 1);
    }

    #[test]
    fn durable_sync_chain_includes_root_for_top_level_dir() {
        assert_eq!(
            build_durable_sync_chain(Path::new("/fabrico11y_root_dir"), None),
            vec![PathBuf::from("/"), PathBuf::from("/fabrico11y_root_dir")]
        );
    }

    #[test]
    fn snapshot_exhaustion_fails_closed_instead_of_partial_listing() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let day1_ms = 1_700_000_000_000;
        olap.write_metrics(&[MetricPoint {
            timestamp_ms: day1_ms,
            name: "m.snapshot".into(),
            value: 1.0,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        }])
        .unwrap();
        let d1_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join("dt=2023-11-14");
        let err = list_partition_parquet_snapshot_with_attempts(&d1_dir, 0)
            .expect_err("exhausted snapshot attempts must fail closed");
        assert!(
            err.contains("unstable partition snapshot"),
            "unexpected snapshot error: {err}"
        );
    }

    #[test]
    fn execute_flush_plan_does_not_duplicate_after_post_rename_compaction() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let day1_ms = 1_700_000_000_000;
        let pt = MetricPoint {
            timestamp_ms: day1_ms,
            name: "m.compacted".into(),
            value: 42.0,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        };
        let mut plan = olap
            .plan_flush(std::slice::from_ref(&pt), &[], &[], &[], &[])
            .unwrap();
        // Simulate real PostRenameSync failure on initial publish, followed by external
        // compaction of the sticky filename before retry.
        {
            let maint = acquire_maintenance_lock(olap.layout()).unwrap();
            let first_item = plan.pending.front_mut().unwrap();
            let day1_path = olap
                .layout()
                .olap_domain_dir(first_item.domain)
                .join(format!("dt={}", first_item.day.format("%Y-%m-%d")))
                .join(&first_item.final_name);
            let sync_err = olap
                .write_planned_partition_with_staging_sync(first_item, &maint, |_| {
                    Err("injected staging sync failure".to_owned())
                })
                .expect_err("staging sync failure must surface");
            assert!(sync_err.contains("injected staging sync failure"));
            assert!(
                first_item.published.is_some(),
                "post-rename sync failure must still record published identity"
            );
            drop(maint);
            let extra_path = olap.write_metrics(std::slice::from_ref(&pt)).unwrap()[0].clone();
            olap.compact_parquet_files(first_item.domain, first_item.day, &[day1_path, extra_path])
                .unwrap();
        }

        olap.execute_flush_plan(&mut plan).unwrap();
        assert!(plan.is_empty());
        let files = olap.list_parquet_files("metrics", None).unwrap();
        assert_eq!(
            files.len(),
            1,
            "post-rename retry after external compaction must not rewrite duplicate parquet file"
        );

        // Pre-populated foreign parquet at sticky filename must fail closed instead of deduplicating.
        let wrong_pt = MetricPoint {
            timestamp_ms: day1_ms,
            name: "m.wrong".into(),
            value: 99.0,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        };
        let mut wrong_plan = olap
            .plan_flush(std::slice::from_ref(&wrong_pt), &[], &[], &[], &[])
            .unwrap();
        let mut wrong_item = wrong_plan.pending.pop_front().unwrap();
        olap.write_batches_to_partition(
            wrong_item.domain,
            wrong_item.day,
            Some(&wrong_item.final_name),
            std::iter::once(Ok(metrics_to_record_batch(&[pt]).unwrap())),
            true,
        )
        .unwrap();
        let maint = acquire_maintenance_lock(olap.layout()).unwrap();
        let mismatch_err = olap
            .write_planned_partition(&mut wrong_item, &maint)
            .expect_err("foreign sticky filename content must fail closed");
        assert!(
            mismatch_err.contains("does not match planned batch"),
            "unexpected mismatch error: {mismatch_err}"
        );
    }

    #[test]
    fn nested_dir_creation_retry_syncs_original_ancestor_when_stop_is_none() {
        let dir = non_tmp_dir();
        let existing = dir.path().join("existing");
        fs::create_dir_all(&existing).unwrap();
        let existing_canon = existing.canonicalize().unwrap();
        let target = existing.join("new/root");

        let fail_err = ensure_durable_non_tmp_dir_with(None, &target, |p| {
            if p == existing_canon {
                Err("injected ancestor sync failure".to_owned())
            } else {
                Ok(())
            }
        })
        .expect_err("ancestor sync failure must surface");
        assert!(fail_err.contains("injected ancestor sync failure"));
        assert!(
            existing.join("new").is_dir(),
            "first attempt should leave intermediate dir on disk"
        );
        assert!(
            !target.exists(),
            "leaf dir should not yet exist after ancestor sync error"
        );

        let mut synced = Vec::new();
        let resolved = ensure_durable_non_tmp_dir_with(None, &target, |p| {
            synced.push(p.to_path_buf());
            Ok(())
        })
        .expect("retry should succeed and sync full ancestor chain");
        assert!(resolved.ends_with("existing/new/root"));

        let intermediate_canon = existing.join("new").canonicalize().unwrap();
        let target_canon = target.canonicalize().unwrap();
        let idx_existing = synced
            .iter()
            .position(|p| p == &existing_canon)
            .expect("retry must still sync original existing ancestor");
        let idx_intermediate = synced
            .iter()
            .position(|p| p == &intermediate_canon)
            .expect("retry must sync intermediate dir");
        let idx_target = synced
            .iter()
            .position(|p| p == &target_canon)
            .expect("retry must sync leaf dir");
        assert!(idx_existing < idx_intermediate && idx_intermediate < idx_target);
    }

    #[test]
    fn writer_lock_rejects_second_concurrent_holder() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let first = try_acquire_writer_lock(catalog.layout()).expect("first writer lock");
        let err = try_acquire_writer_lock(catalog.layout())
            .expect_err("second concurrent writer lock must fail");
        assert!(
            err.contains("storage writer lock busy"),
            "unexpected writer lock error: {err}"
        );
        drop(first);
        let _reacquired =
            try_acquire_writer_lock(catalog.layout()).expect("lock reusable after drop");
    }

    #[test]
    fn compact_parquet_files_reconciles_prior_intent_before_overwriting() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let day_ms = 1_700_000_000_000_i64;
        let day = day_from_ms(day_ms);
        let make_pt = |idx: i64| MetricPoint {
            timestamp_ms: day_ms + idx * 1_000,
            name: format!("compact.intent.{idx}"),
            value: idx as f64,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        };

        let a_path = olap.write_metrics(&[make_pt(1)]).unwrap().pop().unwrap();
        let b_path = olap.write_metrics(&[make_pt(2)]).unwrap().pop().unwrap();
        let c_path = olap.write_metrics(&[make_pt(3)]).unwrap().pop().unwrap();
        let d_path = olap.write_metrics(&[make_pt(4)]).unwrap().pop().unwrap();

        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join(format!("dt={}", day.format("%Y-%m-%d")));
        let stale_intent = ReplaceIntent {
            replacement: c_path.file_name().unwrap().to_string_lossy().into_owned(),
            replacement_id: None,
            inputs: vec![
                a_path.file_name().unwrap().to_string_lossy().into_owned(),
                b_path.file_name().unwrap().to_string_lossy().into_owned(),
            ],
            input_ids: BTreeMap::new(),
            poisoned: false,
            staged_source: None,
        };
        write_atomic_file_in_dir(
            &part_dir,
            REPLACE_INTENT_FILE,
            &serde_json::to_vec(&stale_intent).unwrap(),
        )
        .unwrap();

        let e_path = olap
            .compact_parquet_files("metrics", day, &[c_path.clone(), d_path.clone()])
            .unwrap()
            .expect("compaction produces replacement parquet");
        assert!(e_path.is_file());
        assert!(
            !a_path.exists() && !b_path.exists() && !c_path.exists() && !d_path.exists(),
            "prior superseded inputs and current compaction inputs must all be cleaned up"
        );
        assert!(
            !part_dir.join(REPLACE_INTENT_FILE).exists(),
            "replace intent must be reconciled and removed"
        );
        let live = list_partition_parquet_snapshot(&part_dir).unwrap();
        assert_eq!(live, vec![e_path.clone()]);
        let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(&e_path).unwrap())
            .unwrap()
            .build()
            .unwrap();
        let total_rows: usize = reader.map(|r| r.unwrap().num_rows()).sum();
        assert_eq!(total_rows, 2);
    }

    #[test]
    fn ensure_durable_layout_subdir_rejects_post_open_non_tmp_symlinks() {
        let dir = non_tmp_dir();
        let outside = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let layout = catalog.layout().clone();

        fs::remove_dir_all(layout.spill_dir()).unwrap();
        symlink(outside.path(), layout.spill_dir()).unwrap();
        let spill_err = ensure_durable_layout_subdir(&layout, &layout.spill_dir())
            .expect_err("symlinked spill dir must fail");
        assert!(spill_err.contains("symlink"), "unexpected: {spill_err}");

        fs::remove_dir_all(layout.staging_dir()).unwrap();
        symlink(outside.path(), layout.staging_dir()).unwrap();
        let staging_err = ensure_durable_layout_subdir(&layout, &layout.staging_dir())
            .expect_err("symlinked staging dir must fail");
        assert!(staging_err.contains("symlink"), "unexpected: {staging_err}");

        fs::remove_dir_all(layout.state_dir()).unwrap();
        symlink(outside.path(), layout.state_dir()).unwrap();
        let wm_err = catalog
            .set_watermark("spindle_watermark_id", 7)
            .expect_err("symlinked state dir must fail set_watermark");
        assert!(wm_err.contains("symlink"), "unexpected: {wm_err}");
        let lock_err = try_acquire_writer_lock(&layout)
            .expect_err("symlinked state dir must fail try_acquire_writer_lock");
        assert!(lock_err.contains("symlink"), "unexpected: {lock_err}");
    }

    #[test]
    fn reconcile_partition_intent_rejects_traversal_and_oversized_intent() {
        let dir = non_tmp_dir();
        let outside = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let day_ms = 1_700_000_000_000_i64;
        let day = day_from_ms(day_ms);
        let rep = olap
            .write_metrics(&[MetricPoint {
                timestamp_ms: day_ms,
                name: "intent.keep".into(),
                value: 1.0,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "u".into(),
                pid: None,
                labels_json: "{}".into(),
            }])
            .unwrap()
            .pop()
            .unwrap();
        let rep_name = rep.file_name().unwrap().to_string_lossy().into_owned();
        let part_dir = catalog
            .layout()
            .olap_domain_dir("metrics")
            .join(format!("dt={}", day.format("%Y-%m-%d")));

        let victim = outside.path().join("victim.parquet");
        fs::write(&victim, b"do-not-delete").unwrap();

        for bad_input in [
            "../victim.parquet",
            victim.to_string_lossy().as_ref(),
            "nested/victim.parquet",
            "victim.txt",
        ] {
            let intent = ReplaceIntent {
                replacement: rep_name.clone(),
                replacement_id: None,
                inputs: vec![bad_input.to_owned()],
                input_ids: BTreeMap::new(),
                poisoned: false,
                staged_source: None,
            };
            write_atomic_file_in_dir(
                &part_dir,
                REPLACE_INTENT_FILE,
                &serde_json::to_vec(&intent).unwrap(),
            )
            .unwrap();
            let err = reconcile_partition_intent(catalog.layout(), &part_dir)
                .expect_err("invalid replace intent input must fail");
            assert!(err.contains("invalid"), "unexpected err: {err}");
            assert!(victim.is_file(), "victim outside part_dir must survive");
        }

        let bad_rep = ReplaceIntent {
            replacement: "../escape.parquet".to_owned(),
            replacement_id: None,
            inputs: vec![rep_name.clone()],
            input_ids: BTreeMap::new(),
            poisoned: false,
            staged_source: None,
        };
        write_atomic_file_in_dir(
            &part_dir,
            REPLACE_INTENT_FILE,
            &serde_json::to_vec(&bad_rep).unwrap(),
        )
        .unwrap();
        let rep_err = reconcile_partition_intent(catalog.layout(), &part_dir)
            .expect_err("invalid replacement filename must fail");
        assert!(rep_err.contains("invalid"), "unexpected err: {rep_err}");

        let oversize_file = part_dir.join(REPLACE_INTENT_FILE);
        let f = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&oversize_file)
            .unwrap();
        f.set_len(MAX_REPLACE_INTENT_BYTES + 1).unwrap();
        drop(f);
        let size_err = reconcile_partition_intent(catalog.layout(), &part_dir)
            .expect_err("oversized replace intent must be rejected");
        assert!(size_err.contains("exceeds"), "unexpected err: {size_err}");
    }

    #[test]
    fn layout_subdir_and_olap_domains_reject_parent_traversal_and_post_open_root_symlink() {
        let dir = non_tmp_dir();
        let outside = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());
        let day = day_from_ms(1_700_000_000_000_i64);

        let escape_subdir = catalog.layout().data_dir().join("olap/../../outside");
        let subdir_err = ensure_durable_layout_subdir(catalog.layout(), &escape_subdir)
            .expect_err("parent traversal subdir must fail");
        assert!(
            subdir_err.contains("non-normalized"),
            "unexpected err: {subdir_err}"
        );

        let existing = olap
            .write_metrics(&[MetricPoint {
                timestamp_ms: 1_700_000_000_000_i64,
                name: "domain.check".into(),
                value: 1.0,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "u".into(),
                pid: None,
                labels_json: "{}".into(),
            }])
            .unwrap();
        let batch = metrics_to_record_batch(&[MetricPoint {
            timestamp_ms: 1_700_000_000_000_i64,
            name: "domain.check".into(),
            value: 2.0,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        }])
        .unwrap();
        for bad_domain in ["metrics/../metrics", "../../outside", "bad/domain", ""] {
            assert!(
                olap.write_batch_to_partition(bad_domain, day, &batch)
                    .is_err()
            );
            assert!(
                olap.compact_parquet_files(
                    bad_domain,
                    day,
                    &[existing[0].clone(), existing[0].clone()]
                )
                .is_err()
            );
            assert!(olap.list_parquet_files(bad_domain, None).is_err());
        }

        // Replace data_dir itself with a symlink after OlapParquetStore construction.
        let mut root_sub = non_tmp_dir();
        let sub_data = root_sub.path().join("data");
        let sub_catalog = crate::CatalogStore::open(&sub_data).unwrap();
        let sub_olap = OlapParquetStore::new(sub_catalog.layout().clone());
        drop(sub_catalog);
        fs::remove_dir_all(&sub_data).unwrap();
        symlink(outside.path(), &sub_data).unwrap();
        let list_err = sub_olap
            .list_parquet_files("metrics", None)
            .expect_err("list_parquet_files must reject post-open symlinked data_dir");
        assert!(list_err.contains("symlink"), "unexpected err: {list_err}");
        let _ = &mut root_sub;
    }

    #[test]
    fn crashed_writer_uncommitted_spindle_parquet_rolls_back_until_watermark_commits() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());

        let pt = |ts: i64, name: &str| MetricPoint {
            timestamp_ms: ts,
            name: name.into(),
            value: 1.0,
            host: "h".into(),
            slice: "spindle".into(),
            unit: "svc".into(),
            pid: None,
            labels_json: "{}".into(),
        };

        // 1. Writer 1 publishes Parquet for Spindle watermark 15 then crashes before watermark commit.
        let writer1 = try_acquire_writer_lock(&layout).unwrap();
        let mut plan1 = olap
            .plan_flush(&[pt(1_700_000_000_000, "spindle.w15")], &[], &[], &[], &[])
            .unwrap()
            .with_spindle_watermark(Some(15));
        olap.execute_flush_plan(&mut plan1).unwrap();
        assert_eq!(olap.list_parquet_files("metrics", None).unwrap().len(), 1);
        drop((writer1, plan1));

        // 2. Writer 2 starts up, rolls back uncommitted wm=15 Parquet, replays batch 15 cleanly.
        let writer2 = try_acquire_writer_lock(&layout).unwrap();
        assert!(
            olap.list_parquet_files("metrics", None).unwrap().is_empty(),
            "uncommitted Spindle parquet must roll back on recovery before watermark commit"
        );
        let mut replay_plan15 = olap
            .plan_flush(&[pt(1_700_000_000_000, "spindle.w15")], &[], &[], &[], &[])
            .unwrap()
            .with_spindle_watermark(Some(15));
        olap.execute_flush_plan(&mut replay_plan15).unwrap();
        catalog.set_watermark("spindle_watermark_id", 15).unwrap();
        clear_committed_uncommitted(&layout, Some(15), &BTreeSet::new()).unwrap();
        drop(replay_plan15);
        assert_eq!(olap.list_parquet_files("metrics", None).unwrap().len(), 1);

        // 3. Writer 2 commits watermark 25 in catalog.redb, then crashes before clearing marker.
        let mut plan25 = olap
            .plan_flush(&[pt(1_700_086_400_000, "spindle.w25")], &[], &[], &[], &[])
            .unwrap()
            .with_spindle_watermark(Some(25));
        olap.execute_flush_plan(&mut plan25).unwrap();
        catalog.set_watermark("spindle_watermark_id", 25).unwrap();
        drop((writer2, plan25));

        // 4. Writer 3 starts up: sees committed_wm (25) >= 25 so preserves both Parquet files
        // while cleaning stale .uncommitted-partitions.json.
        let _writer3 = try_acquire_writer_lock(&layout).unwrap();
        assert_eq!(olap.list_parquet_files("metrics", None).unwrap().len(), 2);
        assert!(
            !layout
                .state_dir()
                .join(UNCOMMITTED_PARTITIONS_FILE)
                .exists(),
            "stale uncommitted partitions marker must be removed on recovery"
        );

        // 5. Even if an earlier recovery attempt unlinked the uncommitted Parquet file before sync,
        // a retry still completes cleanly, syncs the partition directory, and removes the marker.
        drop(_writer3);
        let writer4 = try_acquire_writer_lock(&layout).unwrap();
        let mut plan30 = olap
            .plan_flush(&[pt(1_700_172_800_000, "spindle.w30")], &[], &[], &[], &[])
            .unwrap()
            .with_spindle_watermark(Some(30));
        olap.execute_flush_plan(&mut plan30).unwrap();
        let day3_files = olap.list_parquet_files("metrics", None).unwrap();
        let day3_part = day3_files
            .iter()
            .find(|p| {
                p.parent()
                    .and_then(|d| d.file_name())
                    .and_then(|s| s.to_str())
                    == Some("dt=2023-11-16")
            })
            .expect("day3 uncommitted parquet present")
            .clone();
        fs::remove_file(&day3_part).expect("simulate already-unlinked parquet on retry");
        drop((writer4, plan30));

        let _writer5 = try_acquire_writer_lock(&layout).unwrap();
        assert!(
            !layout
                .state_dir()
                .join(UNCOMMITTED_PARTITIONS_FILE)
                .exists(),
            "retry after pre-unlink crash must clear uncommitted partitions marker"
        );
    }

    #[test]
    fn bounded_reader_rejects_fifo_nonblocking_and_compaction_rejects_foreign_inputs() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());

        #[cfg(target_os = "linux")]
        {
            let fifo_path = dir.path().join("stall.fifo");
            let status = std::process::Command::new("mkfifo")
                .arg(&fifo_path)
                .status()
                .expect("spawn mkfifo");
            assert!(status.success(), "mkfifo succeeded");
            let err = read_non_symlink_bytes_bounded_if_present(&fifo_path, "test fifo", 1024)
                .expect_err("FIFO must fail immediately without blocking");
            assert!(err.contains("non-regular"), "unexpected err: {err}");
        }

        let day1_ms = 1_700_000_000_000_i64; // 2023-11-14
        let day2_ms = 1_700_086_400_000_i64; // 2023-11-15
        let day1 = day_from_ms(day1_ms);
        let mk_pt = |ts: i64, name: &str| MetricPoint {
            timestamp_ms: ts,
            name: name.into(),
            value: 1.0,
            host: "h".into(),
            slice: "spindle".into(),
            unit: "svc".into(),
            pid: None,
            labels_json: "{}".into(),
        };
        let f1 = olap.write_metrics(&[mk_pt(day1_ms, "m1")]).unwrap()[0].clone();
        let f2 = olap.write_metrics(&[mk_pt(day2_ms, "m2")]).unwrap()[0].clone();

        let err = olap
            .compact_parquet_files("metrics", day1, &[f1.clone(), f2.clone()])
            .expect_err("cross-partition compaction inputs must be rejected");
        assert!(
            err.contains("outside target partition"),
            "unexpected err: {err}"
        );
        assert!(f1.is_file(), "day1 file untouched");
        assert!(f2.is_file(), "day2 file untouched");
    }

    #[test]
    fn lockfile_staging_and_multi_day_snapshot_stay_fd_bounded_and_confined() {
        let dir = non_tmp_dir();
        let outside = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());

        // 1. Replacing daemon.lock or .olap-maintenance.lock with a symlink or FIFO must fail cleanly without blocking.
        let daemon_lock = layout.state_dir().join("daemon.lock");
        let outside_lock = outside.path().join("external.lock");
        fs::write(&outside_lock, b"").unwrap();
        let _ = fs::remove_file(&daemon_lock);
        symlink(&outside_lock, &daemon_lock).unwrap();
        let sym_err =
            try_acquire_writer_lock(&layout).expect_err("symlinked daemon.lock must be refused");
        assert!(sym_err.contains("symlink"), "unexpected err: {sym_err}");
        fs::remove_file(&daemon_lock).unwrap();

        #[cfg(target_os = "linux")]
        {
            let status = std::process::Command::new("mkfifo")
                .arg(&daemon_lock)
                .status()
                .expect("spawn mkfifo");
            assert!(status.success(), "mkfifo succeeded");
            let fifo_err = try_acquire_writer_lock(&layout)
                .expect_err("FIFO daemon.lock must be rejected without blocking");
            assert!(
                fifo_err.contains("non-regular"),
                "unexpected err: {fifo_err}"
            );
            fs::remove_file(&daemon_lock).unwrap();
        }

        let maint_lock = layout.state_dir().join(".olap-maintenance.lock");
        let _ = fs::remove_file(&maint_lock);
        symlink(&outside_lock, &maint_lock).unwrap();
        let maint_err = acquire_maintenance_lock(&layout)
            .expect_err("symlinked .olap-maintenance.lock must be refused");
        assert!(maint_err.contains("symlink"), "unexpected err: {maint_err}");
        fs::remove_file(&maint_lock).unwrap();

        // 2. Writing 75 distinct day partitions and retaining their PinnedParquetFile snapshot uses O(1) FDs.
        let base_day_ms = 1_700_000_000_000_i64;
        for d in 0..75_i64 {
            let ts = base_day_ms + d * 86_400_000_i64;
            olap.write_metrics(&[MetricPoint {
                timestamp_ms: ts,
                name: format!("m.day.{d}"),
                value: d as f64,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: None,
                labels_json: "{}".into(),
            }])
            .unwrap();
        }

        let fd_count = || fs::read_dir("/proc/self/fd").unwrap().count();
        let fds_before = fd_count();
        let pinned = olap.snapshot_pinned_parquet_files("metrics", None).unwrap();
        let fds_after = fd_count();
        assert_eq!(pinned.len(), 75);
        assert!(
            fds_after.saturating_sub(fds_before) < 16,
            "pinned snapshot across 75 partitions retained {} extra FDs (before={fds_before}, after={fds_after})",
            fds_after.saturating_sub(fds_before)
        );
        for item in pinned.iter().take(5) {
            let _f = item.open_verified().unwrap();
        }
        assert!(dir_disk_bytes(&layout.olap_dir()).unwrap() > 0);
    }

    #[test]
    fn write_planned_partition_republishes_when_missing_without_compaction_or_partition_recreated()
    {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let olap = OlapParquetStore::new(catalog.layout().clone());

        let day_ms = 1_700_000_000_000_i64; // 2023-11-14
        let pt = MetricPoint {
            timestamp_ms: day_ms,
            name: "m.republish".into(),
            value: 9.0,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        };

        let maint = acquire_maintenance_lock(catalog.layout()).unwrap();
        let mut plan = olap
            .plan_flush(std::slice::from_ref(&pt), &[], &[], &[], &[])
            .unwrap();
        let mut item = plan.pending.pop_front().unwrap();
        let published = olap.write_planned_partition(&mut item, &maint).unwrap();
        assert!(published.is_file());
        assert!(item.published.is_some());

        // Case 1: File removed externally without advancing .replace-epoch -> must republish.
        fs::remove_file(&published).unwrap();
        let republished = olap.write_planned_partition(&mut item, &maint).unwrap();
        assert_eq!(republished, published);
        assert!(
            republished.is_file(),
            "missing file without compaction must be republished"
        );

        // Case 2: Partition evicted and recreated (even if .replace-epoch is artificially bumped)
        // -> must republish because partition dir (dev, ino) changed.
        let part_dir = published.parent().unwrap().to_path_buf();
        fs::remove_dir_all(&part_dir).unwrap();
        fs::create_dir_all(&part_dir).unwrap();
        write_atomic_file_in_dir(&part_dir, REPLACE_EPOCH_FILE, b"5").unwrap();
        let recreated = olap.write_planned_partition(&mut item, &maint).unwrap();
        assert_eq!(recreated, published);
        assert!(
            recreated.is_file(),
            "recreated partition dir must force republish"
        );

        // Case 3: Sticky file disappears and UNRELATED files compact (advancing .replace-epoch),
        // without the sticky file in .replaced-inputs.json -> must republish instead of losing data.
        drop(maint);
        fs::remove_file(&recreated).unwrap();
        let u1 = olap
            .write_metrics(std::slice::from_ref(&pt))
            .unwrap()
            .pop()
            .unwrap();
        let u2 = olap.write_metrics(&[pt]).unwrap().pop().unwrap();
        let _compacted = olap
            .compact_parquet_files(item.domain, item.day, &[u1, u2])
            .unwrap()
            .unwrap();
        let maint = acquire_maintenance_lock(catalog.layout()).unwrap();
        let republished_after_unrelated = olap.write_planned_partition(&mut item, &maint).unwrap();
        assert_eq!(republished_after_unrelated, published);
        assert!(
            republished_after_unrelated.is_file(),
            "unrelated epoch advance without per-file replacement record must force republish"
        );
    }

    #[test]
    fn atomic_write_already_exists_preserves_foreign_tmp_and_intent_enforces_identities() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());

        // 1. Pre-existing temp name colliding on create_new(true) must remain untouched.
        let dir_fd = open_standalone_dir_nofollow(dir.path(), "test dir")
            .unwrap()
            .unwrap();
        let colliding_tmp = format!("target.json.tmp.{}.999001", std::process::id());
        let colliding_path = dir.path().join(&colliding_tmp);
        fs::write(&colliding_path, b"foreign-temp-owner").unwrap();
        let create_err =
            write_atomic_with_in_dir_fd_seq_pinned(&dir_fd, "target.json", 999_001, |f, _| {
                f.write_all(b"new-payload").map_err(|e| e.to_string())
            })
            .expect_err("colliding create_new temp must fail");
        assert!(
            create_err.contains("failed creating"),
            "unexpected: {create_err}"
        );
        assert_eq!(
            fs::read(&colliding_path).unwrap(),
            b"foreign-temp-owner",
            "failed create_new must never unlink foreign temp file"
        );

        // 2. ReplaceIntent with mismatched replacement_id must NOT delete inputs.
        let day_ms = 1_700_000_000_000_i64;
        let day = day_from_ms(day_ms);
        let mk_pt = |name: &str| MetricPoint {
            timestamp_ms: day_ms,
            name: name.into(),
            value: 1.0,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        };
        let input1 = olap.write_metrics(&[mk_pt("m1")]).unwrap().pop().unwrap();
        let input2 = olap.write_metrics(&[mk_pt("m2")]).unwrap().pop().unwrap();
        let fake_rep = olap.write_metrics(&[mk_pt("rep")]).unwrap().pop().unwrap();
        let part_dir = layout
            .olap_domain_dir("metrics")
            .join(format!("dt={}", day.format("%Y-%m-%d")));
        let input1_meta = fs::symlink_metadata(&input1).unwrap();
        let input2_meta = fs::symlink_metadata(&input2).unwrap();
        let fake_meta = fs::symlink_metadata(&fake_rep).unwrap();
        let input1_name = input1.file_name().unwrap().to_string_lossy().into_owned();
        let input2_name = input2.file_name().unwrap().to_string_lossy().into_owned();
        let fake_name = fake_rep.file_name().unwrap().to_string_lossy().into_owned();

        let input1_sha = sha256_regular_file(&File::open(&input1).unwrap())
            .unwrap()
            .1;
        let fake_sha = sha256_regular_file(&File::open(&fake_rep).unwrap())
            .unwrap()
            .1;

        let mismatch_rep_intent = ReplaceIntent {
            replacement: fake_name.clone(),
            replacement_id: Some(FileIdentity {
                dev: fake_meta.dev(),
                ino: fake_meta.ino().saturating_add(10_000),
                len: Some(fake_meta.len()),
                sha256: Some(fake_sha.clone()),
            }),
            inputs: vec![input1_name.clone()],
            input_ids: BTreeMap::from([(
                input1_name.clone(),
                FileIdentity {
                    dev: input1_meta.dev(),
                    ino: input1_meta.ino(),
                    len: Some(input1_meta.len()),
                    sha256: Some(input1_sha.clone()),
                },
            )]),
            poisoned: false,
            staged_source: None,
        };
        write_atomic_file_in_dir(
            &part_dir,
            REPLACE_INTENT_FILE,
            &serde_json::to_vec(&mismatch_rep_intent).unwrap(),
        )
        .unwrap();
        let foreign_err = reconcile_partition_intent(&layout, &part_dir)
            .expect_err("foreign-inode replacement_id must fail closed and preserve intent");
        assert!(
            foreign_err.contains("identity") || foreign_err.contains("verification"),
            "unexpected foreign replacement err: {foreign_err}"
        );
        assert!(
            part_dir.join(REPLACE_INTENT_FILE).is_file(),
            ".replace-intent.json must stay intact after foreign replacement inode"
        );
        assert!(
            input1.is_file(),
            "mismatched replacement_id must abort deletion of original inputs"
        );
        assert!(
            fake_rep.is_file(),
            "foreign file at replacement name must stay intact"
        );
        assert!(
            olap.snapshot_pinned_parquet_files("metrics", None).is_err(),
            "snapshot must keep failing closed on foreign replacement inode"
        );

        // 2b. ReplaceIntent with same dev/ino/len but mismatched sha256 must fail closed and
        // preserve `.replace-intent.json` across both reconciliation and post-reconcile snapshots.
        let wrong_sha_intent = ReplaceIntent {
            replacement: fake_name.clone(),
            replacement_id: Some(FileIdentity {
                dev: fake_meta.dev(),
                ino: fake_meta.ino(),
                len: Some(fake_meta.len()),
                sha256: Some("0".repeat(64)),
            }),
            inputs: vec![input1_name.clone()],
            input_ids: BTreeMap::from([(
                input1_name.clone(),
                FileIdentity {
                    dev: input1_meta.dev(),
                    ino: input1_meta.ino(),
                    len: Some(input1_meta.len()),
                    sha256: Some(input1_sha.clone()),
                },
            )]),
            poisoned: false,
            staged_source: None,
        };
        write_atomic_file_in_dir(
            &part_dir,
            REPLACE_INTENT_FILE,
            &serde_json::to_vec(&wrong_sha_intent).unwrap(),
        )
        .unwrap();
        let snap_err = olap
            .snapshot_pinned_parquet_files("metrics", None)
            .expect_err("snapshot must fail closed on replacement sha256 mismatch");
        assert!(
            snap_err.contains("sha256") || snap_err.contains("digest"),
            "unexpected snapshot mismatch err: {snap_err}"
        );
        let rec_rep_err = reconcile_partition_intent(&layout, &part_dir).expect_err(
            "reconciliation must fail closed on same-inode replacement sha256 mismatch",
        );
        assert!(
            rec_rep_err.contains("digest") || rec_rep_err.contains("verification"),
            "unexpected reconcile replacement err: {rec_rep_err}"
        );
        assert!(
            part_dir.join(REPLACE_INTENT_FILE).is_file(),
            ".replace-intent.json must stay intact after same-inode replacement digest mismatch"
        );
        assert!(
            olap.snapshot_pinned_parquet_files("metrics", None).is_err(),
            "post-reconciliation snapshot must continue failing closed"
        );
        assert!(
            input1.is_file(),
            "mismatched replacement sha256 must abort deletion of original inputs"
        );

        // 3. Valid replacement_id deletes matching input1 while preserving foreign-inode input2
        // and keeping `.replace-intent.json` intact so snapshots continue failing closed.
        let id_enforced_intent = ReplaceIntent {
            replacement: fake_name.clone(),
            replacement_id: Some(FileIdentity {
                dev: fake_meta.dev(),
                ino: fake_meta.ino(),
                len: Some(fake_meta.len()),
                sha256: Some(fake_sha.clone()),
            }),
            inputs: vec![input1_name.clone(), input2_name.clone()],
            input_ids: BTreeMap::from([
                (
                    input1_name,
                    FileIdentity {
                        dev: input1_meta.dev(),
                        ino: input1_meta.ino(),
                        len: Some(input1_meta.len()),
                        sha256: Some(input1_sha),
                    },
                ),
                (
                    input2_name.clone(),
                    FileIdentity {
                        dev: input2_meta.dev(),
                        ino: input2_meta.ino().saturating_add(10_000),
                        len: Some(input2_meta.len()),
                        sha256: None,
                    },
                ),
            ]),
            poisoned: false,
            staged_source: None,
        };
        write_atomic_file_in_dir(
            &part_dir,
            REPLACE_INTENT_FILE,
            &serde_json::to_vec(&id_enforced_intent).unwrap(),
        )
        .unwrap();
        let rec_in_err = reconcile_partition_intent(&layout, &part_dir)
            .expect_err("reconciliation must fail when foreign input blocks cleanup");
        assert!(
            rec_in_err.contains("identity") || rec_in_err.contains("digest"),
            "unexpected reconcile input err: {rec_in_err}"
        );
        assert!(
            !input1.exists(),
            "identity-matching superseded input must be unlinked"
        );
        assert!(
            input2.is_file(),
            "substituted foreign-inode file at input name must NOT be deleted"
        );
        assert!(
            part_dir.join(REPLACE_INTENT_FILE).is_file(),
            "intent must remain until identity-mismatched input is resolved"
        );
        let kept_with_foreign = olap.snapshot_pinned_parquet_files("metrics", None).unwrap();
        assert_eq!(
            kept_with_foreign.len(),
            2,
            "foreign-inode file at superseded input name stays visible alongside replacement"
        );

        // 3b. Same-inode SHA-256 mismatch on superseded input must also preserve input and intent.
        let wrong_input_sha_intent = ReplaceIntent {
            replacement: fake_name,
            replacement_id: Some(FileIdentity {
                dev: fake_meta.dev(),
                ino: fake_meta.ino(),
                len: Some(fake_meta.len()),
                sha256: Some(fake_sha),
            }),
            inputs: vec![input2_name.clone()],
            input_ids: BTreeMap::from([(
                input2_name,
                FileIdentity {
                    dev: input2_meta.dev(),
                    ino: input2_meta.ino(),
                    len: Some(input2_meta.len()),
                    sha256: Some("0".repeat(64)),
                },
            )]),
            poisoned: false,
            staged_source: None,
        };
        write_atomic_file_in_dir(
            &part_dir,
            REPLACE_INTENT_FILE,
            &serde_json::to_vec(&wrong_input_sha_intent).unwrap(),
        )
        .unwrap();
        let input_sha_err = reconcile_partition_intent(&layout, &part_dir)
            .expect_err("reconciliation must fail closed on input sha256 mismatch");
        assert!(
            input_sha_err.contains("digest") || input_sha_err.contains("verification"),
            "unexpected input sha256 err: {input_sha_err}"
        );
        assert!(
            input2.is_file(),
            "same-inode digest-mismatched input must NOT be unlinked"
        );
        assert!(part_dir.join(REPLACE_INTENT_FILE).is_file());
        assert!(olap.snapshot_pinned_parquet_files("metrics", None).is_err());
    }

    #[test]
    fn hashing_writer_enforces_cap_before_forwarding_and_preserves_io_errors() {
        let mut writer = HashingWriter::new(Vec::new(), Some(4), Arc::new(AtomicBool::new(false)));
        writer.write_all(b"abcd").unwrap();
        let err = writer.write_all(b"e").unwrap_err();
        assert!(
            err.get_ref()
                .unwrap()
                .downcast_ref::<StagingLimit>()
                .is_some()
        );
        let (bytes, len, _) = writer.finish();
        assert_eq!(bytes, b"abcd");
        assert_eq!(len, 4);
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(ErrorKind::BrokenPipe, "disk failure"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let err = HashingWriter::new(Broken, Some(4), Arc::new(AtomicBool::new(false)))
            .write_all(b"a")
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::BrokenPipe);
    }

    #[test]
    fn bounded_staging_reserves_same_inode_and_cleans_allocation_failures() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());
        let point = MetricPoint {
            timestamp_ms: 1_700_000_000_000,
            name: "reserve".into(),
            value: 1.0,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        };
        let source = olap
            .write_metrics(std::slice::from_ref(&point))
            .unwrap()
            .pop()
            .unwrap();
        let before = sha256_regular_file(&File::open(&source).unwrap()).unwrap();
        let day = day_from_ms(point.timestamp_ms);
        let batch = metrics_to_record_batch(&[point]).unwrap();
        let observed = std::cell::Cell::new(None);
        let result = olap
            .write_batches_to_staging_with_allocator(
                "metrics",
                day,
                None,
                std::iter::once(Ok(batch.clone())),
                true,
                Some(1024 * 1024),
                |file, len| {
                    rustix::fs::fallocate(file, rustix::fs::FallocateFlags::empty(), 0, len)
                        .map_err(std::io::Error::from)?;
                    let meta = file.metadata()?;
                    assert_eq!(meta.len(), len);
                    assert!(meta.blocks() * 512 >= len);
                    observed.set(Some((meta.dev(), meta.ino())));
                    Ok(())
                },
            )
            .unwrap();
        let StagingOutcome::Staged(staged) = result else {
            panic!("staging failed")
        };
        let path = layout.staging_dir().join(&staged.staging_name);
        let meta = fs::metadata(&path).unwrap();
        assert_eq!(observed.get(), Some((meta.dev(), meta.ino())));
        assert_eq!(meta.len(), staged.len);
        assert!(meta.len() < 1024 * 1024);
        ParquetRecordBatchReaderBuilder::try_new(File::open(&path).unwrap())
            .unwrap()
            .build()
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        drop(staged);
        for errno in [28, 95] {
            let result = olap.write_batches_to_staging_with_allocator(
                "metrics",
                day,
                None,
                std::iter::once(Ok(batch.clone())),
                true,
                Some(1024),
                |_, _| Err(std::io::Error::from_raw_os_error(errno)),
            );
            assert!(
                matches!(result, Ok(StagingOutcome::ScratchRefused(error)) if error.contains("staging allocation failed"))
            );
            assert_eq!(fs::read_dir(layout.staging_dir()).unwrap().count(), 0);
            assert_eq!(
                sha256_regular_file(&File::open(&source).unwrap()).unwrap(),
                before
            );
        }
    }

    #[test]
    fn capped_staging_rejects_footer_and_preserves_existing_source() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());
        let point = MetricPoint {
            timestamp_ms: 1_700_000_000_000,
            name: "limit".into(),
            value: 1.0,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        };
        let source = olap
            .write_metrics(std::slice::from_ref(&point))
            .unwrap()
            .pop()
            .unwrap();
        let source_hash = sha256_regular_file(&File::open(&source).unwrap()).unwrap();
        let day = day_from_ms(point.timestamp_ms);
        let batch = metrics_to_record_batch(&[point]).unwrap();
        let stage = |cap| {
            olap.write_batches_to_staging_bounded(
                "metrics",
                day,
                None,
                std::iter::once(Ok(batch.clone())),
                true,
                cap,
            )
            .unwrap()
        };
        let StagingOutcome::Staged(unbounded) = stage(None) else {
            panic!("expected staging")
        };
        let len = unbounded.len;
        let digest = unbounded.sha256.clone();
        drop(unbounded);
        assert!(matches!(
            stage(Some(len - 1)),
            StagingOutcome::ExceededMaxBytes
        ));
        let pinned = olap.snapshot_pinned_parquet_files("metrics", None).unwrap();
        let maint = acquire_maintenance_lock(&layout).unwrap();
        let outcome = olap
            .replace_batches_in_partition_with_validated_capped(
                "metrics",
                day,
                None,
                &pinned,
                None,
                &[],
                std::iter::once(Ok(batch.clone())),
                Some(len - 1),
                &maint,
            )
            .unwrap();
        assert!(matches!(outcome, ReplaceOutcome::ExceededMaxBytes));
        drop(maint);
        drop(pinned);
        assert_eq!(fs::read_dir(layout.staging_dir()).unwrap().count(), 0);
        assert_eq!(
            sha256_regular_file(&File::open(&source).unwrap()).unwrap(),
            source_hash
        );
        let StagingOutcome::Staged(exact) = stage(Some(len)) else {
            panic!("exact cap rejected")
        };
        assert_eq!((exact.len, &exact.sha256), (len, &digest));
        drop(exact);
        assert_eq!(fs::read_dir(layout.staging_dir()).unwrap().count(), 0);
    }

    #[test]
    fn sha256_matches_nist_abc_vector() {
        let mut hasher = Sha256::new();
        hasher.update(b"abc");
        assert_eq!(
            hasher.finish_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn staged_parquet_drop_cleans_up_and_iterative_compaction_handles_high_fan_in() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());

        let day_ms = 1_700_000_000_000_i64;
        let day = day_from_ms(day_ms);
        let mk_pt = |idx: i64| MetricPoint {
            timestamp_ms: day_ms + idx,
            name: format!("fanin.{idx}"),
            value: idx as f64,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        };

        // 1. Dropping an unpublished StagedParquetFile unlinks the staging temp file cleanly.
        let batch = metrics_to_record_batch(&[mk_pt(0)]).unwrap();
        let staged = olap
            .write_batches_to_staging("metrics", day, None, std::iter::once(Ok(batch)), true)
            .unwrap()
            .expect("staged parquet");
        assert!(layout.staging_dir().join(&staged.staging_name).is_file());
        drop(staged);
        let staging_leftovers: Vec<_> = fs::read_dir(layout.staging_dir())
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(
            staging_leftovers.is_empty(),
            "dropping unpublished StagedParquetFile must clean staging dir"
        );

        // 2. PostRenameSync failure in publish leaves ReplaceIntent for reconcile_partition_intent
        //    to sync part_fd before unlinking inputs.
        let a_path = olap.write_metrics(&[mk_pt(100)]).unwrap().pop().unwrap();
        let a_name = a_path.file_name().unwrap().to_string_lossy().into_owned();
        let a_meta = fs::symlink_metadata(&a_path).unwrap();
        let a_sha = sha256_regular_file(&File::open(&a_path).unwrap())
            .unwrap()
            .1;
        let rep_batch = metrics_to_record_batch(&[mk_pt(101)]).unwrap();
        let mut rep_staged = olap
            .write_batches_to_staging("metrics", day, None, std::iter::once(Ok(rep_batch)), true)
            .unwrap()
            .expect("rep staged");
        let part_dir = layout
            .olap_domain_dir("metrics")
            .join(format!("dt={}", day.format("%Y-%m-%d")));
        let part_fd = open_olap_partition_dir_nofollow(&layout, "metrics", day)
            .unwrap()
            .unwrap();
        let intent = ReplaceIntent {
            replacement: rep_staged.final_name.clone(),
            replacement_id: Some(rep_staged.file_identity()),
            inputs: vec![a_name.clone()],
            input_ids: BTreeMap::from([(
                a_name,
                FileIdentity {
                    dev: a_meta.dev(),
                    ino: a_meta.ino(),
                    len: Some(a_meta.len()),
                    sha256: Some(a_sha),
                },
            )]),
            poisoned: false,
            staged_source: None,
        };
        write_atomic_json_in_dir_fd_bounded(
            &part_fd,
            REPLACE_INTENT_FILE,
            &intent,
            MAX_REPLACE_INTENT_BYTES,
        )
        .unwrap();
        let sync_res = OlapParquetStore::publish_staging_to_partition_on_fd_with_hooks(
            &part_fd,
            &mut rep_staged,
            |_| Err("injected staging dir sync failure".to_owned()),
        );
        assert!(matches!(
            sync_res,
            Err(PublishStageError::PostRenameSync(_))
        ));
        assert!(
            a_path.is_file(),
            "PostRenameSync failure must defer input removal until reconciliation"
        );
        assert!(
            part_dir.join(REPLACE_INTENT_FILE).is_file(),
            "replace intent must remain for recovery"
        );
        reconcile_partition_intent(&layout, &part_dir).unwrap();
        assert!(!a_path.exists());
        assert!(!part_dir.join(REPLACE_INTENT_FILE).exists());

        // 3. Compacting 20 files exceeds MAX_REPLACE_FAN_IN_FILES (16) and folds cleanly.
        let mut inputs = olap.list_parquet_files("metrics", None).unwrap();
        for idx in 1..20 {
            inputs.extend(olap.write_metrics(&[mk_pt(200 + idx)]).unwrap());
        }
        assert_eq!(inputs.len(), 20);
        let folded = olap
            .compact_parquet_files("metrics", day, &inputs)
            .unwrap()
            .expect("folded compaction output");
        assert!(folded.is_file());
        assert_eq!(
            olap.list_parquet_files("metrics", None).unwrap(),
            vec![folded]
        );
    }

    #[test]
    fn standalone_flush_plan_holds_uncommitted_lock_until_retry_completes() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());

        let day1_ms = 1_690_000_000_000_i64; // 2023-07-20
        let day2_ms = 1_700_000_000_000_i64; // 2023-11-14
        let day1 = day_from_ms(day1_ms);
        let mk_pt = |ts: i64, name: &str| MetricPoint {
            timestamp_ms: ts,
            name: name.into(),
            value: 1.0,
            host: "h".into(),
            slice: "spindle".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        };

        // Pre-seed two files on day1 so compaction would merge them unless protected.
        let d1_a = olap
            .write_metrics(&[mk_pt(day1_ms, "seed.a")])
            .unwrap()
            .pop()
            .unwrap();
        let d1_b = olap
            .write_metrics(&[mk_pt(day1_ms + 1, "seed.b")])
            .unwrap()
            .pop()
            .unwrap();

        let mut plan = olap
            .plan_flush(
                &[mk_pt(day1_ms + 2, "plan.d1"), mk_pt(day2_ms, "plan.d2")],
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let day2_dir = layout.olap_domain_dir("metrics").join("dt=2023-11-14");
        fs::write(&day2_dir, b"obstruct-day2").unwrap();
        assert!(
            olap.execute_flush_plan(&mut plan).is_err(),
            "day2 obstruction should fail mid-plan after day1 writes"
        );
        assert!(
            plan.uncommitted_lock.is_some(),
            "standalone failed plan must retain shared .uncommitted.lock until retry completes"
        );

        // Concurrent compaction on day1 must skip because standalone plan protects day1.
        assert_eq!(
            olap.compact_parquet_files("metrics", day1, &[d1_a.clone(), d1_b.clone()])
                .unwrap(),
            None
        );

        // Removing obstruction and completing retry releases .uncommitted.lock, allowing compaction.
        fs::remove_file(&day2_dir).unwrap();
        olap.execute_flush_plan(&mut plan).unwrap();
        assert!(plan.uncommitted_lock.is_none());
        let day1_dir_name = format!("dt={}", day1.format("%Y-%m-%d"));
        let day1_files: Vec<PathBuf> = olap
            .list_parquet_files("metrics", None)
            .unwrap()
            .into_iter()
            .filter(|p| {
                p.parent().and_then(|d| d.file_name()) == Some(std::ffi::OsStr::new(&day1_dir_name))
            })
            .collect();
        assert_eq!(day1_files.len(), 3);
        let merged = olap
            .compact_parquet_files("metrics", day1, &day1_files)
            .unwrap()
            .expect("compaction proceeds after standalone plan finishes");
        assert!(merged.is_file());
    }

    #[test]
    fn compaction_respects_max_file_rows_cap_and_staging_checks_footer_size() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());

        let day_ms = 1_700_000_000_000_i64;
        let day = day_from_ms(day_ms);
        let mk_pts = |start: i64, count: i64| -> Vec<MetricPoint> {
            (0..count)
                .map(|i| MetricPoint {
                    timestamp_ms: day_ms + start + i,
                    name: format!("cap.{start}.{i}"),
                    value: i as f64,
                    host: "h".into(),
                    slice: "agent.slice".into(),
                    unit: "u".into(),
                    pid: None,
                    labels_json: "{}".into(),
                })
                .collect()
        };

        let f1 = olap.write_metrics(&mk_pts(0, 2)).unwrap().pop().unwrap();
        let f2 = olap.write_metrics(&mk_pts(10, 2)).unwrap().pop().unwrap();
        let f3 = olap.write_metrics(&mk_pts(20, 1)).unwrap().pop().unwrap();

        let maint = acquire_maintenance_lock(&layout).unwrap();
        let out = olap
            .compact_parquet_files_unlocked_with_max_rows(
                "metrics",
                day,
                &[f1.clone(), f2.clone(), f3.clone()],
                3,
                &maint,
            )
            .unwrap()
            .expect("second and third files (2 + 1 <= 3) merge after sealing front 2-row file");
        drop(maint);
        assert!(
            f1.is_file(),
            "sealed front file exceeds cap when paired with next"
        );
        assert!(!f2.exists() && !f3.exists() && out.is_file());
        let remaining = olap.list_parquet_files("metrics", None).unwrap();
        assert_eq!(remaining.len(), 2);
        let mut row_counts: Vec<i64> = remaining
            .iter()
            .map(|p| {
                ParquetMetaDataReader::new()
                    .parse_and_finish(&File::open(p).unwrap())
                    .unwrap()
                    .file_metadata()
                    .num_rows()
            })
            .collect();
        row_counts.sort_unstable();
        assert_eq!(row_counts, vec![2, 3]);
    }

    #[test]
    fn standalone_spindle_plan_blocks_writer_startup_and_reclaim_until_watermark_release() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());

        let day1_ms = 1_690_000_000_000_i64; // 2023-07-20
        let day2_ms = 1_700_000_000_000_i64; // 2023-11-14
        let mk_pt = |ts: i64, name: &str| MetricPoint {
            timestamp_ms: ts,
            name: name.into(),
            value: 1.0,
            host: "h".into(),
            slice: "spindle".into(),
            unit: "u".into(),
            pid: None,
            labels_json: "{}".into(),
        };

        // Fail mid-plan after day1 publishes, while catalog watermark remains below 20.
        let mut plan20 = olap
            .plan_flush(
                &[mk_pt(day1_ms, "sp.d1"), mk_pt(day2_ms, "sp.d2")],
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap()
            .with_spindle_watermark(Some(20));
        let day2_dir = layout.olap_domain_dir("metrics").join("dt=2023-11-14");
        fs::write(&day2_dir, b"obstruct").unwrap();
        assert!(olap.execute_flush_plan(&mut plan20).is_err());

        // Writer startup must fail closed without reclaiming published day1 while plan is live.
        let busy_err = try_acquire_writer_lock(&layout)
            .expect_err("live standalone flush plan lock must block concurrent writer reclaim");
        assert!(
            busy_err.contains(".uncommitted.lock"),
            "unexpected busy lock err: {busy_err}"
        );
        assert_eq!(
            olap.list_parquet_files("metrics", None).unwrap().len(),
            1,
            "day1 published file must not be deleted while standalone plan holds lock"
        );

        // Retry completes day2; watermark-tagged plan STILL holds .uncommitted.lock until release!
        fs::remove_file(&day2_dir).unwrap();
        olap.execute_flush_plan(&mut plan20).unwrap();
        assert!(plan20.uncommitted_lock.is_some());
        let post_ok_busy = try_acquire_writer_lock(&layout)
            .expect_err("watermark-tagged plan must hold .uncommitted.lock until watermark commit");
        assert!(
            post_ok_busy.contains(".uncommitted.lock"),
            "unexpected post-ok busy err: {post_ok_busy}"
        );
        assert_eq!(olap.list_parquet_files("metrics", None).unwrap().len(), 2);

        catalog.set_watermark("spindle_watermark_id", 20).unwrap();
        clear_committed_uncommitted(&layout, Some(20), &BTreeSet::new()).unwrap();
        plan20.release_uncommitted_lock();
        drop(plan20);

        let _writer = try_acquire_writer_lock(&layout)
            .expect("writer startup proceeds once standalone plan releases lock");
        assert_eq!(olap.list_parquet_files("metrics", None).unwrap().len(), 2);
    }

    #[test]
    fn poisoned_and_missing_input_replace_intents_fail_closed_and_compaction_guards_malicious() {
        let dir = non_tmp_dir();
        let catalog = crate::CatalogStore::open(dir.path()).unwrap();
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());

        let day_ms = 1_700_000_000_000_i64;
        let day = day_from_ms(day_ms);
        let mk_pt = |name: &str| MetricPoint {
            timestamp_ms: day_ms,
            name: name.into(),
            value: 1.0,
            host: "h".into(),
            slice: "agent.slice".into(),
            unit: "u".into(),
            pid: None,
            labels_json: format!("{{\"blob\":\"{}\"}}", "z".repeat(4096)),
        };

        let p1 = olap.write_metrics(&[mk_pt("m1")]).unwrap().pop().unwrap();
        let p1_name = p1.file_name().unwrap().to_string_lossy().into_owned();
        let p1_meta = fs::symlink_metadata(&p1).unwrap();
        let p1_sha = sha256_regular_file(&File::open(&p1).unwrap()).unwrap().1;
        let part_dir = layout
            .olap_domain_dir("metrics")
            .join(format!("dt={}", day.format("%Y-%m-%d")));

        // 1. Poisoned intent persists and fails closed across reconcile & snapshot even when
        //    replacement is absent and input exists intact.
        let poisoned = ReplaceIntent {
            replacement: "part-missing-rep.parquet".to_owned(),
            replacement_id: None,
            inputs: vec![p1_name.clone()],
            input_ids: BTreeMap::from([(
                p1_name.clone(),
                FileIdentity {
                    dev: p1_meta.dev(),
                    ino: p1_meta.ino(),
                    len: Some(p1_meta.len()),
                    sha256: Some(p1_sha.clone()),
                },
            )]),
            poisoned: true,
            staged_source: None,
        };
        write_atomic_file_in_dir(
            &part_dir,
            REPLACE_INTENT_FILE,
            &serde_json::to_vec(&poisoned).unwrap(),
        )
        .unwrap();
        assert!(
            reconcile_partition_intent(&layout, &part_dir)
                .unwrap_err()
                .contains("poisoned")
        );
        assert!(
            olap.snapshot_pinned_parquet_files("metrics", None)
                .unwrap_err()
                .contains("poisoned")
        );
        assert!(part_dir.join(REPLACE_INTENT_FILE).is_file());

        // 2a. Absent replacement + missing tracked input fails closed & preserves intent.
        let missing_in = ReplaceIntent {
            replacement: "part-absent-rep.parquet".to_owned(),
            replacement_id: None,
            inputs: vec!["part-gone-input.parquet".to_owned()],
            input_ids: BTreeMap::from([(
                "part-gone-input.parquet".to_owned(),
                FileIdentity {
                    dev: p1_meta.dev(),
                    ino: p1_meta.ino(),
                    len: Some(p1_meta.len()),
                    sha256: Some("0".repeat(64)),
                },
            )]),
            poisoned: false,
            staged_source: None,
        };
        write_atomic_file_in_dir(
            &part_dir,
            REPLACE_INTENT_FILE,
            &serde_json::to_vec(&missing_in).unwrap(),
        )
        .unwrap();
        assert!(reconcile_partition_intent(&layout, &part_dir).is_err());
        assert!(olap.snapshot_pinned_parquet_files("metrics", None).is_err());
        assert!(part_dir.join(REPLACE_INTENT_FILE).is_file());

        // 2b. Legacy ReplaceIntent (empty input_ids, no staged_source) with missing input & absent
        //     replacement also fails closed and preserves `.replace-intent.json`.
        let legacy_missing_in = ReplaceIntent {
            replacement: "part-absent-legacy.parquet".to_owned(),
            replacement_id: None,
            inputs: vec!["part-legacy-gone.parquet".to_owned()],
            input_ids: BTreeMap::new(),
            poisoned: false,
            staged_source: None,
        };
        write_atomic_file_in_dir(
            &part_dir,
            REPLACE_INTENT_FILE,
            &serde_json::to_vec(&legacy_missing_in).unwrap(),
        )
        .unwrap();
        assert!(
            reconcile_partition_intent(&layout, &part_dir)
                .unwrap_err()
                .contains("missing")
        );
        assert!(olap.snapshot_pinned_parquet_files("metrics", None).is_err());
        assert!(part_dir.join(REPLACE_INTENT_FILE).is_file());

        // 2c. Unpoisoned intent with consumed/absent staged_source & absent replacement fails closed
        //     even when inputs exist (durable fail-closed across ENOSPC / crash after rename).
        let consumed_src = ReplaceIntent {
            replacement: "part-absent-rep2.parquet".to_owned(),
            replacement_id: Some(FileIdentity {
                dev: p1_meta.dev(),
                ino: p1_meta.ino().saturating_add(1),
                len: Some(p1_meta.len()),
                sha256: Some(p1_sha.clone()),
            }),
            inputs: vec![p1_name.clone()],
            input_ids: BTreeMap::from([(
                p1_name.clone(),
                FileIdentity {
                    dev: p1_meta.dev(),
                    ino: p1_meta.ino(),
                    len: Some(p1_meta.len()),
                    sha256: Some(p1_sha.clone()),
                },
            )]),
            poisoned: false,
            staged_source: Some(".replace-source-consumed.staged".to_owned()),
        };
        write_atomic_file_in_dir(
            &part_dir,
            REPLACE_INTENT_FILE,
            &serde_json::to_vec(&consumed_src).unwrap(),
        )
        .unwrap();
        assert!(
            reconcile_partition_intent(&layout, &part_dir)
                .unwrap_err()
                .contains("consuming staged source")
        );
        assert!(olap.snapshot_pinned_parquet_files("metrics", None).is_err());
        assert!(part_dir.join(REPLACE_INTENT_FILE).is_file());

        // 2d. Uncommitted intent with live in-partition `.replace-source-*.staged` witness allows
        //     snapshot reads of inputs and rolls back intent + staged witness cleanly.
        let staged_witness_name = ".replace-source-pending.staged";
        let staged_witness_path = part_dir.join(staged_witness_name);
        fs::copy(&p1, &staged_witness_path).unwrap();
        let sw_meta = fs::symlink_metadata(&staged_witness_path).unwrap();
        let sw_sha = sha256_regular_file(&File::open(&staged_witness_path).unwrap())
            .unwrap()
            .1;
        let pre_cutover = ReplaceIntent {
            replacement: "part-pending-rep.parquet".to_owned(),
            replacement_id: Some(FileIdentity {
                dev: sw_meta.dev(),
                ino: sw_meta.ino(),
                len: Some(sw_meta.len()),
                sha256: Some(sw_sha),
            }),
            inputs: vec![p1_name.clone()],
            input_ids: BTreeMap::from([(
                p1_name.clone(),
                FileIdentity {
                    dev: p1_meta.dev(),
                    ino: p1_meta.ino(),
                    len: Some(p1_meta.len()),
                    sha256: Some(p1_sha.clone()),
                },
            )]),
            poisoned: false,
            staged_source: Some(staged_witness_name.to_owned()),
        };
        write_atomic_file_in_dir(
            &part_dir,
            REPLACE_INTENT_FILE,
            &serde_json::to_vec(&pre_cutover).unwrap(),
        )
        .unwrap();
        assert_eq!(
            olap.snapshot_pinned_parquet_files("metrics", None)
                .unwrap()
                .iter()
                .map(|p| p.display_path().to_path_buf())
                .collect::<Vec<_>>(),
            vec![p1.clone()]
        );
        reconcile_partition_intent(&layout, &part_dir).unwrap();
        assert!(!part_dir.join(REPLACE_INTENT_FILE).exists());
        assert!(!staged_witness_path.exists());
        assert!(p1.is_file());

        // 2e. Pre-rename intent persistence error releases staged_source witness on Drop;
        //     post-rename intent persistence error (e.g. sync_all failure after rename) retains
        //     the witness on Drop so recovery/snapshots never wedge.
        let part_fd_arc = Arc::new(
            open_olap_partition_dir_nofollow(&layout, "metrics", day)
                .unwrap()
                .unwrap(),
        );
        let mk_stage = |tag: &str| {
            let batch = metrics_to_record_batch(&[mk_pt(tag)]).unwrap();
            olap.write_batches_to_staging(
                "metrics",
                day,
                Some("part-r18-rep.parquet"),
                [Ok(batch)],
                false,
            )
            .unwrap()
            .unwrap()
        };
        let p1_identity = FileIdentity {
            dev: p1_meta.dev(),
            ino: p1_meta.ino(),
            len: Some(p1_meta.len()),
            sha256: Some(p1_sha.clone()),
        };
        {
            let mut pre_staged = mk_stage("m-pre-err");
            let pre_src = format!(".replace-source-{}.staged", pre_staged.staging_name);
            OlapParquetStore::stage_into_partition_source_on_fd(
                &part_fd_arc,
                &mut pre_staged,
                &pre_src,
            )
            .unwrap();
            let pre_intent = ReplaceIntent {
                replacement: "part-r18-rep.parquet".to_owned(),
                replacement_id: Some(pre_staged.file_identity()),
                inputs: vec![p1_name.clone()],
                input_ids: BTreeMap::from([(p1_name.clone(), p1_identity.clone())]),
                poisoned: false,
                staged_source: Some(pre_src.clone()),
            };
            let pre_err = OlapParquetStore::persist_replace_intent_preserving_witness_on_fd(
                &part_fd_arc,
                &part_dir,
                &mut pre_staged,
                &pre_intent,
                |_fd, _val| Err("simulated pre-rename intent failure".to_owned()),
            )
            .unwrap_err();
            assert!(pre_err.contains("pre-rename intent failure"));
            assert!(!pre_staged.intent_recorded);
            drop(pre_staged);
            assert!(
                !part_dir.join(&pre_src).exists(),
                "pre-rename intent failure must unlink staged_source witness on Drop"
            );
        }
        {
            let mut post_staged = mk_stage("m-post-err");
            let post_src = format!(".replace-source-{}.staged", post_staged.staging_name);
            OlapParquetStore::stage_into_partition_source_on_fd(
                &part_fd_arc,
                &mut post_staged,
                &post_src,
            )
            .unwrap();
            let post_intent = ReplaceIntent {
                replacement: "part-r18-rep.parquet".to_owned(),
                replacement_id: Some(post_staged.file_identity()),
                inputs: vec![p1_name.clone()],
                input_ids: BTreeMap::from([(p1_name.clone(), p1_identity.clone())]),
                poisoned: false,
                staged_source: Some(post_src.clone()),
            };
            let post_err = OlapParquetStore::persist_replace_intent_preserving_witness_on_fd(
                &part_fd_arc,
                &part_dir,
                &mut post_staged,
                &post_intent,
                |fd, val| {
                    write_atomic_json_in_dir_fd_bounded(
                        fd,
                        REPLACE_INTENT_FILE,
                        val,
                        MAX_REPLACE_INTENT_BYTES,
                    )?;
                    Err("simulated dir sync failure after intent rename".to_owned())
                },
            )
            .unwrap_err();
            assert!(post_err.contains("dir sync failure"));
            assert!(post_staged.intent_recorded);
            drop(post_staged);
            assert!(
                part_dir.join(&post_src).is_file(),
                "post-rename intent sync failure must preserve staged_source witness across Drop"
            );
            assert_eq!(
                olap.snapshot_pinned_parquet_files("metrics", None)
                    .unwrap()
                    .iter()
                    .map(|p| p.display_path().to_path_buf())
                    .collect::<Vec<_>>(),
                vec![p1.clone()]
            );
            reconcile_partition_intent(&layout, &part_dir).unwrap();
            assert!(!part_dir.join(REPLACE_INTENT_FILE).exists());
            assert!(!part_dir.join(&post_src).exists());
            assert!(p1.is_file());
        }

        // 2f. Concurrent cutover: staged_source just renamed into intent.replacement returns
        //     retryable Ok(false) in verify_uncommitted_intent_inputs_on_fd rather than failing closed.
        let cutover_rep_name = "part-r18-cutover.parquet";
        let cutover_rep_path = part_dir.join(cutover_rep_name);
        fs::copy(&p1, &cutover_rep_path).unwrap();
        let cut_meta = fs::symlink_metadata(&cutover_rep_path).unwrap();
        let cut_sha = sha256_regular_file(&File::open(&cutover_rep_path).unwrap())
            .unwrap()
            .1;
        let cutover_intent = ReplaceIntent {
            replacement: cutover_rep_name.to_owned(),
            replacement_id: Some(FileIdentity {
                dev: cut_meta.dev(),
                ino: cut_meta.ino(),
                len: Some(cut_meta.len()),
                sha256: Some(cut_sha),
            }),
            inputs: vec![p1_name.clone()],
            input_ids: BTreeMap::from([(p1_name.clone(), p1_identity)]),
            poisoned: false,
            staged_source: Some(".replace-source-already-renamed.staged".to_owned()),
        };
        write_atomic_file_in_dir(
            &part_dir,
            REPLACE_INTENT_FILE,
            &serde_json::to_vec(&cutover_intent).unwrap(),
        )
        .unwrap();
        assert!(
            !verify_uncommitted_intent_inputs_on_fd(&part_fd_arc, &part_dir, &cutover_intent)
                .unwrap(),
            "missing staged_source after cutover rename must trigger retryable Ok(false)"
        );
        let snap_after_cutover = olap
            .snapshot_pinned_parquet_files("metrics", None)
            .unwrap()
            .into_iter()
            .map(|p| p.display_path().to_path_buf())
            .collect::<Vec<_>>();
        assert_eq!(snap_after_cutover, vec![cutover_rep_path.clone()]);
        fs::remove_file(part_dir.join(REPLACE_INTENT_FILE)).unwrap();
        fs::remove_file(&cutover_rep_path).unwrap();

        // 3. Compaction preflight rejects malformed Thrift wire-type Parquet before decoding.
        let bad_pq = part_dir.join("part-bad-compact.parquet");
        let diff_footer = [
            0x15_u8, 0x02, 0x19, 0x1c, 0x48, 0x01, b'r', 0x00, 0x16, 0x00, 0x13, 0x00, 0x00,
        ];
        let mut raw = b"PAR1".to_vec();
        raw.extend_from_slice(&diff_footer);
        raw.extend_from_slice(&u32::try_from(diff_footer.len()).unwrap().to_le_bytes());
        raw.extend_from_slice(b"PAR1");
        fs::write(&bad_pq, raw).unwrap();
        let compact_err = olap
            .compact_parquet_files("metrics", day, &[p1.clone(), bad_pq.clone()])
            .expect_err("compaction preflight must reject malformed Thrift wire type");
        assert!(
            compact_err.contains("wire type"),
            "unexpected compact preflight err: {compact_err}"
        );
        fs::remove_file(&bad_pq).unwrap();

        // 4. Staged output byte cap prevents compaction from publishing > max_output_bytes.
        let p2 = olap.write_metrics(&[mk_pt("m2")]).unwrap().pop().unwrap();
        let maint = acquire_maintenance_lock(&layout).unwrap();
        let res_none = olap
            .compact_parquet_files_unlocked_with_limits(
                "metrics",
                day,
                &[p1.clone(), p2.clone()],
                100,
                100_000,
                200_000,
                100, // tighter than any valid merged Parquet output
                &maint,
            )
            .unwrap();
        assert!(
            res_none.is_none(),
            "output cap must abort compaction staging without publishing oversized file"
        );
        assert!(p1.is_file() && p2.is_file());
    }

    #[test]
    fn round24_intent_serde_bounds_and_retained_accounting() {
        let dir = non_tmp_dir();
        let layout = StorageLayout::new(dir.path()).unwrap();
        let olap = OlapParquetStore::new(layout.clone());
        let part_dir = layout.olap_domain_dir("metrics").join("dt=2023-11-14");
        fs::create_dir_all(&part_dir).unwrap();

        let part_fd = open_standalone_dir_nofollow(&part_dir, "partition dir")
            .unwrap()
            .unwrap();

        // 1. Valid escaped filenames (e.g., quotes permitted by validate_single_normal_filename)
        //    round-trip within the 3 * pre_cap + MAX_REPLACE_INTENT_DECODED_BYTES parse budget.
        let escaped_intent = ReplaceIntent {
            replacement: "part-\"quoted\".parquet".to_owned(),
            replacement_id: None,
            inputs: vec!["part-\"quoted\"-in.parquet".to_owned()],
            input_ids: BTreeMap::new(),
            poisoned: false,
            staged_source: None,
        };
        let escaped_json = serde_json::to_vec(&escaped_intent).unwrap();
        assert!(escaped_json.contains(&b'\\'));
        fs::write(part_dir.join(REPLACE_INTENT_FILE), &escaped_json).unwrap();
        let escaped_pre_cap = escaped_json.len() + 1;
        let (parsed_escaped, _) = load_replace_intent_on_fd_measured(
            &part_fd,
            &part_dir,
            escaped_pre_cap * 3 + MAX_REPLACE_INTENT_DECODED_BYTES,
        )
        .unwrap()
        .expect(
            "escaped filename replace intent must deserialize within 3 * pre_cap + 20 KiB budget",
        );
        assert_eq!(parsed_escaped, escaped_intent);

        // 2. Duplicate entries in `inputs` reject during bounded serde parse.
        let dup_inputs =
            br#"{"replacement":"part-rep.parquet","inputs":["part-a.parquet","part-a.parquet"]}"#;
        fs::write(part_dir.join(REPLACE_INTENT_FILE), dup_inputs).unwrap();
        let dup_err = olap
            .snapshot_pinned_parquet_files_bounded(
                "metrics",
                None,
                MAX_SNAPSHOT_PARQUET_FILES,
                MAX_SNAPSHOT_METADATA_BYTES,
            )
            .expect_err("duplicate inputs entry must reject during serde parse");
        assert!(
            dup_err.contains("duplicate entry"),
            "unexpected duplicate inputs err: {dup_err}"
        );

        // 3. Sparse BTreeMap node accounting, synthetic max-size deserialization,
        //    and 3 * pre_cap + MAX_REPLACE_INTENT_DECODED_BYTES peak enforcement.
        let single_entry_intent = ReplaceIntent {
            replacement: "part-rep.parquet".to_owned(),
            replacement_id: None,
            inputs: vec!["part-a.parquet".to_owned()],
            input_ids: BTreeMap::from([(
                "part-a.parquet".to_owned(),
                FileIdentity {
                    dev: 1,
                    ino: 2,
                    len: Some(128),
                    sha256: None,
                },
            )]),
            poisoned: false,
            staged_source: None,
        };
        assert!(
            single_entry_intent.retained_heap_bytes() >= 1024,
            "sparse single-entry BTreeMap must account full node allocation, got {}",
            single_entry_intent.retained_heap_bytes()
        );
        assert!(
            intent_filter_overhead(0, &single_entry_intent) >= 1024,
            "sparse single-entry BTreeSet filter_overhead must include node allocation"
        );
        let max_entries_intent = ReplaceIntent {
            replacement: format!(
                "{}.parquet",
                "r".repeat(MAX_REPLACE_INTENT_FILENAME_BYTES - ".parquet".len())
            ),
            replacement_id: Some(FileIdentity {
                dev: 1,
                ino: 2,
                len: Some(1024),
                sha256: Some("a".repeat(MAX_REPLACE_INTENT_SHA256_BYTES)),
            }),
            inputs: (0..MAX_REPLACE_INTENT_INPUTS)
                .map(|i| {
                    format!(
                        "{i:02}{}.parquet",
                        "i".repeat(MAX_REPLACE_INTENT_FILENAME_BYTES - 2 - ".parquet".len())
                    )
                })
                .collect(),
            input_ids: (0..MAX_REPLACE_INTENT_INPUTS)
                .map(|i| {
                    (
                        format!(
                            "{i:02}{}.parquet",
                            "k".repeat(MAX_REPLACE_INTENT_FILENAME_BYTES - 2 - ".parquet".len())
                        ),
                        FileIdentity {
                            dev: 1,
                            ino: i as u64,
                            len: Some(512),
                            sha256: Some("b".repeat(MAX_REPLACE_INTENT_SHA256_BYTES)),
                        },
                    )
                })
                .collect(),
            poisoned: false,
            staged_source: Some(format!(
                ".replace-source-{}.staged",
                "s".repeat(MAX_REPLACE_INTENT_FILENAME_BYTES - ".replace-source-.staged".len())
            )),
        };
        let max_json = serde_json::to_vec(&max_entries_intent).unwrap();
        fs::write(part_dir.join(REPLACE_INTENT_FILE), &max_json).unwrap();
        let max_pre_cap = max_json.len() + 1;
        let (parsed_max, _) = load_replace_intent_on_fd_measured(
            &part_fd,
            &part_dir,
            max_pre_cap * 3 + MAX_REPLACE_INTENT_DECODED_BYTES,
        )
        .unwrap()
        .expect("max-size ReplaceIntent must deserialize within 3 * pre_cap + 20 KiB budget");
        assert_eq!(parsed_max, max_entries_intent);
        assert!(
            parsed_max.retained_heap_bytes() <= MAX_REPLACE_INTENT_DECODED_BYTES,
            "deserialized max ReplaceIntent retained bytes {} exceeded MAX_REPLACE_INTENT_DECODED_BYTES {}",
            parsed_max.retained_heap_bytes(),
            MAX_REPLACE_INTENT_DECODED_BYTES
        );

        // Large 32 KiB+ escaped key causes serde_json's internal scratch Vec to double up to
        // 2 * file_bytes (~65 KiB): verify 2 * pre_cap rejects pre-read while 3 * pre_cap passes.
        let large_escaped_key_json = format!(
            r#"{{"{}\"":"ignored","replacement":"part-rep.parquet","inputs":["part-a.parquet"]}}"#,
            "x".repeat(32 * 1024)
        );
        assert!(large_escaped_key_json.len() > 32 * 1024);
        fs::write(
            part_dir.join(REPLACE_INTENT_FILE),
            large_escaped_key_json.as_bytes(),
        )
        .unwrap();
        let escaped_key_pre_cap = large_escaped_key_json.len() + 1;
        let doubled_scratch_short_budget =
            escaped_key_pre_cap * 2 + MAX_REPLACE_INTENT_DECODED_BYTES;
        let tight_err = load_replace_intent_on_fd_measured(
            &part_fd,
            &part_dir,
            doubled_scratch_short_budget,
        )
        .expect_err("3 * pre_cap + MAX_REPLACE_INTENT_DECODED_BYTES must reject when budget only covers 2 * pre_cap");
        assert!(
            tight_err.contains("replace intent parse budget exceeds"),
            "unexpected tight budget err: {tight_err}"
        );
        let (parsed_escaped_key, _) = load_replace_intent_on_fd_measured(
            &part_fd,
            &part_dir,
            escaped_key_pre_cap * 3 + MAX_REPLACE_INTENT_DECODED_BYTES,
        )
        .unwrap()
        .expect("32 KiB+ escaped-key intent must succeed under 3 * pre_cap + 20 KiB budget");
        assert_eq!(parsed_escaped_key.replacement, "part-rep.parquet");

        // Reload check inside uncommitted_intent_superseded_on_fd_with_budget respects the caller
        // memory budget instead of falling back to usize::MAX when staged_source/inputs vanish.
        let reload_budget_err = verify_uncommitted_intent_inputs_on_fd_with_budget(
            &part_fd,
            &part_dir,
            &parsed_escaped_key,
            doubled_scratch_short_budget,
        )
        .expect_err("superseded reload under tight budget must reject before reading intent");
        assert!(
            reload_budget_err.contains("replace intent parse budget exceeds"),
            "unexpected superseded reload err: {reload_budget_err}"
        );
    }
}
