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
    pub has_gaps: bool,
}

/// Process-wide state of the walk plan.
#[derive(Default)]
pub(crate) struct WalkState {
    /// Per journal file (by first group), the offset up to which frames are indexed.
    scanned: HashMap<u64, u64>,
    pub labels: Vec<String>,
    label_ids: HashMap<String, u32>,
    pub entries: Vec<TailEntry>,
    manifests: HashMap<u64, Manifest>,
    bounds: HashMap<(u64, Table), GroupBounds>,
    /// Verified text filters by label; `None` when a Segment has none or its digest
    /// differs (a named Segment's bytes never change, so the verdict is kept).
    filters: HashMap<u64, Option<Arc<Vec<GroupFilter>>>>,
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

    /// Journal files as (first group, path, length), sealed files in order, the active
    /// file last. A file whose first frame is incomplete holds no complete group yet.
    fn files(journal_dir: &Path) -> io::Result<Vec<(u64, PathBuf, u64)>> {
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
        paths.push(journal_dir.join(fabric_frame::frame::ACTIVE));
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
        let files = Self::files(journal_dir)?;
        let mut rows = Rows::default();
        for (first, path, len) in &files {
            let file = match File::open(path) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Err(interrupted("journal moved"));
                }
                Err(e) => return Err(e),
            };
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
                    extract(group.group_sequence, entry, &mut rows)?;
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
                        has_gaps: !rows.gaps.is_empty(),
                    });
                }
                at = next;
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
        Ok(files.into_iter().map(|(f, p, _)| (f, p)).collect())
    }

    /// The Segments as `segment::list` returns them, manifests read once per label.
    pub fn segments(&mut self, state_dir: &Path) -> io::Result<Vec<(u64, Manifest)>> {
        let dir = segment::segments_dir(state_dir)?;
        let mut found = Vec::new();
        for label in segment::labels(state_dir)? {
            if let Some(m) = self.manifests.get(&label) {
                found.push((label, m.clone()));
                continue;
            }
            match segment::read_manifest(&dir.join(segment::segment_name(label))) {
                Ok(m) => {
                    self.manifests.insert(label, m.clone());
                    found.push((label, m));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            }
        }
        let live: HashSet<u64> = found.iter().map(|(l, _)| *l).collect();
        self.manifests.retain(|l, _| live.contains(l));
        self.bounds.retain(|(l, _), _| live.contains(l));
        self.filters.retain(|l, _| live.contains(l));
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
            let file = match File::open(path) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Err(interrupted("journal moved"));
                }
                Err(e) => return Err(e),
            };
            let len = file.metadata()?.len();
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
        let entry = group
            .entries
            .get(e.index as usize)
            .ok_or_else(|| interrupted("journal moved"))?;
        let mut rows = Rows::default();
        extract(e.group, entry, &mut rows)?;
        Ok(rows)
    }
}
