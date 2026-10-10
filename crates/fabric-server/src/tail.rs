//! The walk plan's view of the unsealed tail and of Segment metadata
//! ([ADR-0024](../../../docs/decisions/ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md), part 1).
//!
//! The stock plan decodes every journal entry on every query. The walk plan keeps,
//! per process, a key index of the tail: one small record per entry with its group,
//! position, node label, receive time and the time bounds of its log rows and metric
//! points. A query reads the index, selects the entries its window and node can
//! match, and decodes them only when the walk reaches them in key order. The index
//! grows by reading only frames appended since the last query, and forgets entries
//! whose file is gone or whose group a Segment now covers, so it holds exactly the
//! tail a stock query would decode. Its size is about 72 bytes per entry, below the
//! rows the stock plan materialises for the same entries.
//!
//! The tail is also held as **blocks** ([ADR-0024] part 3, the format of [ADR-0023]):
//! as frames are indexed, the rows they hold are gathered per journal file, and at
//! the first frame boundary past `block_records` records they become one Zstd-compressed
//! FOB1 block with a trigram filter over its log bodies. A walk then reads a block
//! through `decode_view` in place of decoding each of its entries' OTLP bytes, and
//! skips a block whose filter lacks a needle's trigram. Blocks are derived from the
//! journal, which stays the durable custody record of every Batch's exact bytes; they
//! live only in memory, are rebuilt as the index is rebuilt after a restart, and leave
//! with their file or when a Segment covers them. A block the codec refuses (a NaN
//! point value, for one) is not made, and its entries are read one by one. Entries
//! after the last complete block of a file are read one by one too.
//!
//! [ADR-0023]: ../../../docs/decisions/ADR-0023-define-an-observation-record-with-a-canonical-encoding.md
//! [ADR-0024]: ../../../docs/decisions/ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md
//!
//! Segment manifests and row-group bounds are cached by label: a named Segment is
//! immutable (the manifest is written last and the directory renamed into place),
//! labels are never reused, and the directory is listed on every query, so a Segment
//! removed by retention leaves the answer and the cache together.

use crate::rows::{Rows, extract};
use crate::segment::{self, GroupBounds, MAX_GROUP_PAYLOAD, Manifest, Table};
use crate::store::Group;
use crate::text_filter::GroupFilter;
use fabric_frame::frame::read_frame;
use prost::Message;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) fn interrupted(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, why.to_owned())
}

fn invalid_data(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

// Reopening ACTIVE may reach a different file after rotation. Verify the first
// committed group before using any saved offset, then follow its sealed name.
// File handles live only for this discovery/read operation, never in Sources.
fn open_journal(path: &Path, expected_first: u64) -> io::Result<(File, u64)> {
    fn candidate(path: &Path, expected: u64) -> io::Result<Option<(File, u64)>> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let len = file.metadata()?.len();
        let Some((payload, _)) = read_frame(&file, 0, len, MAX_GROUP_PAYLOAD)? else {
            return Ok(None);
        };
        let first = Group::decode(payload.as_slice())
            .map_err(invalid_data)?
            .group_sequence;
        Ok((first == expected).then_some((file, len)))
    }
    if let Some(opened) = candidate(path, expected_first)? {
        return Ok(opened);
    }
    let sealed = path.with_file_name(format!("sealed-{expected_first:020}.faj"));
    if sealed != path
        && let Some(opened) = candidate(&sealed, expected_first)?
    {
        return Ok(opened);
    }
    Err(interrupted("journal moved"))
}

/// No rows of that kind: the bounds are empty.
pub(crate) const NONE: (u64, u64) = (u64::MAX, 0);

fn bounds(it: impl Iterator<Item = u64>) -> (u64, u64) {
    it.fold(NONE, |(lo, hi), t| (lo.min(t), hi.max(t)))
}

/// One journal entry as the walk sees it before decoding.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TailEntry {
    pub group: u64,
    /// The journal file, by the group of its first frame, and the frame's offset.
    pub file_first: u64,
    pub offset: u64,
    /// Position of the entry in its frame's group.
    pub index: u32,
    pub label: u32,
    pub received: u64,
    pub logs: (u64, u64),
    pub metrics: (u64, u64),
    pub spans: (u64, u64),
    pub has_gaps: bool,
    /// The block that holds this entry's rows, once one is complete.
    pub block: Option<u32>,
}

/// Records gathered into one block before it closes, by default.
pub(crate) const BLOCK_RECORDS: usize = 4096;

/// One entry's place in a block.
#[derive(Clone, Debug)]
pub(crate) struct BlockEntry {
    pub offset: u64,
    pub index: u32,
    /// Position of the entry's first record in the block.
    pub start: usize,
    pub node: String,
    pub group: u64,
}

/// Consecutive frames of one journal file as a compressed FOB1 block.
pub(crate) struct TailBlock {
    pub zst: Vec<u8>,
    pub raw_len: usize,
    /// In record order.
    pub entries: Vec<BlockEntry>,
    /// Trigram filters over the block's log bodies and over its spans' hex trace IDs.
    pub filter: GroupFilter,
    pub trace_filter: GroupFilter,
    file_first: u64,
}

/// Rows of one journal file gathered since its last block closed.
#[derive(Default)]
struct Pending {
    records: Vec<fabric_observation::Observation>,
    entries: Vec<BlockEntry>,
    /// A row FOB1 cannot carry (a span with a malformed identity or an unknown
    /// kind or status): the block is not made and its entries are read one by one.
    refused: bool,
}

fn unhex<const N: usize>(text: &str) -> Option<[u8; N]> {
    if text.len() != 2 * N {
        return None;
    }
    let mut out = [0_u8; N];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(text.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

fn span_record(
    r: &crate::rows::SpanRow,
    generation: u64,
) -> Option<fabric_observation::Observation> {
    use fabric_observation as fo;
    let parent_span_id = if r.parent_span_id.is_empty() {
        None
    } else {
        Some(unhex::<8>(&r.parent_span_id)?)
    };
    let kind = match r.kind {
        0 => fo::SpanKind::Unspecified,
        1 => fo::SpanKind::Internal,
        2 => fo::SpanKind::Server,
        3 => fo::SpanKind::Client,
        4 => fo::SpanKind::Producer,
        5 => fo::SpanKind::Consumer,
        _ => return None,
    };
    let status = match r.status {
        0 => fo::Status::Unset,
        1 => fo::Status::Ok,
        2 => fo::Status::Error,
        _ => return None,
    };
    Some(fo::Observation {
        strand: fo::Strand {
            node_id: r.node_id,
            generation,
        },
        sequence: r.sequence,
        index: r.index,
        time_ns: r.start_ns,
        locators: Some(fo::Locators {
            trace_id: unhex::<16>(&r.trace_id)?,
            span_id: unhex::<8>(&r.span_id)?,
            parent_span_id,
        }),
        attributes: r
            .attributes
            .iter()
            .map(|(k, v)| (k.clone(), fo::Value::Str(v.clone())))
            .collect(),
        signal: fo::Signal::Span {
            name: r.name.clone(),
            end_ns: r.end_ns,
            status,
            kind,
        },
    })
}

impl Pending {
    fn push(
        &mut self,
        rows: &Rows,
        offset: u64,
        index: u32,
        node: &str,
        group: u64,
        generation: u64,
    ) {
        use fabric_observation as fo;
        self.entries.push(BlockEntry {
            offset,
            index,
            start: self.records.len(),
            node: node.to_owned(),
            group,
        });
        let attributes = |a: &crate::rows::Attributes| {
            a.iter()
                .map(|(k, v)| (k.clone(), fo::Value::Str(v.clone())))
                .collect()
        };
        for r in &rows.logs {
            self.records.push(fo::Observation {
                strand: fo::Strand {
                    node_id: r.node_id,
                    generation,
                },
                sequence: r.sequence,
                index: r.index,
                time_ns: r.observed_ns,
                locators: None,
                attributes: attributes(&r.attributes),
                signal: fo::Signal::Log {
                    severity: 0,
                    event: String::new(),
                    body: r.body.clone(),
                },
            });
        }
        for r in &rows.spans {
            match span_record(r, generation) {
                Some(record) => self.records.push(record),
                None => self.refused = true,
            }
        }
        for r in &rows.metrics {
            self.records.push(fo::Observation {
                strand: fo::Strand {
                    node_id: r.node_id,
                    generation,
                },
                sequence: r.sequence,
                index: r.index,
                time_ns: r.time_ns,
                locators: None,
                attributes: attributes(&r.attributes),
                signal: fo::Signal::Point {
                    name: r.name.clone(),
                    unit: r.unit.clone(),
                    kind: if r.sum {
                        fo::PointKind::Sum {
                            monotonic: r.monotonic,
                            start_ns: r.start_ns,
                        }
                    } else {
                        fo::PointKind::Gauge
                    },
                    value: match r.value {
                        crate::rows::Number::Int(v) => fo::Number::Int(v),
                        crate::rows::Number::Double(v) => fo::Number::Double(v),
                    },
                },
            });
        }
    }

    /// The block of everything gathered, or `None` if the codec refuses it.
    fn close(&mut self, file_first: u64) -> Option<TailBlock> {
        let records = std::mem::take(&mut self.records);
        let entries = std::mem::take(&mut self.entries);
        if std::mem::take(&mut self.refused) {
            return None;
        }
        let trace_ids: Vec<String> = records
            .iter()
            .filter_map(|r| r.locators.map(|l| crate::rows::hex(&l.trace_id)))
            .collect();
        let trace_filter = GroupFilter::build(trace_ids.iter().map(String::as_str));
        let raw = fabric_observation::encode(&records).ok()?;
        let filter = GroupFilter::build(records.iter().filter_map(|r| match &r.signal {
            fabric_observation::Signal::Log { body, .. } => Some(body.as_str()),
            _ => None,
        }));
        let zst = zstd::bulk::compress(&raw, 3).ok()?;
        Some(TailBlock {
            zst,
            raw_len: raw.len(),
            entries,
            filter,
            trace_filter,
            file_first,
        })
    }
}

/// Query ownership of immutable metadata. Neither variant pins Segment files.
pub(crate) enum ManifestHandle {
    Owned(Manifest),
    Shared(Arc<Manifest>),
}

impl ManifestHandle {
    fn from_cached(manifest: &Arc<Manifest>, shared: bool) -> Self {
        if shared {
            Self::Shared(manifest.clone())
        } else {
            Self::Owned(manifest.as_ref().clone())
        }
    }
}

impl std::ops::Deref for ManifestHandle {
    type Target = Manifest;
    fn deref(&self) -> &Manifest {
        match self {
            Self::Owned(m) => m,
            Self::Shared(m) => m,
        }
    }
}

/// Process-wide state of the walk plan.
#[derive(Default)]
pub(crate) struct WalkState {
    /// Per journal file (by first group), the offset up to which frames are indexed.
    scanned: HashMap<u64, u64>,
    pub labels: Vec<String>,
    label_ids: HashMap<String, u32>,
    pub entries: Vec<TailEntry>,
    manifests: HashMap<u64, Arc<Manifest>>,
    bounds: HashMap<(u64, Table), GroupBounds>,
    /// Verified text filters by label; `None` when a Segment has none or its digest
    /// differs (a named Segment's bytes never change, so the verdict is kept).
    filters: HashMap<u64, Option<Arc<Vec<GroupFilter>>>>,
    spans_filters: HashMap<u64, Option<Arc<Vec<GroupFilter>>>>,
    /// Complete tail blocks by id, the rows of each file not yet in one, and the size
    /// at which a block closes (0 means `BLOCK_RECORDS`).
    pub blocks: HashMap<u32, Arc<TailBlock>>,
    next_block: u32,
    pending: HashMap<u64, Pending>,
    pub block_records: usize,
}

impl WalkState {
    fn label_id(&mut self, label: &str) -> u32 {
        if let Some(&id) = self.label_ids.get(label) {
            return id;
        }
        let id = self.labels.len() as u32;
        self.labels.push(label.to_owned());
        self.label_ids.insert(label.to_owned(), id);
        id
    }

    /// Capture ACTIVE before listing sealed files, so rotation cannot hide the
    /// old active file between the listing and its open. Two handles at most:
    /// the captured active file and the sealed file currently being inspected.
    fn files(journal_dir: &Path) -> io::Result<Vec<(u64, PathBuf, u64)>> {
        let active_path = journal_dir.join(fabric_frame::frame::ACTIVE);
        let active = match File::open(&active_path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(interrupted("journal moved"));
            }
            Err(error) => return Err(error),
        };
        let active_len = active.metadata()?.len();
        let mut paths: Vec<PathBuf> = std::fs::read_dir(journal_dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("sealed-") && n.ends_with(".faj"))
            })
            .collect();
        paths.sort();
        let mut out = Vec::new();
        for path in paths {
            let file = match File::open(&path) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Err(interrupted("journal moved"));
                }
                Err(e) => return Err(e),
            };
            let len = file.metadata()?.len();
            let Some((payload, _)) = read_frame(&file, 0, len, MAX_GROUP_PAYLOAD)? else {
                continue;
            };
            let first = Group::decode(payload.as_slice())
                .map_err(invalid_data)?
                .group_sequence;
            out.push((first, path, len));
        }
        if let Some((payload, _)) = read_frame(&active, 0, active_len, MAX_GROUP_PAYLOAD)? {
            let first = Group::decode(payload.as_slice())
                .map_err(invalid_data)?
                .group_sequence;
            // A rotation may have listed the captured active file as sealed.
            // Prefer that immutable pathname and index its frames only once.
            if !out.iter().any(|(listed, _, _)| *listed == first) {
                out.push((first, active_path, active_len));
            }
        }
        Ok(out)
    }

    /// Index every complete frame not indexed yet; forget entries whose file is gone
    /// or whose group a Segment covers (`covered` sorted by first group). Returns the
    /// live journal files by first group.
    pub fn extend(
        &mut self,
        journal_dir: &Path,
        covered: &[(u64, u64)],
    ) -> io::Result<HashMap<u64, PathBuf>> {
        self.extend_inner(journal_dir, covered, || {})
    }

    #[cfg(test)]
    pub(crate) fn extend_at_discovery_cut(
        &mut self,
        journal_dir: &Path,
        covered: &[(u64, u64)],
        after_discovery: impl FnOnce(),
    ) -> io::Result<HashMap<u64, PathBuf>> {
        self.extend_inner(journal_dir, covered, after_discovery)
    }

    fn extend_inner(
        &mut self,
        journal_dir: &Path,
        covered: &[(u64, u64)],
        after_discovery: impl FnOnce(),
    ) -> io::Result<HashMap<u64, PathBuf>> {
        let files = Self::files(journal_dir)?;
        after_discovery();
        let mut rows = Rows::default();
        for (first, path, len) in &files {
            let (file, verified_len) = open_journal(path, *first)?;
            if verified_len < *len {
                return Err(invalid_data("verified journal file shrank after discovery"));
            }
            let mut at = self.scanned.get(first).copied().unwrap_or(0);
            while at < *len {
                let Some((payload, next)) = read_frame(&file, at, *len, MAX_GROUP_PAYLOAD)? else {
                    break; // an append in progress
                };
                let group = Group::decode(payload.as_slice()).map_err(invalid_data)?;
                for (i, entry) in group.entries.iter().enumerate() {
                    rows.logs.clear();
                    rows.metrics.clear();
                    rows.gaps.clear();
                    rows.spans.clear();
                    extract(group.group_sequence, entry, &mut rows)?;
                    let generation = fabric_frame::envelope::Batch::decode(entry.batch.as_slice())
                        .map(|b| b.generation)
                        .unwrap_or(0);
                    self.pending.entry(*first).or_default().push(
                        &rows,
                        at,
                        i as u32,
                        &entry.label,
                        group.group_sequence,
                        generation,
                    );
                    let label = self.label_id(&entry.label);
                    self.entries.push(TailEntry {
                        group: group.group_sequence,
                        file_first: *first,
                        offset: at,
                        index: i as u32,
                        label,
                        received: entry.received_unix_nano,
                        logs: bounds(rows.logs.iter().map(|r| r.observed_ns)),
                        metrics: bounds(rows.metrics.iter().map(|r| r.time_ns)),
                        spans: bounds(rows.spans.iter().map(|r| r.start_ns)),
                        has_gaps: !rows.gaps.is_empty(),
                        block: None,
                    });
                }
                at = next;
                let limit = if self.block_records == 0 {
                    BLOCK_RECORDS
                } else {
                    self.block_records
                };
                if self
                    .pending
                    .get(first)
                    .is_some_and(|p| p.records.len() >= limit)
                {
                    self.close_block(*first);
                }
            }
            self.scanned.insert(*first, at);
        }
        let live: HashSet<u64> = files.iter().map(|(f, _, _)| *f).collect();
        self.scanned.retain(|f, _| live.contains(f));
        let covers = |g: u64| {
            let i = covered.partition_point(|(a, _)| *a <= g);
            i > 0 && covered[i - 1].1 >= g
        };
        self.entries
            .retain(|e| live.contains(&e.file_first) && !covers(e.group));
        self.blocks.retain(|_, b| {
            live.contains(&b.file_first) && !b.entries.first().is_some_and(|e| covers(e.group))
        });
        self.pending
            .retain(|f, p| live.contains(f) && !p.entries.first().is_some_and(|e| covers(e.group)));
        Ok(files.into_iter().map(|(f, p, _)| (f, p)).collect())
    }

    /// Close the pending rows of one file into a block and mark its entries.
    fn close_block(&mut self, file_first: u64) {
        let Some(pending) = self.pending.get_mut(&file_first) else {
            return;
        };
        let Some(block) = pending.close(file_first) else {
            return;
        };
        let id = self.next_block;
        self.next_block = self.next_block.wrapping_add(1);
        let keys: HashSet<(u64, u32)> = block.entries.iter().map(|e| (e.offset, e.index)).collect();
        let mut left = keys.len();
        for e in self.entries.iter_mut().rev() {
            if left == 0 {
                break;
            }
            if e.file_first == file_first && keys.contains(&(e.offset, e.index)) {
                e.block = Some(id);
                left -= 1;
            }
        }
        self.blocks.insert(id, Arc::new(block));
    }

    /// List live immutable metadata, selecting deep-copy or shared query ownership.
    pub fn segments(
        &mut self,
        state_dir: &Path,
        shared: bool,
    ) -> io::Result<Vec<(u64, ManifestHandle)>> {
        let dir = segment::segments_dir(state_dir)?;
        let mut found = Vec::new();
        for label in segment::labels(state_dir)? {
            if let Some(m) = self.manifests.get(&label) {
                found.push((label, ManifestHandle::from_cached(m, shared)));
                continue;
            }
            match segment::read_manifest(&dir.join(segment::segment_name(label))) {
                Ok(m) => {
                    let m = Arc::new(m);
                    self.manifests.insert(label, m.clone());
                    found.push((label, ManifestHandle::from_cached(&m, shared)));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            }
        }
        let live: HashSet<u64> = found.iter().map(|(l, _)| *l).collect();
        self.manifests.retain(|l, _| live.contains(l));
        self.bounds.retain(|(l, _), _| live.contains(l));
        self.filters.retain(|l, _| live.contains(l));
        self.spans_filters.retain(|l, _| live.contains(l));
        Ok(found)
    }

    /// Row-group bounds of one Segment's table, read once per label. Errors are not
    /// cached: an unreadable Segment is reported unavailable on every query.
    pub fn bounds(
        &mut self,
        dir: &Path,
        manifest: &Manifest,
        table: Table,
    ) -> io::Result<GroupBounds> {
        let key = (manifest.journal_label, table);
        if let Some(b) = self.bounds.get(&key) {
            return Ok(b.clone());
        }
        let b = segment::row_group_bounds(dir, manifest, table)?;
        self.bounds.insert(key, b.clone());
        Ok(b)
    }
}

impl WalkState {
    /// The verified text filters of one Segment's logs row groups, read once per label.
    pub fn spans_filters(
        &mut self,
        dir: &Path,
        manifest: &Manifest,
    ) -> Option<Arc<Vec<GroupFilter>>> {
        self.spans_filters
            .entry(manifest.journal_label)
            .or_insert_with(|| segment::read_spans_filter(dir, manifest).map(Arc::new))
            .clone()
    }

    pub fn filters(&mut self, dir: &Path, manifest: &Manifest) -> Option<Arc<Vec<GroupFilter>>> {
        self.filters
            .entry(manifest.journal_label)
            .or_insert_with(|| segment::read_text_filter(dir, manifest).map(Arc::new))
            .clone()
    }
}

/// Reads selected tail entries, keeping the last 64 decoded frames: in key order
/// consecutive entries interleave among a few frames, and re-decoding a frame per
/// entry was the walk's whole overhead when nothing stops it (hypothesis A4).
pub(crate) struct TailReader<'a> {
    paths: &'a HashMap<u64, PathBuf>,
    frames: HashMap<(u64, u64), Group>,
}

const FRAME_CACHE: usize = 64;

impl<'a> TailReader<'a> {
    pub fn new(paths: &'a HashMap<u64, PathBuf>) -> Self {
        Self {
            paths,
            frames: HashMap::new(),
        }
    }

    /// The decoded group of one frame, from the cache or the file.
    pub fn group(&mut self, file_first: u64, offset: u64) -> io::Result<&Group> {
        if !self.frames.contains_key(&(file_first, offset)) {
            if self.frames.len() >= FRAME_CACHE {
                self.frames.clear();
            }
            let path = self
                .paths
                .get(&file_first)
                .ok_or_else(|| interrupted("journal moved"))?;
            let (file, len) = open_journal(path, file_first)?;
            if offset > len {
                return Err(invalid_data(
                    "tail frame offset beyond verified journal file",
                ));
            }
            let Some((payload, _)) = read_frame(&file, offset, len, MAX_GROUP_PAYLOAD)? else {
                return Err(interrupted("journal moved"));
            };
            let group = Group::decode(payload.as_slice()).map_err(invalid_data)?;
            self.frames.insert((file_first, offset), group);
        }
        Ok(&self.frames[&(file_first, offset)])
    }

    /// The rows of one entry.
    pub fn rows(&mut self, e: &TailEntry) -> io::Result<Rows> {
        let group = self.group(e.file_first, e.offset)?;
        if group.group_sequence != e.group {
            return Err(invalid_data("tail entry group identity mismatch"));
        }
        let entry = group
            .entries
            .get(e.index as usize)
            .ok_or_else(|| interrupted("journal moved"))?;
        let mut rows = Rows::default();
        extract(e.group, entry, &mut rows)?;
        Ok(rows)
    }
}

/// Visit every record of the selected entries (`wanted`, by frame offset and entry
/// index) of one block, in block order, with the entry it belongs to.
pub(crate) fn visit_block(
    block: &TailBlock,
    wanted: &HashSet<(u64, u32)>,
    dctx: &mut zstd::bulk::Decompressor<'_>,
    mut visit: impl FnMut(&BlockEntry, &fabric_observation::ObservationRef<'_>),
) -> io::Result<()> {
    let raw = dctx.decompress(&block.zst, block.raw_len)?;
    let view = fabric_observation::decode_view(&raw).map_err(invalid_data)?;
    let mut at = 0_usize;
    for (i, record) in view.iter().enumerate() {
        while at + 1 < block.entries.len() && block.entries[at + 1].start <= i {
            at += 1;
        }
        let entry = &block.entries[at];
        if wanted.contains(&(entry.offset, entry.index)) {
            visit(entry, record);
        }
    }
    Ok(())
}

fn attributes(record: &fabric_observation::ObservationRef<'_>) -> crate::rows::Attributes {
    record
        .attributes
        .iter()
        .filter_map(|(k, v)| match v {
            fabric_observation::ValueRef::Str(s) => Some(((*k).to_owned(), (*s).to_owned())),
            _ => None,
        })
        .collect()
}

/// The log row a block record stands for.
pub(crate) fn log_row(
    entry: &BlockEntry,
    record: &fabric_observation::ObservationRef<'_>,
    body: &str,
) -> crate::rows::LogRow {
    crate::rows::LogRow {
        group: entry.group,
        node: entry.node.clone(),
        node_id: record.strand.node_id,
        sequence: record.sequence,
        index: record.index,
        observed_ns: record.time_ns,
        body: body.to_owned(),
        attributes: attributes(record),
    }
}

/// The metric point a block record stands for, if it is one.
pub(crate) fn metric_row(
    entry: &BlockEntry,
    record: &fabric_observation::ObservationRef<'_>,
) -> Option<crate::rows::MetricRow> {
    use fabric_observation::{Number, PointKind, SignalRef};
    let SignalRef::Point {
        name,
        unit,
        kind,
        value,
    } = record.signal
    else {
        return None;
    };
    let (sum, monotonic, start_ns) = match kind {
        PointKind::Gauge => (false, false, 0),
        PointKind::Sum {
            monotonic,
            start_ns,
        } => (true, monotonic, start_ns),
    };
    Some(crate::rows::MetricRow {
        group: entry.group,
        node: entry.node.clone(),
        node_id: record.strand.node_id,
        sequence: record.sequence,
        index: record.index,
        name: name.to_owned(),
        unit: unit.to_owned(),
        sum,
        monotonic,
        time_ns: record.time_ns,
        start_ns,
        value: match value {
            Number::Int(v) => crate::rows::Number::Int(v),
            Number::Double(v) => crate::rows::Number::Double(v),
        },
        attributes: attributes(record),
    })
}

/// The span row a block record stands for, if it is one.
pub(crate) fn span_row(
    entry: &BlockEntry,
    record: &fabric_observation::ObservationRef<'_>,
) -> Option<crate::rows::SpanRow> {
    use fabric_observation::{SignalRef, SpanKind, Status};
    let SignalRef::Span {
        name,
        end_ns,
        status,
        kind,
    } = record.signal
    else {
        return None;
    };
    let locators = record.locators?;
    Some(crate::rows::SpanRow {
        group: entry.group,
        node: entry.node.clone(),
        node_id: record.strand.node_id,
        sequence: record.sequence,
        index: record.index,
        trace_id: crate::rows::hex(&locators.trace_id),
        span_id: crate::rows::hex(&locators.span_id),
        parent_span_id: locators
            .parent_span_id
            .map(|p| crate::rows::hex(&p))
            .unwrap_or_default(),
        name: name.to_owned(),
        kind: match kind {
            SpanKind::Unspecified => 0,
            SpanKind::Internal => 1,
            SpanKind::Server => 2,
            SpanKind::Client => 3,
            SpanKind::Producer => 4,
            SpanKind::Consumer => 5,
        },
        status: match status {
            Status::Unset => 0,
            Status::Ok => 1,
            Status::Error => 2,
        },
        start_ns: record.time_ns,
        end_ns,
        attributes: attributes(record),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rows::{MetricRow, Number};

    fn point(value: Number) -> Rows {
        let mut rows = Rows::default();
        rows.metrics.push(MetricRow {
            group: 1,
            node: "n".into(),
            node_id: [7; 16],
            sequence: 1,
            index: 0,
            name: "m".into(),
            unit: "1".into(),
            sum: false,
            monotonic: false,
            time_ns: 10,
            start_ns: 0,
            value,
            attributes: Default::default(),
        });
        rows
    }

    #[test]
    fn a_block_the_codec_refuses_is_not_made() {
        let mut pending = Pending::default();
        pending.push(&point(Number::Double(1.5)), 0, 0, "n", 1, 0);
        assert!(pending.close(1).is_some());
        let mut pending = Pending::default();
        pending.push(&point(Number::Double(f64::NAN)), 0, 0, "n", 1, 0);
        assert!(
            pending.close(1).is_none(),
            "a NaN point value cannot be a canonical block"
        );
    }
}

#[cfg(test)]
#[path = "tail_identity_tests.rs"]
mod tail_identity_tests;
