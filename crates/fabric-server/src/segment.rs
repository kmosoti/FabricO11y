//! Immutable Zstd Parquet segments (retained-history contract).
//!
//! A segment is built from one sealed server journal file. It is written
//! into `segments/.building-<label>`, every file is synced, the manifest is
//! written and synced last, and the directory is renamed to `seg-<label>` with
//! a directory sync: the rename is the commit point. A `.building-*`
//! directory found at startup is incomplete and removed.

use crate::rows::{GapRow, LogRow, MetricRow, Number, Rows, SpanRow, extract};
use crate::store::{Entry, Group};
use arrow_array::builder::FixedSizeBinaryBuilder;
use arrow_array::{
    Array, BinaryArray, BooleanArray, FixedSizeBinaryArray, Float64Array, Int32Array, Int64Array,
    RecordBatch, StringArray, UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use fabric_frame::frame::read_frame;
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;
use parquet::file::statistics::Statistics;
use prost::Message;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const VERSION: u32 = 1;
const ROW_GROUP: usize = 8192;
pub const MAX_GROUP_PAYLOAD: usize = 4 * 1024 * 1024;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn err(e: impl std::fmt::Display) -> io::Error {
    invalid(e.to_string())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    pub sha256: String,
    pub bytes: u64,
    pub rows: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub journal_label: u64,
    pub first_group: u64,
    pub last_group: u64,
    pub records: u64,
    pub received_min_ns: u64,
    pub received_max_ns: u64,
    /// Newest observed or point time per node label.
    pub freshness: BTreeMap<String, u64>,
    pub files: BTreeMap<String, FileEntry>,
}

pub fn segments_dir(state_dir: &Path) -> io::Result<PathBuf> {
    let dir = state_dir.join("segments");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn segment_name(label: u64) -> String {
    format!("seg-{label:020}")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

fn attrs_json(attributes: &BTreeMap<String, String>) -> String {
    serde_json::to_string(attributes).unwrap_or_else(|_| "{}".into())
}

fn node_id_array(ids: impl Iterator<Item = [u8; 16]>) -> io::Result<FixedSizeBinaryArray> {
    let mut builder = FixedSizeBinaryBuilder::new(16);
    for id in ids {
        builder.append_value(id).map_err(err)?;
    }
    Ok(builder.finish())
}

pub fn logs_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("group", DataType::UInt64, false),
        Field::new("node", DataType::Utf8, false),
        Field::new("node_id", DataType::FixedSizeBinary(16), false),
        Field::new("sequence", DataType::UInt64, false),
        Field::new("index", DataType::UInt32, false),
        Field::new("observed_ns", DataType::Int64, false),
        Field::new("body", DataType::Utf8, false),
        Field::new("attributes", DataType::Utf8, false),
    ]))
}

/// The spans table (ADR-0025), sorted by start time, node identity, sequence, index.
pub const SPANS: &str = "spans.parquet";
/// The trigram filter over each spans row group's hex trace IDs.
pub const SPANS_FILTER: &str = "spans_filter.bin";

pub fn spans_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("group", DataType::UInt64, false),
        Field::new("node", DataType::Utf8, false),
        Field::new("node_id", DataType::FixedSizeBinary(16), false),
        Field::new("sequence", DataType::UInt64, false),
        Field::new("index", DataType::UInt32, false),
        Field::new("start_ns", DataType::Int64, false),
        Field::new("end_ns", DataType::Int64, false),
        Field::new("trace_id", DataType::Utf8, false),
        Field::new("span_id", DataType::Utf8, false),
        Field::new("parent_span_id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("kind", DataType::Int32, false),
        Field::new("status", DataType::Int32, false),
        Field::new("attributes", DataType::Utf8, false),
    ]))
}

fn spans_batch(rows: &[SpanRow]) -> io::Result<RecordBatch> {
    let strings =
        |f: fn(&SpanRow) -> &str| Arc::new(StringArray::from_iter_values(rows.iter().map(f)));
    RecordBatch::try_new(
        spans_schema(),
        vec![
            Arc::new(UInt64Array::from_iter_values(rows.iter().map(|r| r.group))),
            strings(|r| r.node.as_str()),
            Arc::new(node_id_array(rows.iter().map(|r| r.node_id))?),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|r| r.sequence),
            )),
            Arc::new(UInt32Array::from_iter_values(rows.iter().map(|r| r.index))),
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|r| r.start_ns as i64),
            )),
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|r| r.end_ns as i64),
            )),
            strings(|r| r.trace_id.as_str()),
            strings(|r| r.span_id.as_str()),
            strings(|r| r.parent_span_id.as_str()),
            strings(|r| r.name.as_str()),
            Arc::new(Int32Array::from_iter_values(rows.iter().map(|r| r.kind))),
            Arc::new(Int32Array::from_iter_values(rows.iter().map(|r| r.status))),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| attrs_json(&r.attributes)),
            )),
        ],
    )
    .map_err(err)
}

pub fn metrics_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("group", DataType::UInt64, false),
        Field::new("node", DataType::Utf8, false),
        Field::new("node_id", DataType::FixedSizeBinary(16), false),
        Field::new("sequence", DataType::UInt64, false),
        Field::new("index", DataType::UInt32, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("unit", DataType::Utf8, false),
        Field::new("sum", DataType::Boolean, false),
        Field::new("monotonic", DataType::Boolean, false),
        Field::new("time_ns", DataType::Int64, false),
        Field::new("start_ns", DataType::Int64, false),
        Field::new("value_int", DataType::Int64, true),
        Field::new("value_double", DataType::Float64, true),
        Field::new("attributes", DataType::Utf8, false),
    ]))
}

pub fn gaps_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("group", DataType::UInt64, false),
        Field::new("node", DataType::Utf8, false),
        Field::new("node_id", DataType::FixedSizeBinary(16), false),
        Field::new("sequence", DataType::UInt64, false),
        Field::new("received_ns", DataType::Int64, false),
        Field::new("text", DataType::Utf8, false),
    ]))
}

pub fn batches_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("group", DataType::UInt64, false),
        Field::new("label", DataType::Utf8, false),
        Field::new("received_ns", DataType::Int64, false),
        Field::new("sha256", DataType::FixedSizeBinary(32), false),
        Field::new("batch", DataType::Binary, false),
    ]))
}

fn logs_batch(rows: &[LogRow]) -> io::Result<RecordBatch> {
    RecordBatch::try_new(
        logs_schema(),
        vec![
            Arc::new(UInt64Array::from_iter_values(rows.iter().map(|r| r.group))),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.node.as_str()),
            )),
            Arc::new(node_id_array(rows.iter().map(|r| r.node_id))?),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|r| r.sequence),
            )),
            Arc::new(UInt32Array::from_iter_values(rows.iter().map(|r| r.index))),
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|r| r.observed_ns as i64),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.body.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| attrs_json(&r.attributes)),
            )),
        ],
    )
    .map_err(err)
}

fn metrics_batch(rows: &[MetricRow]) -> io::Result<RecordBatch> {
    RecordBatch::try_new(
        metrics_schema(),
        vec![
            Arc::new(UInt64Array::from_iter_values(rows.iter().map(|r| r.group))),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.node.as_str()),
            )),
            Arc::new(node_id_array(rows.iter().map(|r| r.node_id))?),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|r| r.sequence),
            )),
            Arc::new(UInt32Array::from_iter_values(rows.iter().map(|r| r.index))),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.name.as_str()),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.unit.as_str()),
            )),
            Arc::new(BooleanArray::from_iter(rows.iter().map(|r| Some(r.sum)))),
            Arc::new(BooleanArray::from_iter(
                rows.iter().map(|r| Some(r.monotonic)),
            )),
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|r| r.time_ns as i64),
            )),
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|r| r.start_ns as i64),
            )),
            Arc::new(Int64Array::from_iter(rows.iter().map(|r| match r.value {
                Number::Int(v) => Some(v),
                Number::Double(_) => None,
            }))),
            Arc::new(Float64Array::from_iter(rows.iter().map(
                |r| match r.value {
                    Number::Double(v) => Some(v),
                    Number::Int(_) => None,
                },
            ))),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| attrs_json(&r.attributes)),
            )),
        ],
    )
    .map_err(err)
}

fn gaps_batch(rows: &[GapRow]) -> io::Result<RecordBatch> {
    RecordBatch::try_new(
        gaps_schema(),
        vec![
            Arc::new(UInt64Array::from_iter_values(rows.iter().map(|r| r.group))),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.node.as_str()),
            )),
            Arc::new(node_id_array(rows.iter().map(|r| r.node_id))?),
            Arc::new(UInt64Array::from_iter_values(
                rows.iter().map(|r| r.sequence),
            )),
            Arc::new(Int64Array::from_iter_values(
                rows.iter().map(|r| r.received_ns as i64),
            )),
            Arc::new(StringArray::from_iter_values(
                rows.iter().map(|r| r.text.as_str()),
            )),
        ],
    )
    .map_err(err)
}

fn batches_batch(records: &[(u64, Entry)]) -> io::Result<RecordBatch> {
    let mut sha = FixedSizeBinaryBuilder::new(32);
    for (_, entry) in records {
        sha.append_value(Sha256::digest(&entry.batch))
            .map_err(err)?;
    }
    RecordBatch::try_new(
        batches_schema(),
        vec![
            Arc::new(UInt64Array::from_iter_values(
                records.iter().map(|(g, _)| *g),
            )),
            Arc::new(StringArray::from_iter_values(
                records.iter().map(|(_, e)| e.label.as_str()),
            )),
            Arc::new(Int64Array::from_iter_values(
                records.iter().map(|(_, e)| e.received_unix_nano as i64),
            )),
            Arc::new(sha.finish()),
            Arc::new(BinaryArray::from_iter_values(
                records.iter().map(|(_, e)| e.batch.as_slice()),
            )),
        ],
    )
    .map_err(err)
}

/// Hash only bytes accepted by the sink. A failed write/flush prevents a
/// manifest from being returned; buffered bytes are flushed before file sync.
struct HashingWriter<W> {
    inner: W,
    hash: Sha256,
    bytes: u64,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(data)?;
        self.hash.update(&data[..n]);
        self.bytes += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn write_table(path: &Path, batch: &RecordBatch) -> io::Result<FileEntry> {
    let properties = WriterProperties::builder()
        .set_max_row_group_row_count(Some(ROW_GROUP))
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(3).map_err(err)?))
        .build();
    let mut output = HashingWriter {
        inner: BufWriter::with_capacity(256 * 1024, File::create(path)?),
        hash: Sha256::new(),
        bytes: 0,
    };
    {
        let mut writer =
            ArrowWriter::try_new(&mut output, batch.schema(), Some(properties)).map_err(err)?;
        writer.write(batch).map_err(err)?;
        writer.close().map_err(err)?;
    }
    output.flush()?;
    output.inner.get_ref().sync_all()?;
    Ok(FileEntry {
        sha256: hex(&output.hash.finalize()),
        bytes: output.bytes,
        rows: batch.num_rows() as u64,
    })
}

/// Write `text_filter.bin` beside a just-written `logs.parquet`: one filter per row
/// group, the groups taken from the file's own metadata so they match the reader's.
fn write_text_filter(building: &Path, logs: &[LogRow]) -> io::Result<FileEntry> {
    write_filter(
        building,
        "logs.parquet",
        crate::text_filter::FILE,
        logs,
        |r| r.body.as_str(),
    )
}

/// Write a trigram filter file beside a just-written table: one filter per row group
/// over `text` of its rows, the groups taken from the file's own metadata.
fn write_filter<R>(
    building: &Path,
    table: &str,
    file_name: &str,
    rows: &[R],
    text: impl Fn(&R) -> &str,
) -> io::Result<FileEntry> {
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(File::open(building.join(table))?).map_err(err)?;
    let mut groups = Vec::new();
    let mut at = 0_usize;
    for rg in builder.metadata().row_groups() {
        let n = usize::try_from(rg.num_rows()).map_err(err)?;
        let chunk = rows
            .get(at..at + n)
            .ok_or_else(|| invalid(format!("{table} row groups exceed rows")))?;
        groups.push(crate::text_filter::GroupFilter::build(
            chunk.iter().map(&text),
        ));
        at += n;
    }
    if at != rows.len() {
        return Err(invalid(format!("{table} row groups do not cover the rows")));
    }
    let bytes = crate::text_filter::encode(&groups);
    let mut file = File::create(building.join(file_name))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(FileEntry {
        sha256: hex(&Sha256::digest(&bytes)),
        bytes: bytes.len() as u64,
        rows: groups.len() as u64,
    })
}

/// The row-group filters of a Segment's logs table if the Segment has them and their
/// bytes match the manifest's digest and group count; `None` otherwise, and the
/// caller scans exactly.
pub fn read_text_filter(
    dir: &Path,
    manifest: &Manifest,
) -> Option<Vec<crate::text_filter::GroupFilter>> {
    read_filter(dir, manifest, crate::text_filter::FILE)
}

/// The verified trace-ID filters of a Segment's spans row groups (ADR-0025).
pub fn read_spans_filter(
    dir: &Path,
    manifest: &Manifest,
) -> Option<Vec<crate::text_filter::GroupFilter>> {
    read_filter(dir, manifest, SPANS_FILTER)
}

fn read_filter(
    dir: &Path,
    manifest: &Manifest,
    name: &str,
) -> Option<Vec<crate::text_filter::GroupFilter>> {
    let entry = manifest.files.get(name)?;
    let bytes = fs::read(dir.join(name)).ok()?;
    if bytes.len() as u64 != entry.bytes || hex(&Sha256::digest(&bytes)) != entry.sha256 {
        return None;
    }
    let groups = crate::text_filter::decode(&bytes).ok()?;
    (groups.len() as u64 == entry.rows).then_some(groups)
}

/// Read every group of one sealed journal file, in order.
pub fn read_sealed(path: &Path) -> io::Result<Vec<Group>> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut at = 0;
    let mut groups = Vec::new();
    while at < len {
        let (payload, next) = read_frame(&file, at, len, MAX_GROUP_PAYLOAD)?
            .ok_or_else(|| invalid("incomplete frame in sealed journal file"))?;
        groups.push(Group::decode(payload.as_slice()).map_err(err)?);
        at = next;
    }
    Ok(groups)
}

/// Build and commit the segment for sealed journal file `label`.
pub fn build(state_dir: &Path, label: u64, groups: &[Group]) -> io::Result<Manifest> {
    let dir = segments_dir(state_dir)?;
    let building = dir.join(format!(".building-{label:020}"));
    let _ = fs::remove_dir_all(&building);
    fs::create_dir(&building)?;
    let mut rows = Rows::default();
    let mut records = Vec::new();
    for group in groups {
        for entry in &group.entries {
            extract(group.group_sequence, entry, &mut rows)?;
            records.push((group.group_sequence, entry.clone()));
        }
    }
    if records.is_empty() {
        return Err(invalid("sealed journal file holds no records"));
    }
    let key_log = |r: &LogRow| (r.observed_ns, r.node_id, r.sequence, r.index);
    rows.logs.sort_by_key(key_log);
    rows.metrics
        .sort_by_key(|r| (r.time_ns, r.node_id, r.sequence, r.index));
    rows.spans
        .sort_by_key(|r| (r.start_ns, r.node_id, r.sequence, r.index));
    let mut freshness: BTreeMap<String, u64> = BTreeMap::new();
    for r in &rows.logs {
        let f = freshness.entry(r.node.clone()).or_default();
        *f = (*f).max(r.observed_ns);
    }
    for r in &rows.metrics {
        let f = freshness.entry(r.node.clone()).or_default();
        *f = (*f).max(r.time_ns);
    }
    for r in &rows.spans {
        let f = freshness.entry(r.node.clone()).or_default();
        *f = (*f).max(r.start_ns);
    }
    let mut files = BTreeMap::new();
    if !rows.spans.is_empty() {
        files.insert(
            SPANS.into(),
            write_table(&building.join(SPANS), &spans_batch(&rows.spans)?)?,
        );
        files.insert(
            SPANS_FILTER.into(),
            write_filter(&building, SPANS, SPANS_FILTER, &rows.spans, |r| {
                r.trace_id.as_str()
            })?,
        );
    }
    files.insert(
        "batches.parquet".into(),
        write_table(&building.join("batches.parquet"), &batches_batch(&records)?)?,
    );
    files.insert(
        "logs.parquet".into(),
        write_table(&building.join("logs.parquet"), &logs_batch(&rows.logs)?)?,
    );
    files.insert(
        crate::text_filter::FILE.into(),
        write_text_filter(&building, &rows.logs)?,
    );
    files.insert(
        "metrics.parquet".into(),
        write_table(
            &building.join("metrics.parquet"),
            &metrics_batch(&rows.metrics)?,
        )?,
    );
    files.insert(
        "gaps.parquet".into(),
        write_table(&building.join("gaps.parquet"), &gaps_batch(&rows.gaps)?)?,
    );
    let manifest = Manifest {
        version: VERSION,
        journal_label: label,
        first_group: groups.first().unwrap().group_sequence,
        last_group: groups.last().unwrap().group_sequence,
        records: records.len() as u64,
        received_min_ns: records
            .iter()
            .map(|(_, e)| e.received_unix_nano)
            .min()
            .unwrap(),
        received_max_ns: records
            .iter()
            .map(|(_, e)| e.received_unix_nano)
            .max()
            .unwrap(),
        freshness,
        files,
    };
    let mut out = File::create(building.join("manifest.json"))?;
    out.write_all(&serde_json::to_vec_pretty(&manifest).map_err(err)?)?;
    out.sync_all()?;
    sync_dir(&building)?;
    fs::rename(&building, dir.join(segment_name(label)))?;
    sync_dir(&dir)?;
    Ok(manifest)
}

/// Remove incomplete builds and interrupted deletions. Startup only: a
/// running sealer's `.building-*` directory must not be touched.
pub fn cleanup(state_dir: &Path) -> io::Result<()> {
    let dir = segments_dir(state_dir)?;
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(".building-") || name.starts_with(".deleting-") {
            fs::remove_dir_all(entry.path())?;
        }
    }
    sync_dir(&dir)
}

/// Committed segments in label order.
/// The labels of the committed Segment directories, in no particular order.
pub fn labels(state_dir: &Path) -> io::Result<Vec<u64>> {
    let dir = segments_dir(state_dir)?;
    let mut labels = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if let Some(label) = name
            .strip_prefix("seg-")
            .and_then(|d| d.parse::<u64>().ok())
        {
            labels.push(label);
        }
    }
    Ok(labels)
}

pub fn list(state_dir: &Path) -> io::Result<Vec<(u64, Manifest)>> {
    let dir = segments_dir(state_dir)?;
    let mut found = Vec::new();
    for label in labels(state_dir)? {
        match read_manifest(&dir.join(segment_name(label))) {
            Ok(manifest) => found.push((label, manifest)),
            // Deleted by retention between listing and reading.
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        }
    }
    found.sort_by_key(|(label, _)| *label);
    Ok(found)
}

pub fn read_manifest(dir: &Path) -> io::Result<Manifest> {
    let text = fs::read(dir.join("manifest.json"))?;
    let manifest: Manifest = serde_json::from_slice(&text).map_err(err)?;
    if manifest.version != VERSION {
        return Err(invalid("unsupported segment version"));
    }
    Ok(manifest)
}

/// Open one table, checking its size and row count against the manifest.
/// Whole-file hashes are checked by `verify`, not on every query.
fn open_table(
    dir: &Path,
    manifest: &Manifest,
    name: &str,
    schema: &SchemaRef,
) -> io::Result<ParquetRecordBatchReaderBuilder<File>> {
    let expected = manifest
        .files
        .get(name)
        .ok_or_else(|| invalid(format!("manifest lacks {name}")))?;
    // Read through the file so pruned row groups are never loaded.
    let file = File::open(dir.join(name))?;
    if file.metadata()?.len() != expected.bytes {
        return Err(invalid(format!("{name} size differs from its manifest")));
    }
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).map_err(err)?;
    if builder.schema().fields() != schema.fields()
        || builder.metadata().file_metadata().num_rows() as u64 != expected.rows
    {
        return Err(invalid(format!("{name} schema or row count differs")));
    }
    Ok(builder)
}

/// Row groups whose `column` (Int64) range overlaps `[from, to)`.
fn prune(
    builder: &ParquetRecordBatchReaderBuilder<File>,
    column: usize,
    from: u64,
    to: u64,
) -> Vec<usize> {
    builder
        .metadata()
        .row_groups()
        .iter()
        .enumerate()
        .filter(|(_, rg)| match rg.column(column).statistics() {
            Some(Statistics::Int64(s)) => match (s.min_opt(), s.max_opt()) {
                (Some(min), Some(max)) => (*max as u64) >= from && (*min as u64) < to,
                _ => true,
            },
            _ => true,
        })
        .map(|(i, _)| i)
        .collect()
}

/// Which table of a Segment a row-group bound describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Table {
    Logs,
    Metrics,
    Spans,
}

/// Row groups as (index, min, max) of their key's time column.
pub type GroupBounds = Vec<(usize, u64, u64)>;

/// Every row group of `table` as (index, min, max) of its key's time column, from the
/// file's statistics. A group without statistics is unbounded: `(0, u64::MAX)`, so a
/// walk over these bounds never skips a group it cannot prove empty.
pub fn row_group_bounds(
    dir: &Path,
    manifest: &Manifest,
    table: Table,
) -> io::Result<Vec<(usize, u64, u64)>> {
    let (builder, column) = match table {
        Table::Logs => (
            open_table(dir, manifest, "logs.parquet", &logs_schema())?,
            5,
        ),
        Table::Metrics => (
            open_table(dir, manifest, "metrics.parquet", &metrics_schema())?,
            9,
        ),
        // A Segment sealed without spans has no spans table: no groups.
        Table::Spans if !manifest.files.contains_key(SPANS) => return Ok(Vec::new()),
        Table::Spans => (open_table(dir, manifest, SPANS, &spans_schema())?, 5),
    };
    Ok(builder
        .metadata()
        .row_groups()
        .iter()
        .enumerate()
        .map(|(i, rg)| match rg.column(column).statistics() {
            Some(Statistics::Int64(s)) => match (s.min_opt(), s.max_opt()) {
                (Some(min), Some(max)) => (i, *min as u64, *max as u64),
                _ => (i, 0, u64::MAX),
            },
            _ => (i, 0, u64::MAX),
        })
        .collect())
}

fn col<A: Array + 'static>(batch: &RecordBatch, index: usize) -> io::Result<&A> {
    batch
        .column(index)
        .as_any()
        .downcast_ref::<A>()
        .ok_or_else(|| invalid(format!("unexpected array type at column {index}")))
}

fn attrs_of(text: &str) -> io::Result<BTreeMap<String, String>> {
    serde_json::from_str(text).map_err(err)
}

pub fn scan_logs(
    dir: &Path,
    manifest: &Manifest,
    from: u64,
    to: u64,
    mut visit: impl FnMut(LogRow),
) -> io::Result<()> {
    let builder = open_table(dir, manifest, "logs.parquet", &logs_schema())?;
    let groups = prune(&builder, 5, from, to);
    read_logs(builder, groups, from, to, &mut visit)
}

/// Scan only the given row groups of the logs table, rows in `[from, to)`.
pub fn scan_logs_groups(
    dir: &Path,
    manifest: &Manifest,
    groups: Vec<usize>,
    from: u64,
    to: u64,
    mut visit: impl FnMut(LogRow),
) -> io::Result<()> {
    let builder = open_table(dir, manifest, "logs.parquet", &logs_schema())?;
    read_logs(builder, groups, from, to, &mut visit)
}

/// Spans with `start_ns` in `[from, to)`; none when the Segment has no spans table.
pub fn scan_spans(
    dir: &Path,
    manifest: &Manifest,
    from: u64,
    to: u64,
    mut visit: impl FnMut(SpanRow),
) -> io::Result<()> {
    if !manifest.files.contains_key(SPANS) {
        return Ok(());
    }
    let builder = open_table(dir, manifest, SPANS, &spans_schema())?;
    let groups = prune(&builder, 5, from, to);
    read_spans(builder, groups, from, to, &mut visit)
}

/// Scan only the given row groups of the spans table.
pub fn scan_spans_groups(
    dir: &Path,
    manifest: &Manifest,
    groups: Vec<usize>,
    from: u64,
    to: u64,
    mut visit: impl FnMut(SpanRow),
) -> io::Result<()> {
    let builder = open_table(dir, manifest, SPANS, &spans_schema())?;
    read_spans(builder, groups, from, to, &mut visit)
}

fn read_spans(
    builder: ParquetRecordBatchReaderBuilder<File>,
    groups: Vec<usize>,
    from: u64,
    to: u64,
    visit: &mut impl FnMut(SpanRow),
) -> io::Result<()> {
    for batch in builder.with_row_groups(groups).build().map_err(err)? {
        let batch = batch.map_err(err)?;
        let s = |i| col::<StringArray>(&batch, i);
        let (g, n, id, sq, ix, st, en) = (
            col::<UInt64Array>(&batch, 0)?,
            s(1)?,
            col::<FixedSizeBinaryArray>(&batch, 2)?,
            col::<UInt64Array>(&batch, 3)?,
            col::<UInt32Array>(&batch, 4)?,
            col::<Int64Array>(&batch, 5)?,
            col::<Int64Array>(&batch, 6)?,
        );
        let (tr, sp, pa, nm, ki, stt, at) = (
            s(7)?,
            s(8)?,
            s(9)?,
            s(10)?,
            col::<Int32Array>(&batch, 11)?,
            col::<Int32Array>(&batch, 12)?,
            s(13)?,
        );
        for row in 0..batch.num_rows() {
            let start = st.value(row) as u64;
            if start < from || start >= to {
                continue;
            }
            visit(SpanRow {
                group: g.value(row),
                node: n.value(row).to_owned(),
                node_id: id.value(row).try_into().map_err(err)?,
                sequence: sq.value(row),
                index: ix.value(row),
                trace_id: tr.value(row).to_owned(),
                span_id: sp.value(row).to_owned(),
                parent_span_id: pa.value(row).to_owned(),
                name: nm.value(row).to_owned(),
                kind: ki.value(row),
                status: stt.value(row),
                start_ns: start,
                end_ns: en.value(row) as u64,
                attributes: attrs_of(at.value(row))?,
            });
        }
    }
    Ok(())
}

fn read_logs(
    builder: ParquetRecordBatchReaderBuilder<File>,
    groups: Vec<usize>,
    from: u64,
    to: u64,
    visit: &mut impl FnMut(LogRow),
) -> io::Result<()> {
    for batch in builder.with_row_groups(groups).build().map_err(err)? {
        let batch = batch.map_err(err)?;
        let (g, n, id, s, i, t, b, a) = (
            col::<UInt64Array>(&batch, 0)?,
            col::<StringArray>(&batch, 1)?,
            col::<FixedSizeBinaryArray>(&batch, 2)?,
            col::<UInt64Array>(&batch, 3)?,
            col::<UInt32Array>(&batch, 4)?,
            col::<Int64Array>(&batch, 5)?,
            col::<StringArray>(&batch, 6)?,
            col::<StringArray>(&batch, 7)?,
        );
        for row in 0..batch.num_rows() {
            let observed = t.value(row) as u64;
            if observed < from || observed >= to {
                continue;
            }
            visit(LogRow {
                group: g.value(row),
                node: n.value(row).to_owned(),
                node_id: id.value(row).try_into().map_err(err)?,
                sequence: s.value(row),
                index: i.value(row),
                observed_ns: observed,
                body: b.value(row).to_owned(),
                attributes: attrs_of(a.value(row))?,
            });
        }
    }
    Ok(())
}

pub fn scan_metrics(
    dir: &Path,
    manifest: &Manifest,
    from: u64,
    to: u64,
    mut visit: impl FnMut(MetricRow),
) -> io::Result<()> {
    let builder = open_table(dir, manifest, "metrics.parquet", &metrics_schema())?;
    let groups = prune(&builder, 9, from, to);
    read_metrics(builder, groups, from, to, &mut visit)
}

/// Scan only the given row groups of the metrics table, points in `[from, to)`.
pub fn scan_metrics_groups(
    dir: &Path,
    manifest: &Manifest,
    groups: Vec<usize>,
    from: u64,
    to: u64,
    mut visit: impl FnMut(MetricRow),
) -> io::Result<()> {
    let builder = open_table(dir, manifest, "metrics.parquet", &metrics_schema())?;
    read_metrics(builder, groups, from, to, &mut visit)
}

fn read_metrics(
    builder: ParquetRecordBatchReaderBuilder<File>,
    groups: Vec<usize>,
    from: u64,
    to: u64,
    visit: &mut impl FnMut(MetricRow),
) -> io::Result<()> {
    for batch in builder.with_row_groups(groups).build().map_err(err)? {
        let batch = batch.map_err(err)?;
        let vi = col::<Int64Array>(&batch, 11)?;
        let vd = col::<Float64Array>(&batch, 12)?;
        let t = col::<Int64Array>(&batch, 9)?;
        for row in 0..batch.num_rows() {
            let time = t.value(row) as u64;
            if time < from || time >= to {
                continue;
            }
            let value = if vi.is_valid(row) {
                Number::Int(vi.value(row))
            } else {
                Number::Double(vd.value(row))
            };
            visit(MetricRow {
                group: col::<UInt64Array>(&batch, 0)?.value(row),
                node: col::<StringArray>(&batch, 1)?.value(row).to_owned(),
                node_id: col::<FixedSizeBinaryArray>(&batch, 2)?
                    .value(row)
                    .try_into()
                    .map_err(err)?,
                sequence: col::<UInt64Array>(&batch, 3)?.value(row),
                index: col::<UInt32Array>(&batch, 4)?.value(row),
                name: col::<StringArray>(&batch, 5)?.value(row).to_owned(),
                unit: col::<StringArray>(&batch, 6)?.value(row).to_owned(),
                sum: col::<BooleanArray>(&batch, 7)?.value(row),
                monotonic: col::<BooleanArray>(&batch, 8)?.value(row),
                time_ns: time,
                start_ns: col::<Int64Array>(&batch, 10)?.value(row) as u64,
                value,
                attributes: attrs_of(col::<StringArray>(&batch, 13)?.value(row))?,
            });
        }
    }
    Ok(())
}

pub fn scan_gaps(dir: &Path, manifest: &Manifest, mut visit: impl FnMut(GapRow)) -> io::Result<()> {
    let builder = open_table(dir, manifest, "gaps.parquet", &gaps_schema())?;
    for batch in builder.build().map_err(err)? {
        let batch = batch.map_err(err)?;
        for row in 0..batch.num_rows() {
            visit(GapRow {
                group: col::<UInt64Array>(&batch, 0)?.value(row),
                node: col::<StringArray>(&batch, 1)?.value(row).to_owned(),
                node_id: col::<FixedSizeBinaryArray>(&batch, 2)?
                    .value(row)
                    .try_into()
                    .map_err(err)?,
                sequence: col::<UInt64Array>(&batch, 3)?.value(row),
                received_ns: col::<Int64Array>(&batch, 4)?.value(row) as u64,
                text: col::<StringArray>(&batch, 5)?.value(row).to_owned(),
            });
        }
    }
    Ok(())
}

/// Every record of a segment with its group, for server_dump and checks.
pub fn scan_batches(
    dir: &Path,
    manifest: &Manifest,
    mut visit: impl FnMut(u64, Entry),
) -> io::Result<()> {
    let builder = open_table(dir, manifest, "batches.parquet", &batches_schema())?;
    for batch in builder.build().map_err(err)? {
        let batch = batch.map_err(err)?;
        let g = col::<UInt64Array>(&batch, 0)?;
        let l = col::<StringArray>(&batch, 1)?;
        let r = col::<Int64Array>(&batch, 2)?;
        let sha = col::<FixedSizeBinaryArray>(&batch, 3)?;
        let b = col::<BinaryArray>(&batch, 4)?;
        for row in 0..batch.num_rows() {
            if Sha256::digest(b.value(row)).as_slice() != sha.value(row) {
                return Err(invalid("segment record digest mismatch"));
            }
            visit(
                g.value(row),
                Entry {
                    label: l.value(row).to_owned(),
                    batch: b.value(row).to_vec(),
                    received_unix_nano: r.value(row) as u64,
                },
            );
        }
    }
    Ok(())
}

/// Check every file's SHA-256 against the manifest.
pub fn verify(dir: &Path, manifest: &Manifest) -> io::Result<()> {
    for (name, expected) in &manifest.files {
        let bytes = fs::read(dir.join(name))?;
        if hex(&Sha256::digest(&bytes)) != expected.sha256 {
            return Err(invalid(format!("{name} digest differs from its manifest")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod output_tests {
    use super::*;

    struct PartialSink {
        bytes: Vec<u8>,
        remaining: usize,
    }
    impl Write for PartialSink {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::other("injected write failure"));
            }
            let n = data.len().min(3).min(self.remaining);
            self.bytes.extend_from_slice(&data[..n]);
            self.remaining -= n;
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("injected flush failure"))
        }
    }

    #[test]
    fn hash_and_length_follow_partial_writes_and_exclude_failed_bytes() {
        let mut output = HashingWriter {
            inner: PartialSink {
                bytes: Vec::new(),
                remaining: 5,
            },
            hash: Sha256::new(),
            bytes: 0,
        };
        assert!(output.write_all(b"123456789").is_err());
        assert_eq!(output.inner.bytes, b"12345");
        assert_eq!(output.bytes, 5);
        assert_eq!(output.hash.finalize(), Sha256::digest(b"12345"));
    }

    #[test]
    fn flush_failure_is_propagated() {
        let mut output = HashingWriter {
            inner: PartialSink {
                bytes: Vec::new(),
                remaining: 10,
            },
            hash: Sha256::new(),
            bytes: 0,
        };
        output.write_all(b"123456").unwrap();
        assert_eq!(output.bytes, 6);
        assert_eq!(
            output.flush().unwrap_err().to_string(),
            "injected flush failure"
        );
    }

    #[test]
    fn streamed_table_is_byte_identical_to_buffered_reference() {
        let schema = Arc::new(Schema::new(vec![Field::new("body", DataType::Utf8, false)]));
        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(StringArray::from_iter_values(
                (0..ROW_GROUP + 3)
                    .map(|i| format!("row-{i}: preserves unicode λ and row-group boundaries")),
            ))],
        )
        .unwrap();
        let properties = WriterProperties::builder()
            .set_max_row_group_row_count(Some(ROW_GROUP))
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(3).unwrap()))
            .build();
        let mut expected = Vec::new();
        {
            let mut writer =
                ArrowWriter::try_new(&mut expected, batch.schema(), Some(properties)).unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();
        }
        let dir = std::env::temp_dir().join(format!(
            "fabric-output-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("table.parquet");
        let entry = write_table(&path, &batch).unwrap();
        assert_eq!(fs::read(&path).unwrap(), expected);
        assert_eq!(entry.sha256, hex(&Sha256::digest(&expected)));
        assert_eq!(entry.bytes, expected.len() as u64);
        assert_eq!(entry.rows, batch.num_rows() as u64);
        fs::remove_dir_all(dir).unwrap();
    }
}
