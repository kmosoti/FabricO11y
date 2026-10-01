//! Scratch study: candidate Segment-building algorithms over one sealed
//! journal file. Not product code.

use crate::rows::{Attributes, GapRow, LogRow, MetricRow, Number, Rows, extract};
use crate::segment::{
    FileEntry, MAX_GROUP_PAYLOAD, Manifest, batches_batch, batches_schema, gaps_batch,
    gaps_schema, hex, logs_batch, logs_schema, metrics_batch, metrics_schema, segment_name,
};
use crate::store::{Entry, Group};
use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use fabric_frame::frame::read_frame;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;
use prost::Message;
use sha2::{Digest, Sha256};
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BinaryHeap};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

pub const ROW_GROUP: usize = 8192;
/// Row-group byte cap for the streaming sinks (encoded, in progress).
pub const RG_BYTES: usize = 8 << 20;
/// Batches-table chunk cap (raw bytes).
const BATCH_CHUNK_BYTES: usize = 4 << 20;
/// Byte cap for every reorder window, run and bucket.
pub const BUF_BYTES: usize = 16 << 20;

/// An output buffer is full at a row group of rows or `RG_BYTES`.
fn full<R: SortRow>(v: &[R], bytes: &mut usize, r: &R) -> bool {
    *bytes += r.approx_bytes();
    if v.len() >= ROW_GROUP || *bytes >= RG_BYTES {
        *bytes = 0;
        true
    } else {
        false
    }
}

pub type Key = (u64, [u8; 16], u64, u32);

fn err(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

// ---------- spill encoding ----------

fn put_u64(o: &mut Vec<u8>, v: u64) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn put_u32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn put_str(o: &mut Vec<u8>, s: &str) {
    put_u32(o, s.len() as u32);
    o.extend_from_slice(s.as_bytes());
}
fn put_attrs(o: &mut Vec<u8>, a: &Attributes) {
    put_u32(o, a.len() as u32);
    for (k, v) in a {
        put_str(o, k);
        put_str(o, v);
    }
}
pub struct Cur<'a>(&'a [u8]);
impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(err("short spill record"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn id(&mut self) -> io::Result<[u8; 16]> {
        Ok(self.take(16)?.try_into().unwrap())
    }
    fn string(&mut self) -> io::Result<String> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(err)
    }
    fn attrs(&mut self) -> io::Result<Attributes> {
        let n = self.u32()?;
        let mut a = Attributes::new();
        for _ in 0..n {
            let k = self.string()?;
            let v = self.string()?;
            a.insert(k, v);
        }
        Ok(a)
    }
}

// ---------- sortable tables ----------

pub trait SortRow: Sized {
    fn key(&self) -> Key;
    fn batch(rows: &[Self]) -> io::Result<RecordBatch>;
    fn schema() -> SchemaRef;
    fn encode(&self, o: &mut Vec<u8>);
    fn decode(c: &mut Cur<'_>) -> io::Result<Self>;
    fn approx_bytes(&self) -> usize;
}

impl SortRow for LogRow {
    fn key(&self) -> Key {
        (self.observed_ns, self.node_id, self.sequence, self.index)
    }
    fn batch(rows: &[Self]) -> io::Result<RecordBatch> {
        logs_batch(rows)
    }
    fn schema() -> SchemaRef {
        logs_schema()
    }
    fn encode(&self, o: &mut Vec<u8>) {
        put_u64(o, self.group);
        put_str(o, &self.node);
        o.extend_from_slice(&self.node_id);
        put_u64(o, self.sequence);
        put_u32(o, self.index);
        put_u64(o, self.observed_ns);
        put_str(o, &self.body);
        put_attrs(o, &self.attributes);
    }
    fn decode(c: &mut Cur<'_>) -> io::Result<Self> {
        Ok(LogRow {
            group: c.u64()?,
            node: c.string()?,
            node_id: c.id()?,
            sequence: c.u64()?,
            index: c.u32()?,
            observed_ns: c.u64()?,
            body: c.string()?,
            attributes: c.attrs()?,
        })
    }
    fn approx_bytes(&self) -> usize {
        96 + self.node.len() + self.body.len() + self.attributes.len() * 64
    }
}

impl SortRow for MetricRow {
    fn key(&self) -> Key {
        (self.time_ns, self.node_id, self.sequence, self.index)
    }
    fn batch(rows: &[Self]) -> io::Result<RecordBatch> {
        metrics_batch(rows)
    }
    fn schema() -> SchemaRef {
        metrics_schema()
    }
    fn encode(&self, o: &mut Vec<u8>) {
        put_u64(o, self.group);
        put_str(o, &self.node);
        o.extend_from_slice(&self.node_id);
        put_u64(o, self.sequence);
        put_u32(o, self.index);
        put_str(o, &self.name);
        put_str(o, &self.unit);
        o.push(self.sum as u8 | (self.monotonic as u8) << 1);
        put_u64(o, self.time_ns);
        put_u64(o, self.start_ns);
        match self.value {
            Number::Int(v) => {
                o.push(0);
                put_u64(o, v as u64)
            }
            Number::Double(v) => {
                o.push(1);
                put_u64(o, v.to_bits())
            }
        }
        put_attrs(o, &self.attributes);
    }
    fn decode(c: &mut Cur<'_>) -> io::Result<Self> {
        let group = c.u64()?;
        let node = c.string()?;
        let node_id = c.id()?;
        let sequence = c.u64()?;
        let index = c.u32()?;
        let name = c.string()?;
        let unit = c.string()?;
        let flags = c.take(1)?[0];
        let time_ns = c.u64()?;
        let start_ns = c.u64()?;
        let tag = c.take(1)?[0];
        let raw = c.u64()?;
        let value = if tag == 0 {
            Number::Int(raw as i64)
        } else {
            Number::Double(f64::from_bits(raw))
        };
        Ok(MetricRow {
            group,
            node,
            node_id,
            sequence,
            index,
            name,
            unit,
            sum: flags & 1 != 0,
            monotonic: flags & 2 != 0,
            time_ns,
            start_ns,
            value,
            attributes: c.attrs()?,
        })
    }
    fn approx_bytes(&self) -> usize {
        128 + self.node.len() + self.name.len() + self.unit.len() + self.attributes.len() * 64
    }
}

// ---------- sinks ----------

struct HashFile {
    file: BufWriter<File>,
    hash: Sha256,
    bytes: u64,
}
impl Write for HashFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.file.write(buf)?;
        self.hash.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

pub struct Sink {
    writer: ArrowWriter<HashFile>,
    rows: u64,
}

fn props(compression: bool) -> io::Result<WriterProperties> {
    let b = WriterProperties::builder().set_max_row_group_row_count(Some(ROW_GROUP));
    Ok(if compression {
        b.set_compression(Compression::ZSTD(ZstdLevel::try_new(3).map_err(err)?))
    } else {
        b
    }
    .build())
}

impl Sink {
    pub fn new(path: &Path, schema: SchemaRef) -> io::Result<Self> {
        let hf = HashFile {
            file: BufWriter::with_capacity(256 << 10, File::create(path)?),
            hash: Sha256::new(),
            bytes: 0,
        };
        Ok(Sink {
            writer: ArrowWriter::try_new(hf, schema, Some(props(true)?)).map_err(err)?,
            rows: 0,
        })
    }
    pub fn write(&mut self, batch: &RecordBatch) -> io::Result<()> {
        self.writer.write(batch).map_err(err)?;
        self.rows += batch.num_rows() as u64;
        if self.writer.in_progress_size() >= RG_BYTES {
            self.writer.flush().map_err(err)?;
        }
        Ok(())
    }
    /// Force a row-group boundary.
    pub fn cut(&mut self) -> io::Result<()> {
        self.writer.flush().map_err(err)
    }
    pub fn close(self) -> io::Result<FileEntry> {
        let rows = self.rows;
        let hf = self.writer.into_inner().map_err(err)?;
        let HashFile { file, hash, bytes } = hf;
        let file = file.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        Ok(FileEntry {
            sha256: hex(&hash.finalize()),
            bytes,
            rows,
        })
    }
}

// ---------- strategies for the two sorted tables ----------

pub trait Strategy<R: SortRow> {
    fn push(&mut self, row: R, sink: &mut Sink) -> io::Result<()>;
    fn finish(&mut self, sink: &mut Sink) -> io::Result<()>;
}

/// A: sort each row-group-sized chunk independently.
pub struct ChunkSort<R> {
    buf: Vec<R>,
    bytes: usize,
}
impl<R> ChunkSort<R> {
    pub fn new() -> Self {
        ChunkSort {
            buf: Vec::new(),
            bytes: 0,
        }
    }
}
impl<R: SortRow> ChunkSort<R> {
    fn flush(&mut self, sink: &mut Sink) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        self.buf.sort_unstable_by_key(|r| r.key());
        sink.write(&R::batch(&self.buf)?)?;
        sink.cut()?;
        self.buf.clear();
        self.bytes = 0;
        Ok(())
    }
}
impl<R: SortRow> Strategy<R> for ChunkSort<R> {
    fn push(&mut self, row: R, sink: &mut Sink) -> io::Result<()> {
        self.bytes += row.approx_bytes();
        self.buf.push(row);
        if self.buf.len() >= ROW_GROUP || self.bytes >= RG_BYTES {
            self.flush(sink)?;
        }
        Ok(())
    }
    fn finish(&mut self, sink: &mut Sink) -> io::Result<()> {
        self.flush(sink)
    }
}

struct Keyed<R>(Key, R);
impl<R> PartialEq for Keyed<R> {
    fn eq(&self, o: &Self) -> bool {
        self.0 == o.0
    }
}
impl<R> Eq for Keyed<R> {}
impl<R> PartialOrd for Keyed<R> {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl<R> Ord for Keyed<R> {
    fn cmp(&self, o: &Self) -> Ordering {
        self.0.cmp(&o.0)
    }
}

/// C: a bounded min-heap reorder window; rows that arrive behind the
/// watermark go to separate, independently sorted row groups.
pub struct Watermark<R> {
    heap: BinaryHeap<Reverse<Keyed<R>>>,
    window: usize,
    heap_bytes: usize,
    last: Option<Key>,
    main: Vec<R>,
    main_bytes: usize,
    late: Vec<R>,
    late_bytes: usize,
    pub late_rows: u64,
}
impl<R> Watermark<R> {
    pub fn new(window: usize) -> Self {
        Watermark {
            heap: BinaryHeap::with_capacity(window + 1),
            window,
            heap_bytes: 0,
            last: None,
            main: Vec::new(),
            main_bytes: 0,
            late: Vec::new(),
            late_bytes: 0,
            late_rows: 0,
        }
    }
}
impl<R: SortRow> Watermark<R> {
    fn emit(&mut self, k: Key, r: R, sink: &mut Sink) -> io::Result<()> {
        if self.last.is_none_or(|l| k >= l) {
            self.last = Some(k);
            let f = full(&self.main, &mut self.main_bytes, &r);
            self.main.push(r);
            if f || self.main.len() >= ROW_GROUP {
                sink.write(&R::batch(&self.main)?)?;
                sink.cut()?;
                self.main.clear();
                self.main_bytes = 0;
            }
        } else {
            self.late_rows += 1;
            let f = full(&self.late, &mut self.late_bytes, &r);
            self.late.push(r);
            if f || self.late.len() >= ROW_GROUP {
                self.late_bytes = 0;
                self.late.sort_unstable_by_key(|r| r.key());
                sink.write(&R::batch(&self.late)?)?;
                sink.cut()?;
                self.late.clear();
            }
        }
        Ok(())
    }
}
impl<R: SortRow> Strategy<R> for Watermark<R> {
    fn push(&mut self, row: R, sink: &mut Sink) -> io::Result<()> {
        self.heap_bytes += row.approx_bytes();
        self.heap.push(Reverse(Keyed(row.key(), row)));
        while self.heap.len() > self.window || self.heap_bytes > BUF_BYTES {
            let Reverse(Keyed(k, r)) = self.heap.pop().unwrap();
            self.heap_bytes -= r.approx_bytes();
            self.emit(k, r, sink)?;
        }
        Ok(())
    }
    fn finish(&mut self, sink: &mut Sink) -> io::Result<()> {
        while let Some(Reverse(Keyed(k, r))) = self.heap.pop() {
            self.emit(k, r, sink)?;
        }
        for v in [&mut self.main, &mut self.late] {
            if !v.is_empty() {
                v.sort_unstable_by_key(|r| r.key());
                sink.write(&R::batch(v)?)?;
                sink.cut()?;
                v.clear();
            }
        }
        Ok(())
    }
}

fn spill_write(w: &mut BufWriter<File>, scratch: &mut Vec<u8>, r: &impl SortRow) -> io::Result<u64> {
    scratch.clear();
    r.encode(scratch);
    w.write_all(&(scratch.len() as u32).to_le_bytes())?;
    w.write_all(scratch)?;
    Ok(4 + scratch.len() as u64)
}

struct SpillReader<R> {
    r: BufReader<File>,
    buf: Vec<u8>,
    _p: std::marker::PhantomData<R>,
}
impl<R: SortRow> SpillReader<R> {
    fn open(path: &Path) -> io::Result<Self> {
        Ok(SpillReader {
            r: BufReader::with_capacity(64 << 10, File::open(path)?),
            buf: Vec::new(),
            _p: std::marker::PhantomData,
        })
    }
    fn next(&mut self) -> io::Result<Option<R>> {
        let mut len = [0u8; 4];
        match self.r.read_exact(&mut len) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }
        self.buf.resize(u32::from_le_bytes(len) as usize, 0);
        self.r.read_exact(&mut self.buf)?;
        Ok(Some(R::decode(&mut Cur(&self.buf))?))
    }
}

/// D: exact range partitioning. Splitters come from a key-only first pass;
/// each bucket holds at most `bucket` rows, so its in-memory sort is bounded.
pub struct RangePartition<R> {
    splitters: Vec<Key>,
    dir: PathBuf,
    tag: &'static str,
    writers: Vec<Option<BufWriter<File>>>,
    scratch: Vec<u8>,
    pub spill_bytes: u64,
    _p: std::marker::PhantomData<R>,
}
impl<R> RangePartition<R> {
    pub fn new(splitters: Vec<Key>, dir: PathBuf, tag: &'static str) -> Self {
        let n = splitters.len() + 1;
        RangePartition {
            splitters,
            dir,
            tag,
            writers: (0..n).map(|_| None).collect(),
            scratch: Vec::new(),
            spill_bytes: 0,
            _p: std::marker::PhantomData,
        }
    }
    fn path(&self, b: usize) -> PathBuf {
        self.dir.join(format!(".spill-{}-{b:05}", self.tag))
    }
}
impl<R: SortRow> Strategy<R> for RangePartition<R> {
    fn push(&mut self, row: R, _sink: &mut Sink) -> io::Result<()> {
        let k = row.key();
        let b = self.splitters.partition_point(|s| *s <= k);
        if self.writers[b].is_none() {
            self.writers[b] = Some(BufWriter::with_capacity(
                64 << 10,
                File::create(self.path(b))?,
            ));
        }
        let w = self.writers[b].as_mut().unwrap();
        self.spill_bytes += spill_write(w, &mut self.scratch, &row)?;
        Ok(())
    }
    fn finish(&mut self, sink: &mut Sink) -> io::Result<()> {
        for b in 0..self.writers.len() {
            let Some(w) = self.writers[b].take() else {
                continue;
            };
            drop(w.into_inner().map_err(|e| e.into_error())?);
            let path = self.path(b);
            let mut reader = SpillReader::<R>::open(&path)?;
            let mut rows = Vec::new();
            while let Some(r) = reader.next()? {
                rows.push(r);
            }
            rows.sort_unstable_by_key(|r| r.key());
            let (mut start, mut bytes) = (0, 0);
            for i in 0..rows.len() {
                if full(&rows[start..i], &mut bytes, &rows[i]) {
                    sink.write(&R::batch(&rows[start..=i])?)?;
                    start = i + 1;
                }
            }
            if start < rows.len() {
                sink.write(&R::batch(&rows[start..])?)?;
            }
            drop(rows);
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

/// E: external merge sort: sorted runs of `run` rows, then a k-way merge.
pub struct ExternalMerge<R> {
    run: usize,
    buf: Vec<R>,
    buf_bytes: usize,
    dir: PathBuf,
    tag: &'static str,
    runs: Vec<PathBuf>,
    scratch: Vec<u8>,
    pub spill_bytes: u64,
}
impl<R> ExternalMerge<R> {
    pub fn new(run: usize, dir: PathBuf, tag: &'static str) -> Self {
        ExternalMerge {
            run,
            buf: Vec::new(),
            buf_bytes: 0,
            dir,
            tag,
            runs: Vec::new(),
            scratch: Vec::new(),
            spill_bytes: 0,
        }
    }
}
impl<R: SortRow> ExternalMerge<R> {
    fn spill(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        self.buf.sort_unstable_by_key(|r| r.key());
        let path = self.dir.join(format!(".run-{}-{:05}", self.tag, self.runs.len()));
        let mut w = BufWriter::with_capacity(64 << 10, File::create(&path)?);
        for r in self.buf.drain(..) {
            self.spill_bytes += spill_write(&mut w, &mut self.scratch, &r)?;
        }
        drop(w.into_inner().map_err(|e| e.into_error())?);
        self.runs.push(path);
        Ok(())
    }
}
impl<R: SortRow> Strategy<R> for ExternalMerge<R> {
    fn push(&mut self, row: R, _sink: &mut Sink) -> io::Result<()> {
        self.buf_bytes += row.approx_bytes();
        self.buf.push(row);
        if self.buf.len() >= self.run || self.buf_bytes >= BUF_BYTES {
            self.spill()?;
            self.buf_bytes = 0;
        }
        Ok(())
    }
    fn finish(&mut self, sink: &mut Sink) -> io::Result<()> {
        self.spill()?;
        self.buf = Vec::new();
        let mut readers = Vec::new();
        let mut heap = BinaryHeap::new();
        for (i, p) in self.runs.iter().enumerate() {
            let mut r = SpillReader::<R>::open(p)?;
            if let Some(row) = r.next()? {
                heap.push(Reverse(Keyed(row.key(), (i, row))));
            }
            readers.push(r);
        }
        let mut out = Vec::new();
        let mut out_bytes = 0;
        while let Some(Reverse(Keyed(_, (i, row)))) = heap.pop() {
            let f = full(&out, &mut out_bytes, &row);
            out.push(row);
            if f || out.len() >= ROW_GROUP {
                sink.write(&R::batch(&out)?)?;
                out.clear();
                out_bytes = 0;
            }
            if let Some(next) = readers[i].next()? {
                heap.push(Reverse(Keyed(next.key(), (i, next))));
            }
        }
        if !out.is_empty() {
            sink.write(&R::batch(&out)?)?;
        }
        for p in self.runs.drain(..) {
            fs::remove_file(p)?;
        }
        Ok(())
    }
}

// ---------- driver ----------

/// Visit every entry of a sealed file in order, one group in memory at a time.
pub fn stream_entries(path: &Path, mut f: impl FnMut(u64, Entry) -> io::Result<()>) -> io::Result<()> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut at = 0;
    while at < len {
        let (payload, next) = read_frame(&file, at, len, MAX_GROUP_PAYLOAD)?
            .ok_or_else(|| err("incomplete frame in sealed journal file"))?;
        let group = Group::decode(payload.as_slice()).map_err(err)?;
        drop(payload);
        let seq = group.group_sequence;
        for e in group.entries {
            f(seq, e)?;
        }
        at = next;
    }
    Ok(())
}

#[derive(Default)]
pub struct Report {
    pub late_logs: u64,
    pub late_metrics: u64,
    pub spill_bytes: u64,
    pub key_index_bytes: u64,
}

/// Streaming build with the given strategies; same commit protocol as
/// `segment::build`.
pub fn build_streaming<L: Strategy<LogRow>, M: Strategy<MetricRow>>(
    sealed: &Path,
    out_dir: &Path,
    label: u64,
    logs_s: &mut L,
    metrics_s: &mut M,
) -> io::Result<Manifest> {
    let building = out_dir.join(format!(".building-{label:020}"));
    let _ = fs::remove_dir_all(&building);
    fs::create_dir_all(&building)?;
    let mut logs = Sink::new(&building.join("logs.parquet"), logs_schema())?;
    let mut metrics = Sink::new(&building.join("metrics.parquet"), metrics_schema())?;
    let mut gaps = Sink::new(&building.join("gaps.parquet"), gaps_schema())?;
    let mut batches = Sink::new(&building.join("batches.parquet"), batches_schema())?;
    let mut chunk: Vec<(u64, Entry)> = Vec::new();
    let mut chunk_bytes = 0;
    let mut rows = Rows::default();
    let (mut first, mut last, mut records) = (None, 0, 0u64);
    let (mut rmin, mut rmax) = (u64::MAX, 0);
    let mut freshness: BTreeMap<String, u64> = BTreeMap::new();
    stream_entries(sealed, |g, entry| {
        extract(g, &entry, &mut rows)?;
        first.get_or_insert(g);
        last = g;
        records += 1;
        rmin = rmin.min(entry.received_unix_nano);
        rmax = rmax.max(entry.received_unix_nano);
        for r in rows.logs.drain(..) {
            if let Some(f) = freshness.get_mut(&r.node) {
                *f = (*f).max(r.observed_ns);
            } else {
                freshness.insert(r.node.clone(), r.observed_ns);
            }
            logs_s.push(r, &mut logs)?;
        }
        for r in rows.metrics.drain(..) {
            if let Some(f) = freshness.get_mut(&r.node) {
                *f = (*f).max(r.time_ns);
            } else {
                freshness.insert(r.node.clone(), r.time_ns);
            }
            metrics_s.push(r, &mut metrics)?;
        }
        if !rows.gaps.is_empty() {
            let g: Vec<GapRow> = rows.gaps.drain(..).collect();
            gaps.write(&gaps_batch(&g)?)?;
        }
        chunk_bytes += entry.batch.len();
        chunk.push((g, entry));
        if chunk_bytes >= BATCH_CHUNK_BYTES {
            batches.write(&batches_batch(&chunk)?)?;
            chunk.clear();
            chunk_bytes = 0;
        }
        Ok(())
    })?;
    if !chunk.is_empty() {
        batches.write(&batches_batch(&chunk)?)?;
    }
    drop(chunk);
    logs_s.finish(&mut logs)?;
    metrics_s.finish(&mut metrics)?;
    let first = first.ok_or_else(|| err("sealed journal file holds no records"))?;
    let mut files = BTreeMap::new();
    files.insert("batches.parquet".to_string(), batches.close()?);
    files.insert("logs.parquet".to_string(), logs.close()?);
    files.insert("metrics.parquet".to_string(), metrics.close()?);
    files.insert("gaps.parquet".to_string(), gaps.close()?);
    let manifest = Manifest {
        version: 1,
        journal_label: label,
        first_group: first,
        last_group: last,
        records,
        received_min_ns: rmin,
        received_max_ns: rmax,
        freshness,
        files,
    };
    let mut out = File::create(building.join("manifest.json"))?;
    out.write_all(&serde_json::to_vec_pretty(&manifest).map_err(err)?)?;
    out.sync_all()?;
    File::open(&building)?.sync_all()?;
    let dest = out_dir.join(segment_name(label));
    let _ = fs::remove_dir_all(&dest);
    fs::rename(&building, &dest)?;
    File::open(out_dir)?.sync_all()?;
    Ok(manifest)
}

/// D's first pass: exact splitters from a key-only scan, so that every
/// bucket holds at most `bucket` rows. Returns the key-index bytes used.
pub fn splitters(sealed: &Path, bucket: usize) -> io::Result<(Vec<Key>, Vec<Key>, u64)> {
    let mut lk: Vec<(Key, u32)> = Vec::new();
    let mut mk: Vec<(Key, u32)> = Vec::new();
    let mut rows = Rows::default();
    stream_entries(sealed, |g, entry| {
        extract(g, &entry, &mut rows)?;
        lk.extend(rows.logs.drain(..).map(|r| (r.key(), r.approx_bytes() as u32)));
        mk.extend(rows.metrics.drain(..).map(|r| (r.key(), r.approx_bytes() as u32)));
        rows.gaps.clear();
        Ok(())
    })?;
    let bytes = ((lk.capacity() + mk.capacity()) * std::mem::size_of::<(Key, u32)>()) as u64;
    lk.sort_unstable();
    mk.sort_unstable();
    // A new bucket starts at the row that would exceed either cap.
    let pick = |v: &[(Key, u32)]| {
        let (mut out, mut n, mut b) = (Vec::new(), 0usize, 0usize);
        for (k, w) in v {
            if n > 0 && (n >= bucket || b + *w as usize > BUF_BYTES) {
                out.push(*k);
                n = 0;
                b = 0;
            }
            n += 1;
            b += *w as usize;
        }
        out
    };
    Ok((pick(&lk), pick(&mk), bytes))
}

/// H: a reorder window for the in-order stream; rows behind the watermark
/// go to an external merge and form one second sorted sequence. Exact when
/// nothing is late; otherwise every time is covered by at most two
/// sequences, and only late rows touch the disk.
pub struct Hybrid<R> {
    heap: BinaryHeap<Reverse<Keyed<R>>>,
    window: usize,
    heap_bytes: usize,
    last: Option<Key>,
    main: Vec<R>,
    main_bytes: usize,
    pub late: ExternalMerge<R>,
    pub late_rows: u64,
}
impl<R> Hybrid<R> {
    pub fn new(window: usize, run: usize, dir: PathBuf, tag: &'static str) -> Self {
        Hybrid {
            heap: BinaryHeap::with_capacity(window + 1),
            window,
            heap_bytes: 0,
            last: None,
            main: Vec::new(),
            main_bytes: 0,
            late: ExternalMerge::new(run, dir, tag),
            late_rows: 0,
        }
    }
}
impl<R: SortRow> Hybrid<R> {
    fn emit(&mut self, k: Key, r: R, sink: &mut Sink) -> io::Result<()> {
        if self.last.is_none_or(|l| k >= l) {
            self.last = Some(k);
            let f = full(&self.main, &mut self.main_bytes, &r);
            self.main.push(r);
            if f || self.main.len() >= ROW_GROUP {
                sink.write(&R::batch(&self.main)?)?;
                self.main.clear();
                self.main_bytes = 0;
            }
            Ok(())
        } else {
            self.late_rows += 1;
            self.late.push(r, sink)
        }
    }
}
impl<R: SortRow> Strategy<R> for Hybrid<R> {
    fn push(&mut self, row: R, sink: &mut Sink) -> io::Result<()> {
        self.heap_bytes += row.approx_bytes();
        self.heap.push(Reverse(Keyed(row.key(), row)));
        while self.heap.len() > self.window || self.heap_bytes > BUF_BYTES {
            let Reverse(Keyed(k, r)) = self.heap.pop().unwrap();
            self.heap_bytes -= r.approx_bytes();
            self.emit(k, r, sink)?;
        }
        Ok(())
    }
    fn finish(&mut self, sink: &mut Sink) -> io::Result<()> {
        while let Some(Reverse(Keyed(k, r))) = self.heap.pop() {
            self.emit(k, r, sink)?;
        }
        self.heap = BinaryHeap::new();
        if !self.main.is_empty() {
            sink.write(&R::batch(&self.main)?)?;
            self.main.clear();
        }
        if self.late_rows > 0 {
            sink.cut()?;
            self.late.finish(sink)?;
        }
        Ok(())
    }
}
