//! Exact queries over retained history (retained-history contract).
//!
//! A snapshot is the group range `[oldest retained, newest committed]` at the
//! first page. Records never change group, only move from the journal into a
//! segment, so a later page recomputes the same set from wherever the records
//! now live. If retention removed part of the range, the page answers Gone.
//! Rows are kept in a bounded heap of `limit + 1`, so memory does not grow
//! with the number of matching rows. What the window, page, snapshot and
//! counter rules mean is decided by `fabric_core::query`.
//!
//! Two plans answer the same query with the same rows ([ADR-0024], part 1). The
//! scan plan, the default, decodes every journal entry and reads every row group the
//! window's statistics admit. The walk plan orders the sources (tail entries and
//! row groups) by the smallest key each can hold and stops once the heap is full and
//! its largest key is below the next source's bound: the threshold rule of
//! `fabric_core::query::spec::threshold_walk`. It reads the tail through a key index
//! and caches Segment metadata (see `tail`).
//!
//! [ADR-0024]: ../../../docs/decisions/ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md

use crate::rows::{GapRow, LogRow, MetricRow, Number, Rows, extract};
use crate::segment::{self, GroupBounds, MAX_GROUP_PAYLOAD, Manifest, Table};
use crate::store::Group;
use crate::tail::{TailEntry, TailReader, WalkState, interrupted};
use fabric_core::query::{
    self as kernel, CounterPoint, CounterStep, QueryRejection, Snapshot, Window,
};
use fabric_frame::frame::read_frame;
use prost::Message;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BinaryHeap, HashMap};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Query {
    Logs {
        #[serde(default)]
        node: Option<String>,
        from_ns: u64,
        to_ns: u64,
        #[serde(default)]
        contains: Option<String>,
        limit: u32,
        #[serde(default)]
        page: Option<String>,
    },
    Metrics {
        #[serde(default)]
        node: Option<String>,
        name: String,
        from_ns: u64,
        to_ns: u64,
        limit: u32,
        #[serde(default)]
        page: Option<String>,
    },
    Rate {
        #[serde(default)]
        node: Option<String>,
        name: String,
        from_ns: u64,
        to_ns: u64,
    },
}

#[derive(Debug)]
pub enum QueryError {
    Invalid(String),
    /// A page's snapshot is no longer retained.
    Gone,
    Io(io::Error),
}

impl From<io::Error> for QueryError {
    fn from(error: io::Error) -> Self {
        QueryError::Io(error)
    }
}

type Key = kernel::RowKey;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex16(text: &str) -> Option<[u8; 16]> {
    if text.len() != 32 {
        return None;
    }
    let mut out = [0_u8; 16];
    for (i, chunk) in text.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(out)
}

/// Page tokens are hex of a small JSON document; opaque to callers.
fn encode_token(value: &Value) -> String {
    hex(value.to_string().as_bytes())
}

fn decode_token(token: &str) -> Option<Value> {
    if !token.len().is_multiple_of(2) || token.len() > 4096 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..token.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(token.get(i..i + 2)?, 16).ok())
        .collect();
    serde_json::from_slice(&bytes?).ok()
}

/// Everything a query reads, fixed for one snapshot.
struct Sources {
    segments: Vec<(PathBuf, Manifest)>,
    /// Journal groups inside the snapshot that no segment covers.
    journal: Vec<Group>,
    oldest_group: u64,
    /// Walk plan: selected tail entries left undecoded until the walk reaches them,
    /// the live journal files, receive bounds and freshness over the whole tail of the
    /// snapshot (which `journal` then no longer holds), and each Segment's row-group
    /// bounds for the queried table, or the error that made them unreadable.
    tail: Vec<TailEntry>,
    tail_paths: HashMap<u64, PathBuf>,
    tail_evidence: Option<((u64, u64), BTreeMap<String, u64>)>,
    bounds: Vec<Result<GroupBounds, String>>,
    /// Walk plan, logs queries with a needle of three or more bytes: each Segment's
    /// verified row-group text filters, if it has them (ADR-0024 part 2).
    filters: Vec<Option<std::sync::Arc<Vec<crate::text_filter::GroupFilter>>>>,
}

/// One source of the walk: a tail entry, or a row group of a Segment.
#[derive(Clone, Copy)]
enum Source {
    Tail(usize),
    Group(usize, usize),
}

/// How a query reads its sources; both plans return the same answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Plan {
    /// Decode every tail entry, read every row group the window admits.
    #[default]
    Scan,
    /// Read sources in key order and stop at the heap's threshold (ADR-0024).
    Walk,
}

fn number_json(value: Number) -> Value {
    match value {
        Number::Int(v) => json!(v),
        Number::Double(v) => json!(v),
    }
}

fn log_json(r: &LogRow) -> Value {
    json!({
        "node": r.node, "node_id": hex(&r.node_id), "sequence": r.sequence, "index": r.index,
        "observed_ns": r.observed_ns, "body": r.body, "attributes": r.attributes,
    })
}

fn metric_json(r: &MetricRow) -> Value {
    let mut row = json!({
        "node": r.node, "node_id": hex(&r.node_id), "sequence": r.sequence, "index": r.index,
        "name": r.name, "unit": r.unit, "kind": if r.sum { "sum" } else { "gauge" },
        "time_ns": r.time_ns, "start_ns": r.start_ns,
        "value": number_json(r.value), "attributes": r.attributes,
    });
    if r.sum {
        row["monotonic"] = json!(r.monotonic);
    }
    row
}

pub struct History {
    state_dir: PathBuf,
    plan: Plan,
    walk: Mutex<WalkState>,
}

/// Keeps the `capacity` smallest rows by key.
struct Smallest<T> {
    heap: BinaryHeap<(Key, usize)>,
    rows: BTreeMap<usize, T>,
    next: usize,
    capacity: usize,
}

impl<T> Smallest<T> {
    fn new(capacity: usize) -> Self {
        Self {
            heap: BinaryHeap::new(),
            rows: BTreeMap::new(),
            next: 0,
            capacity,
        }
    }
    fn offer(&mut self, key: Key, row: T) {
        if self.heap.len() == self.capacity {
            if self.heap.peek().is_some_and(|(top, _)| key >= *top) {
                return;
            }
            let (_, id) = self.heap.pop().unwrap();
            self.rows.remove(&id);
        }
        self.heap.push((key, self.next));
        self.rows.insert(self.next, row);
        self.next += 1;
    }
    /// The largest held key once full: no key at or above it can enter.
    fn threshold(&self) -> Option<Key> {
        if self.heap.len() == self.capacity {
            self.heap.peek().map(|(k, _)| *k)
        } else {
            None
        }
    }
    fn sorted(self) -> Vec<(Key, T)> {
        let mut keyed: Vec<(Key, usize)> = self.heap.into_vec();
        keyed.sort();
        let mut rows = self.rows;
        keyed
            .into_iter()
            .map(|(k, id)| (k, rows.remove(&id).unwrap()))
            .collect()
    }
}

impl History {
    pub fn new(state_dir: &Path) -> Self {
        Self::with_plan(state_dir, Plan::Scan)
    }

    pub fn with_plan(state_dir: &Path, plan: Plan) -> Self {
        Self {
            state_dir: state_dir.to_path_buf(),
            plan,
            walk: Mutex::new(WalkState::default()),
        }
    }

    /// Walk plan: the snapshot's sources from the tail index and the metadata cache.
    /// Entries are selected by their bounds for the queried kind and by node; those
    /// with gaps inside the window are decoded now (the answer's `gaps` needs them),
    /// the rest wait for the walk.
    fn sources_walk(
        &self,
        oldest: Option<u64>,
        newest: u64,
        query: &Query,
        window: &Window,
        node: &Option<String>,
    ) -> io::Result<Sources> {
        let mut state = self.walk.lock().unwrap_or_else(|e| e.into_inner());
        let segments_dir = segment::segments_dir(&self.state_dir)?;
        let mut covered = Vec::new();
        let mut segments = Vec::new();
        for (label, manifest) in state.segments(&self.state_dir)? {
            if manifest.first_group > newest {
                continue;
            }
            covered.push((manifest.first_group, manifest.last_group));
            segments.push((segments_dir.join(segment::segment_name(label)), manifest));
        }
        covered.sort_unstable();
        let tail_paths = state.extend(&self.state_dir.join("journal"), &covered)?;
        let oldest_group = segments
            .iter()
            .map(|(_, m)| m.first_group)
            .chain(
                state
                    .entries
                    .iter()
                    .map(|e| e.group)
                    .filter(|g| *g <= newest),
            )
            .min()
            .unwrap_or(newest + 1);
        let floor = oldest.unwrap_or(oldest_group);
        segments.retain(|(_, m)| m.last_group >= floor);
        let kind = |e: &TailEntry| match query {
            Query::Logs { .. } => e.logs,
            Query::Metrics { .. } | Query::Rate { .. } => e.metrics,
        };
        let mut received = (u64::MAX, 0_u64);
        let mut freshness: BTreeMap<String, u64> = BTreeMap::new();
        let mut lazy = Vec::new();
        let mut eager = Vec::new();
        for e in &state.entries {
            if e.group > newest || e.group < floor {
                continue;
            }
            received = (received.0.min(e.received), received.1.max(e.received));
            let label = &state.labels[e.label as usize];
            let newest_time = e.logs.1.max(e.metrics.1);
            if e.logs != crate::tail::NONE || e.metrics != crate::tail::NONE {
                let f = freshness.entry(label.clone()).or_default();
                *f = (*f).max(newest_time);
            }
            if node.as_deref().is_some_and(|n| n != label) {
                continue;
            }
            if e.has_gaps && window.contains(e.received) {
                eager.push(*e);
            } else {
                let (min, max) = kind(e);
                if max >= window.from_ns && min < window.to_ns {
                    lazy.push(*e);
                }
            }
        }
        let table = match query {
            Query::Logs { .. } => Table::Logs,
            Query::Metrics { .. } | Query::Rate { .. } => Table::Metrics,
        };
        let bounds = segments
            .iter()
            .map(|(dir, manifest)| {
                state
                    .bounds(dir, manifest, table)
                    .map_err(|e| e.to_string())
            })
            .collect();
        let needle = matches!(query, Query::Logs { contains: Some(c), .. } if c.len() >= 3);
        let filters = if needle {
            segments
                .iter()
                .map(|(dir, manifest)| state.filters(dir, manifest))
                .collect()
        } else {
            Vec::new()
        };
        drop(state);
        let mut reader = TailReader::new(&tail_paths);
        let mut journal: Vec<Group> = Vec::new();
        for e in &eager {
            let entry = reader
                .group(e.file_first, e.offset)?
                .entries
                .get(e.index as usize)
                .ok_or_else(|| interrupted("journal moved"))?
                .clone();
            match journal.last_mut() {
                Some(g) if g.group_sequence == e.group => g.entries.push(entry),
                _ => journal.push(Group {
                    group_sequence: e.group,
                    entries: vec![entry],
                }),
            }
        }
        Ok(Sources {
            segments,
            journal,
            oldest_group,
            tail: lazy,
            tail_paths,
            tail_evidence: Some((received, freshness)),
            bounds,
            filters,
        })
    }

    /// Walk plan: every source that can hold a row of the window, in order of the
    /// smallest key it can hold. A Segment whose bounds could not be read is reported
    /// unavailable, as the scan reports a Segment it cannot read.
    fn walk_order(
        sources: &Sources,
        window: &Window,
        kind: impl Fn(&TailEntry) -> (u64, u64),
        needle: Option<&str>,
        unavailable: &mut Vec<Value>,
    ) -> io::Result<Vec<(u64, u64, Source)>> {
        let mut items: Vec<(u64, u64, Source)> = sources
            .tail
            .iter()
            .enumerate()
            .map(|(i, e)| (kind(e).0, kind(e).1, Source::Tail(i)))
            .collect();
        for (si, ((dir, manifest), bounds)) in
            sources.segments.iter().zip(&sources.bounds).enumerate()
        {
            match bounds {
                Ok(groups) => {
                    // A row group whose verified filter lacks one of the needle's
                    // trigrams holds no match (ADR-0024 part 2).
                    let filter = sources.filters.get(si).and_then(|f| f.as_deref());
                    let excluded = |rg: usize| {
                        needle.zip(filter).is_some_and(|(n, f)| {
                            f.get(rg).is_some_and(|g| !g.may_contain(n.as_bytes()))
                        })
                    };
                    for &(rg, min, max) in groups {
                        if max >= window.from_ns && min < window.to_ns && !excluded(rg) {
                            items.push((min, max, Source::Group(si, rg)));
                        }
                    }
                }
                Err(e) => {
                    if !dir.exists() {
                        return Err(interrupted("segment removed"));
                    }
                    unavailable.push(json!({"segment": manifest.journal_label, "error": e}));
                }
            }
        }
        items.sort_unstable_by_key(|(min, _, _)| *min);
        Ok(items)
    }

    /// Load the segments and journal groups for `[oldest, newest]`.
    fn sources(&self, oldest: Option<u64>, newest: u64) -> io::Result<Sources> {
        let segments_dir = segment::segments_dir(&self.state_dir)?;
        let listed = segment::list(&self.state_dir)?;
        let mut covered = Vec::new();
        let mut segments = Vec::new();
        for (label, manifest) in listed {
            if manifest.first_group > newest {
                continue;
            }
            covered.push((manifest.first_group, manifest.last_group));
            segments.push((segments_dir.join(segment::segment_name(label)), manifest));
        }
        let journal_dir = self.state_dir.join("journal");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&journal_dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("sealed-") && n.ends_with(".faj"))
            })
            .collect();
        files.sort();
        files.push(journal_dir.join(fabric_frame::frame::ACTIVE));
        let mut journal = Vec::new();
        for path in files {
            let file = match File::open(&path) {
                Ok(file) => file,
                // Segmented and reclaimed since the listing: its groups are
                // in a segment the listing may not include, so retry.
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "journal moved"));
                }
                Err(e) => return Err(e),
            };
            let len = file.metadata()?.len();
            let mut at = 0;
            while at < len {
                let Some((payload, next)) = read_frame(&file, at, len, MAX_GROUP_PAYLOAD)? else {
                    break; // an append in progress; its group is beyond the snapshot
                };
                let group = Group::decode(payload.as_slice())
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
                let seq = group.group_sequence;
                if seq <= newest && !covered.iter().any(|(a, b)| (*a..=*b).contains(&seq)) {
                    journal.push(group);
                }
                at = next;
            }
        }
        let oldest_group = segments
            .iter()
            .map(|(_, m)| m.first_group)
            .chain(journal.iter().map(|g| g.group_sequence))
            .min()
            .unwrap_or(newest + 1);
        let floor = oldest.unwrap_or(oldest_group);
        segments.retain(|(_, m)| m.last_group >= floor);
        journal.retain(|g| g.group_sequence >= floor);
        Ok(Sources {
            segments,
            journal,
            oldest_group,
            tail: Vec::new(),
            tail_paths: HashMap::new(),
            tail_evidence: None,
            bounds: Vec::new(),
            filters: Vec::new(),
        })
    }

    pub fn run(&self, query: &Query, committed_group: u64) -> Result<Value, QueryError> {
        let mut last = None;
        for _ in 0..3 {
            match self.run_once(query, committed_group) {
                Err(QueryError::Io(e)) if e.kind() == io::ErrorKind::Interrupted => last = Some(e),
                other => return other,
            }
        }
        Err(QueryError::Io(last.unwrap()))
    }

    fn run_once(&self, query: &Query, committed_group: u64) -> Result<Value, QueryError> {
        let (node, from, to, limit, page) = match query {
            Query::Logs {
                node,
                from_ns,
                to_ns,
                limit,
                page,
                ..
            }
            | Query::Metrics {
                node,
                from_ns,
                to_ns,
                limit,
                page,
                ..
            } => (node, *from_ns, *to_ns, Some(*limit), page.as_ref()),
            Query::Rate {
                node,
                from_ns,
                to_ns,
                ..
            } => (node, *from_ns, *to_ns, None, None),
        };
        let rejected = |r: QueryRejection| {
            QueryError::Invalid(match r {
                QueryRejection::EmptyWindow => "from_ns must be below to_ns".into(),
                QueryRejection::LimitOutOfRange => "limit must be 1-10000".into(),
            })
        };
        let window = Window::new(from, to).map_err(rejected)?;
        if let Some(limit) = limit {
            kernel::check_limit(limit).map_err(rejected)?;
        }
        let fingerprint = {
            let mut q = serde_json::to_value(QueryShape(query)).unwrap();
            q.as_object_mut().unwrap().remove("page");
            q
        };
        let (floor, newest, after) = match page {
            None => (None, committed_group, None),
            Some(token) => {
                let t =
                    decode_token(token).ok_or(QueryError::Invalid("invalid page token".into()))?;
                if t["query"] != fingerprint {
                    return Err(QueryError::Invalid(
                        "page token belongs to another query".into(),
                    ));
                }
                let key = t["after"].as_array().and_then(|a| {
                    Some((
                        a.first()?.as_u64()?,
                        unhex16(a.get(1)?.as_str()?)?,
                        a.get(2)?.as_u64()?,
                        a.get(3)?.as_u64()? as u32,
                    ))
                });
                match (t["oldest"].as_u64(), t["newest"].as_u64(), key) {
                    (Some(o), Some(n), Some(k)) => (Some(o), n, Some(k)),
                    _ => return Err(QueryError::Invalid("invalid page token".into())),
                }
            }
        };
        let walking = self.plan == Plan::Walk;
        let sources = if walking {
            self.sources_walk(floor, newest, query, &window, node)?
        } else {
            self.sources(floor, newest)?
        };
        let oldest = floor.unwrap_or(sources.oldest_group);
        if floor.is_some_and(|f| !kernel::page_snapshot_retained(f, sources.oldest_group)) {
            return Err(QueryError::Gone);
        }
        let snapshot = Snapshot {
            oldest_group: oldest,
            newest_group: newest,
        };
        let node_ok = |n: &str| node.as_deref().is_none_or(|want| want == n);

        let mut unavailable = Vec::new();
        let mut freshness: BTreeMap<String, u64> = BTreeMap::new();
        let mut received = (u64::MAX, 0_u64);
        let mut gaps: Vec<GapRow> = Vec::new();
        let mut journal_rows = Rows::default();
        for group in &sources.journal {
            for entry in &group.entries {
                received = (
                    received.0.min(entry.received_unix_nano),
                    received.1.max(entry.received_unix_nano),
                );
                extract(group.group_sequence, entry, &mut journal_rows)?;
            }
        }
        for r in &journal_rows.logs {
            let f = freshness.entry(r.node.clone()).or_default();
            *f = (*f).max(r.observed_ns);
        }
        for r in &journal_rows.metrics {
            let f = freshness.entry(r.node.clone()).or_default();
            *f = (*f).max(r.time_ns);
        }
        if let Some((r, f)) = &sources.tail_evidence {
            received = (received.0.min(r.0), received.1.max(r.1));
            for (n, t) in f {
                let e = freshness.entry(n.clone()).or_default();
                *e = (*e).max(*t);
            }
        }
        gaps.extend(journal_rows.gaps.iter().cloned());
        for (dir, manifest) in &sources.segments {
            received = (
                received.0.min(manifest.received_min_ns),
                received.1.max(manifest.received_max_ns),
            );
            for (n, t) in &manifest.freshness {
                let f = freshness.entry(n.clone()).or_default();
                *f = (*f).max(*t);
            }
            if let Err(e) = segment::scan_gaps(dir, manifest, |g| gaps.push(g)) {
                if !dir.exists() {
                    return Err(
                        io::Error::new(io::ErrorKind::Interrupted, "segment removed").into(),
                    );
                }
                unavailable
                    .push(json!({"segment": manifest.journal_label, "error": e.to_string()}));
            }
        }
        gaps.retain(|g| {
            node_ok(&g.node) && window.contains(g.received_ns) && snapshot.contains(g.group)
        });
        gaps.sort_by_key(|g| (g.received_ns, g.node_id, g.sequence));

        let mut next_page = Value::Null;
        let rows_json = match query {
            Query::Logs {
                contains, limit, ..
            } => {
                let mut best = Smallest::new(*limit as usize + 1);
                let keep = |r: &LogRow| {
                    node_ok(&r.node)
                        && window.contains(r.observed_ns)
                        && snapshot.contains(r.group)
                        && contains.as_deref().is_none_or(|c| r.body.contains(c))
                };
                let key = |r: &LogRow| (r.observed_ns, r.node_id, r.sequence, r.index);
                for r in journal_rows.logs.iter().filter(|r| keep(r)) {
                    if kernel::after_page(&key(r), after.as_ref()) {
                        best.offer(key(r), r.clone());
                    }
                }
                if walking {
                    let items = Self::walk_order(
                        &sources,
                        &window,
                        |e| e.logs,
                        contains.as_deref(),
                        &mut unavailable,
                    )?;
                    let mut reader = TailReader::new(&sources.tail_paths);
                    for (min, max, source) in items {
                        if after.as_ref().is_some_and(|a| max < a.0) {
                            continue;
                        }
                        if best.threshold().is_some_and(|t| min > t.0) {
                            break;
                        }
                        match source {
                            Source::Tail(i) => {
                                for r in reader.rows(&sources.tail[i])?.logs {
                                    if keep(&r) && kernel::after_page(&key(&r), after.as_ref()) {
                                        best.offer(key(&r), r);
                                    }
                                }
                            }
                            Source::Group(si, rg) => {
                                let (dir, manifest) = &sources.segments[si];
                                let scanned = segment::scan_logs_groups(
                                    dir,
                                    manifest,
                                    vec![rg],
                                    from,
                                    to,
                                    |r| {
                                        if keep(&r) && kernel::after_page(&key(&r), after.as_ref())
                                        {
                                            best.offer(key(&r), r);
                                        }
                                    },
                                );
                                if let Err(e) = scanned {
                                    if !dir.exists() {
                                        return Err(io::Error::new(
                                            io::ErrorKind::Interrupted,
                                            "segment removed",
                                        )
                                        .into());
                                    }
                                    unavailable.push(
                            json!({"segment": manifest.journal_label, "error": e.to_string()}),
                        );
                                }
                            }
                        }
                    }
                } else {
                    for (dir, manifest) in &sources.segments {
                        let scanned = segment::scan_logs(dir, manifest, from, to, |r| {
                            if keep(&r) && kernel::after_page(&key(&r), after.as_ref()) {
                                best.offer(key(&r), r);
                            }
                        });
                        if let Err(e) = scanned {
                            if !dir.exists() {
                                return Err(io::Error::new(
                                    io::ErrorKind::Interrupted,
                                    "segment removed",
                                )
                                .into());
                            }
                            unavailable.push(
                                json!({"segment": manifest.journal_label, "error": e.to_string()}),
                            );
                        }
                    }
                }
                let mut sorted = best.sorted();
                if sorted.len() > *limit as usize {
                    sorted.truncate(*limit as usize);
                    let (k, _) = sorted.last().unwrap();
                    next_page = json!(encode_token(&json!({
                        "oldest": oldest, "newest": newest, "query": fingerprint,
                        "after": [k.0, hex(&k.1), k.2, k.3],
                    })));
                }
                sorted.iter().map(|(_, r)| log_json(r)).collect()
            }
            Query::Metrics { name, limit, .. } => {
                let mut best = Smallest::new(*limit as usize + 1);
                let keep = |r: &MetricRow| {
                    node_ok(&r.node)
                        && &r.name == name
                        && window.contains(r.time_ns)
                        && snapshot.contains(r.group)
                };
                let key = |r: &MetricRow| (r.time_ns, r.node_id, r.sequence, r.index);
                for r in journal_rows.metrics.iter().filter(|r| keep(r)) {
                    if kernel::after_page(&key(r), after.as_ref()) {
                        best.offer(key(r), r.clone());
                    }
                }
                if walking {
                    let items =
                        Self::walk_order(&sources, &window, |e| e.metrics, None, &mut unavailable)?;
                    let mut reader = TailReader::new(&sources.tail_paths);
                    for (min, max, source) in items {
                        if after.as_ref().is_some_and(|a| max < a.0) {
                            continue;
                        }
                        if best.threshold().is_some_and(|t| min > t.0) {
                            break;
                        }
                        match source {
                            Source::Tail(i) => {
                                for r in reader.rows(&sources.tail[i])?.metrics {
                                    if keep(&r) && kernel::after_page(&key(&r), after.as_ref()) {
                                        best.offer(key(&r), r);
                                    }
                                }
                            }
                            Source::Group(si, rg) => {
                                let (dir, manifest) = &sources.segments[si];
                                let scanned = segment::scan_metrics_groups(
                                    dir,
                                    manifest,
                                    vec![rg],
                                    from,
                                    to,
                                    |r| {
                                        if keep(&r) && kernel::after_page(&key(&r), after.as_ref())
                                        {
                                            best.offer(key(&r), r);
                                        }
                                    },
                                );
                                if let Err(e) = scanned {
                                    if !dir.exists() {
                                        return Err(io::Error::new(
                                            io::ErrorKind::Interrupted,
                                            "segment removed",
                                        )
                                        .into());
                                    }
                                    unavailable.push(
                            json!({"segment": manifest.journal_label, "error": e.to_string()}),
                        );
                                }
                            }
                        }
                    }
                } else {
                    for (dir, manifest) in &sources.segments {
                        let scanned = segment::scan_metrics(dir, manifest, from, to, |r| {
                            if keep(&r) && kernel::after_page(&key(&r), after.as_ref()) {
                                best.offer(key(&r), r);
                            }
                        });
                        if let Err(e) = scanned {
                            if !dir.exists() {
                                return Err(io::Error::new(
                                    io::ErrorKind::Interrupted,
                                    "segment removed",
                                )
                                .into());
                            }
                            unavailable.push(
                                json!({"segment": manifest.journal_label, "error": e.to_string()}),
                            );
                        }
                    }
                }
                let mut sorted = best.sorted();
                if sorted.len() > *limit as usize {
                    sorted.truncate(*limit as usize);
                    let (k, _) = sorted.last().unwrap();
                    next_page = json!(encode_token(&json!({
                        "oldest": oldest, "newest": newest, "query": fingerprint,
                        "after": [k.0, hex(&k.1), k.2, k.3],
                    })));
                }
                sorted.iter().map(|(_, r)| metric_json(r)).collect()
            }
            Query::Rate { name, .. } => {
                let mut points: Vec<MetricRow> = Vec::new();
                let keep = |r: &MetricRow| {
                    node_ok(&r.node)
                        && &r.name == name
                        && r.sum
                        && r.monotonic
                        && window.contains(r.time_ns)
                        && snapshot.contains(r.group)
                };
                points.extend(journal_rows.metrics.iter().filter(|r| keep(r)).cloned());
                if walking {
                    // A rate has no limit, so nothing stops it; the walk reads only the
                    // tail entries and row groups whose bounds meet the window.
                    let items =
                        Self::walk_order(&sources, &window, |e| e.metrics, None, &mut unavailable)?;
                    let mut reader = TailReader::new(&sources.tail_paths);
                    for (_, _, source) in items {
                        match source {
                            Source::Tail(i) => {
                                points.extend(
                                    reader
                                        .rows(&sources.tail[i])?
                                        .metrics
                                        .into_iter()
                                        .filter(|r| keep(r)),
                                );
                            }
                            Source::Group(si, rg) => {
                                let (dir, manifest) = &sources.segments[si];
                                let scanned = segment::scan_metrics_groups(
                                    dir,
                                    manifest,
                                    vec![rg],
                                    from,
                                    to,
                                    |r| {
                                        if keep(&r) {
                                            points.push(r);
                                        }
                                    },
                                );
                                if let Err(e) = scanned {
                                    if !dir.exists() {
                                        return Err(io::Error::new(
                                            io::ErrorKind::Interrupted,
                                            "segment removed",
                                        )
                                        .into());
                                    }
                                    unavailable.push(
                            json!({"segment": manifest.journal_label, "error": e.to_string()}),
                        );
                                }
                            }
                        }
                    }
                } else {
                    for (dir, manifest) in &sources.segments {
                        let scanned = segment::scan_metrics(dir, manifest, from, to, |r| {
                            if keep(&r) {
                                points.push(r);
                            }
                        });
                        if let Err(e) = scanned {
                            if !dir.exists() {
                                return Err(io::Error::new(
                                    io::ErrorKind::Interrupted,
                                    "segment removed",
                                )
                                .into());
                            }
                            unavailable.push(
                                json!({"segment": manifest.journal_label, "error": e.to_string()}),
                            );
                        }
                    }
                }
                rates(points)
            }
        };
        let retained = if received.0 == u64::MAX {
            (json!(0), json!(0))
        } else {
            (json!(received.0), json!(received.1))
        };
        Ok(json!({
            "complete": kernel::complete(unavailable.len()),
            "unavailable": unavailable,
            "retained_from_ns": retained.0,
            "retained_to_ns": retained.1,
            "freshness": freshness,
            "gaps": gaps.iter().map(|g| json!({
                "node": g.node, "sequence": g.sequence, "receive_ns": g.received_ns, "gap": g.text,
            })).collect::<Vec<_>>(),
            "snapshot": format!("g{oldest}-{newest}"),
            "next_page": next_page,
            "rows": rows_json,
        }))
    }
}

/// Serialize a query for the page-token fingerprint.
struct QueryShape<'a>(&'a Query);

impl serde::Serialize for QueryShape<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let v = match self.0 {
            Query::Logs {
                node,
                from_ns,
                to_ns,
                contains,
                limit,
                ..
            } => json!({
                "kind": "logs", "node": node, "from_ns": from_ns, "to_ns": to_ns,
                "contains": contains, "limit": limit,
            }),
            Query::Metrics {
                node,
                name,
                from_ns,
                to_ns,
                limit,
                ..
            } => json!({
                "kind": "metrics", "node": node, "name": name, "from_ns": from_ns,
                "to_ns": to_ns, "limit": limit,
            }),
            Query::Rate {
                node,
                name,
                from_ns,
                to_ns,
            } => json!({
                "kind": "rate", "node": node, "name": name, "from_ns": from_ns, "to_ns": to_ns,
            }),
        };
        v.serialize(s)
    }
}

/// Per series of a monotonic cumulative sum: a rate per consecutive pair
/// sharing `start_ns` with no decrease, otherwise a reset marker.
fn rates(mut points: Vec<MetricRow>) -> Vec<Value> {
    let series = |r: &MetricRow| {
        (
            r.node.clone(),
            r.attributes
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>(),
        )
    };
    points.sort_by(|a, b| {
        (series(a), a.time_ns, a.node_id, a.sequence, a.index).cmp(&(
            series(b),
            b.time_ns,
            b.node_id,
            b.sequence,
            b.index,
        ))
    });
    let as_f64 = |v: Number| match v {
        Number::Int(i) => i as f64,
        Number::Double(d) => d,
    };
    let mut out = Vec::new();
    for pair in points.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if series(a) != series(b) {
            continue;
        }
        let base = json!({"node": b.node, "name": b.name, "attributes": b.attributes, "time_ns": b.time_ns});
        let mut row = base.as_object().unwrap().clone();
        let point = |r: &MetricRow| CounterPoint {
            start_ns: r.start_ns,
            time_ns: r.time_ns,
            value: as_f64(r.value),
        };
        match kernel::counter_step(point(a), point(b)) {
            CounterStep::Rate(rate) => {
                row.insert("reset".into(), json!(false));
                row.insert("rate".into(), json!(rate));
            }
            CounterStep::Reset => {
                row.insert("reset".into(), json!(true));
                row.insert("rate".into(), Value::Null);
            }
        }
        out.push(Value::Object(row));
    }
    out
}
