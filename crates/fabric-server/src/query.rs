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

use crate::read_catalog::{CatalogRequest, FilterKind, ReadCatalog, Sources};
use crate::rows::{
    GapRow, LogRow, MetricRow, Number, Rows, SpanRow, extract, latest_observation_bytes,
};
use crate::segment::{self, MAX_GROUP_PAYLOAD, Table};
use crate::store::Group;
use crate::tail::{ManifestHandle, TailEntry, TailReader, interrupted};
use fabric_core::query::{
    self as kernel, CounterSample, CounterStep, QueryRejection, Snapshot, Window,
};
use fabric_frame::frame::read_frame;
use prost::Message;
use serde::Deserialize;
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

mod selection;
use selection::Smallest;

#[cfg(test)]
#[path = "query_discovery_race_test.rs"]
mod discovery_race_tests;

#[cfg(test)]
#[path = "catalog_lifetime_tests.rs"]
mod catalog_lifetime_tests;

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
    /// Spans by start time, optionally one trace or one name (ADR-0025).
    Spans {
        #[serde(default)]
        node: Option<String>,
        from_ns: u64,
        to_ns: u64,
        #[serde(default)]
        trace_id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        limit: u32,
        #[serde(default)]
        page: Option<String>,
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

fn key_can_enter_page(key: &Key, after: Option<&Key>, threshold: Option<Key>) -> bool {
    kernel::after_page(key, after) && threshold.is_none_or(|top| *key < top)
}

#[cfg(test)]
mod key_first_tests {
    use super::*;

    #[test]
    fn full_key_page_and_threshold_boundaries_preserve_stable_ties() {
        let first = (7, [0; 16], 1, 0);
        let later_index = (7, [0; 16], 1, 1);
        let later_identity = (7, [1; 16], 0, 0);
        assert!(key_can_enter_page(&first, None, None));
        assert!(!key_can_enter_page(&first, Some(&first), None));
        assert!(key_can_enter_page(&later_index, Some(&first), None));
        assert!(!key_can_enter_page(&first, None, Some(first)));
        assert!(key_can_enter_page(&first, None, Some(later_index)));
        assert!(key_can_enter_page(
            &later_index,
            Some(&first),
            Some(later_identity)
        ));
        let mut best = Smallest::new(1);
        best.offer(first, "first equal-key payload");
        if key_can_enter_page(&first, None, best.threshold()) {
            best.offer(first, "later equal-key payload");
        }
        assert_eq!(best.sorted(), vec![(first, "first equal-key payload")]);
    }
}

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

/// One source of the walk: a tail entry, or a row group of a Segment.
#[derive(Clone, Copy)]
enum Source {
    Tail(usize),
    Block(usize),
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

/// Boundary Segments can contain groups committed after a page's snapshot.
/// Derive their envelope only from included records; stage it until every raw
/// record has been verified so a failed scan cannot leak partial metadata.
type SegmentEvidence<'a> = ((u64, u64), Cow<'a, BTreeMap<String, u64>>);

fn boundary_evidence(
    dir: &Path,
    manifest: &segment::Manifest,
    snapshot: Snapshot,
) -> io::Result<SegmentEvidence<'static>> {
    let mut received = (u64::MAX, 0_u64);
    let mut freshness: BTreeMap<String, u64> = BTreeMap::new();
    let mut failure = None;
    segment::scan_batches_borrowed_one_at_a_time(dir, manifest, |group, entry| {
        if failure.is_some() || !snapshot.contains(group) {
            return;
        }
        // Full typed validation, one Entry at a time, without projecting Rows.
        let newest = match latest_observation_bytes(entry.batch) {
            Ok(newest) => newest,
            Err(error) => {
                failure = Some(error);
                return;
            }
        };
        received.0 = received.0.min(entry.received_unix_nano);
        received.1 = received.1.max(entry.received_unix_nano);
        if let Some(newest) = newest {
            if let Some(current) = freshness.get_mut(entry.label) {
                *current = (*current).max(newest);
            } else {
                freshness.insert(entry.label.to_owned(), newest);
            }
        }
    })?;
    if let Some(error) = failure {
        return Err(error);
    }
    Ok((received, Cow::Owned(freshness)))
}

fn segment_evidence<'a>(
    dir: &Path,
    manifest: &'a segment::Manifest,
    snapshot: Snapshot,
) -> io::Result<SegmentEvidence<'a>> {
    if snapshot.contains(manifest.first_group) && snapshot.contains(manifest.last_group) {
        Ok((
            (manifest.received_min_ns, manifest.received_max_ns),
            Cow::Borrowed(&manifest.freshness),
        ))
    } else {
        boundary_evidence(dir, manifest, snapshot)
    }
}

#[cfg(test)]
#[path = "query/snapshot_evidence_tests.rs"]
mod snapshot_evidence_tests;

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

fn span_json(r: &SpanRow) -> Value {
    json!({
        "node": r.node, "node_id": hex(&r.node_id), "sequence": r.sequence, "index": r.index,
        "trace_id": r.trace_id, "span_id": r.span_id, "parent_span_id": r.parent_span_id,
        "name": r.name, "kind": r.kind, "status": r.status,
        "start_ns": r.start_ns, "end_ns": r.end_ns, "attributes": r.attributes,
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
    catalog: ReadCatalog,
}

impl History {
    /// Compile-time experimental scanner selector, acknowledged by native probes.
    #[doc(hidden)]
    pub fn borrowed_logs_enabled() -> bool {
        option_env!("FABRIC_BORROWED_LOG_EXPERIMENT").is_some_and(|v| v == "1")
    }

    /// Opt-in query admission ordering; readers and validation stay unchanged.
    #[doc(hidden)]
    pub fn key_first_enabled() -> bool {
        option_env!("FABRIC_KEY_FIRST_EXPERIMENT").is_some_and(|v| v == "1")
    }

    pub fn new(state_dir: &Path) -> Self {
        Self::with_plan(state_dir, Plan::Scan)
    }

    pub fn with_plan(state_dir: &Path, plan: Plan) -> Self {
        Self {
            state_dir: state_dir.to_path_buf(),
            plan,
            catalog: ReadCatalog::new(state_dir),
        }
    }

    /// Experimental immutable metadata views; production constructors keep cloning.
    #[doc(hidden)]
    pub fn with_shared_catalog(mut self) -> Self {
        self.catalog.use_shared_metadata();
        self
    }

    /// Experimental bounded physical descriptor reuse; no retained-file lease.
    #[doc(hidden)]
    pub fn with_descriptor_reuse(mut self) -> Self {
        self.catalog.use_descriptor_reuse();
        self
    }

    /// Read after timing; the byte charge is an estimate, not allocator live bytes.
    #[doc(hidden)]
    pub fn descriptor_reuse_stats(&self) -> Option<Value> {
        self.catalog
            .reuse_stats()
            .map(|(hits, builds, fallbacks, holders, bytes)| {
                json!({"hits":hits,"builds":builds,"fallbacks":fallbacks,
                "held_snapshot_readers":holders,"estimated_charged_bytes":bytes,
                "charge_cap_bytes":8*1024*1024,"holder_cap":4,"descriptor_cap":256,
                "selected_indices_max_bytes_per_holder":256*std::mem::size_of::<usize>()})
            })
    }

    /// Explicit derived-state maintenance for bounded experimental schedules.
    /// Does not alter durable commit, publication, ACK or retention ordering.
    #[doc(hidden)]
    pub fn refresh_catalog(&self, committed_group: u64) -> Result<(), QueryError> {
        self.catalog
            .refresh(committed_group)
            .map_err(QueryError::Io)
    }

    /// Close tail blocks at `records` records instead of the default (tests use small
    /// blocks so that a small history has several).
    #[doc(hidden)]
    pub fn with_tail_block_records(self, records: usize) -> Self {
        self.catalog.set_block_records(records);
        self
    }

    /// How many complete tail blocks the walk plan holds (for tests and diagnostics).
    #[doc(hidden)]
    pub fn tail_blocks(&self) -> usize {
        self.catalog.blocks()
    }

    /// Physical source acquisition belongs to storage; semantic filtering remains here.
    fn sources_walk(
        &self,
        oldest: Option<u64>,
        newest: u64,
        query: &Query,
        window: &Window,
        node: &Option<String>,
        authorized_nodes: Option<&HashSet<String>>,
    ) -> io::Result<Sources> {
        #[cfg(feature = "phase-probe")]
        let _phase = fabric_frame::probe::span("query_walk_source_loading");
        let table = match query {
            Query::Logs { .. } => Table::Logs,
            Query::Metrics { .. } | Query::Rate { .. } => Table::Metrics,
            Query::Spans { .. } => Table::Spans,
        };
        let filter = match query {
            Query::Logs {
                contains: Some(c), ..
            } if c.len() >= 3 => FilterKind::Logs,
            Query::Spans {
                trace_id: Some(t), ..
            } if t.len() >= 3 => FilterKind::Spans,
            _ => FilterKind::None,
        };
        self.catalog.view(CatalogRequest {
            oldest,
            newest,
            table,
            window,
            node,
            authorized_nodes,
            filter,
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
        traces: bool,
        unavailable: &mut Vec<Value>,
    ) -> io::Result<Vec<(u64, u64, Source)>> {
        let mut items: Vec<(u64, u64, Source)> = sources
            .tail
            .iter()
            .enumerate()
            .map(|(i, e)| (kind(e).0, kind(e).1, Source::Tail(i)))
            .collect();
        for (j, b) in sources.blocks.iter().enumerate() {
            // A block whose filter lacks one of the needle's trigrams holds no match.
            let filter = if traces {
                &b.block.trace_filter
            } else {
                &b.block.filter
            };
            if needle.is_some_and(|n| n.len() >= 3 && !filter.may_contain(n.as_bytes())) {
                continue;
            }
            items.push((b.bounds.0, b.bounds.1, Source::Block(j)));
        }
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
        self.sources_inner(oldest, newest, || {})
    }

    #[cfg(test)]
    pub(crate) fn sources_at_discovery_cut(
        &self,
        oldest: Option<u64>,
        newest: u64,
        after_segments: impl FnOnce(),
    ) -> io::Result<Sources> {
        self.sources_inner(oldest, newest, after_segments)
    }

    fn sources_inner(
        &self,
        oldest: Option<u64>,
        newest: u64,
        after_segments: impl FnOnce(),
    ) -> io::Result<Sources> {
        #[cfg(feature = "phase-probe")]
        let _phase = fabric_frame::probe::span("query_scan_source_loading");

        let segments_dir = segment::segments_dir(&self.state_dir)?;
        let listed = segment::list(&self.state_dir)?;
        let before: HashSet<_> = listed.iter().map(|(label, _)| *label).collect();
        let mut covered = Vec::new();
        let mut segments = Vec::new();
        for (label, manifest) in listed {
            if manifest.first_group > newest {
                continue;
            }
            covered.push((manifest.first_group, manifest.last_group));
            segments.push((
                segments_dir.join(segment::segment_name(label)),
                ManifestHandle::Owned(manifest),
            ));
        }
        after_segments();
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
        let after_labels: HashSet<_> = segment::labels(&self.state_dir)?.into_iter().collect();
        if before != after_labels {
            return Err(interrupted("segment coverage changed during discovery"));
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
            segments: crate::coupled_catalog::SegmentSources::Owned(segments),
            journal,
            oldest_group,
            tail: Vec::new(),
            tail_paths: HashMap::new(),
            tail_evidence: None,
            bounds: Vec::new(),
            filters: Vec::new(),
            blocks: Vec::new(),
        })
    }

    pub fn run(&self, query: &Query, committed_group: u64) -> Result<Value, QueryError> {
        self.run_authorized(query, committed_group, None)
    }

    /// The transport resolves immutable authorized enrollments to their durable,
    /// non-reusable labels. Enforce that set before row selection and metadata
    /// projection, including all pages. Never filter only the completed answer.
    pub fn run_scoped(
        &self,
        query: &Query,
        committed_group: u64,
        nodes: &HashSet<String>,
    ) -> Result<Value, QueryError> {
        self.run_authorized(query, committed_group, Some(nodes))
    }

    fn run_authorized(
        &self,
        query: &Query,
        committed_group: u64,
        nodes: Option<&HashSet<String>>,
    ) -> Result<Value, QueryError> {
        let mut last = None;
        for _ in 0..3 {
            match self.run_once(query, committed_group, nodes) {
                Err(QueryError::Io(e)) if e.kind() == io::ErrorKind::Interrupted => last = Some(e),
                other => return other,
            }
        }
        Err(QueryError::Io(last.unwrap()))
    }

    fn run_once(
        &self,
        query: &Query,
        committed_group: u64,
        authorized_nodes: Option<&HashSet<String>>,
    ) -> Result<Value, QueryError> {
        #[cfg(feature = "phase-probe")]
        let _phase = fabric_frame::probe::span("query_run_inclusive");

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
            }
            | Query::Spans {
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
        // Scoped source maps apply the authorization set before lazy payload
        // selection and compute tail evidence for the requested signal only.
        let walking = self.plan == Plan::Walk || authorized_nodes.is_some();
        let sources = if walking {
            self.sources_walk(floor, newest, query, &window, node, authorized_nodes)?
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
        let node_ok = |n: &str| {
            node.as_deref().is_none_or(|want| want == n)
                && authorized_nodes.is_none_or(|allowed| allowed.contains(n))
        };
        let scoped_latest = |bytes: &[u8]| {
            crate::rows::latest_selected_observation_bytes(
                bytes,
                matches!(query, Query::Logs { .. }),
                matches!(query, Query::Metrics { .. } | Query::Rate { .. }),
                matches!(query, Query::Spans { .. }),
            )
        };

        let mut unavailable = Vec::new();
        let mut freshness: BTreeMap<String, u64> = BTreeMap::new();
        let mut received = (u64::MAX, 0_u64);
        let mut gaps: Vec<GapRow> = Vec::new();
        let mut journal_rows = Rows::default();
        for group in &sources.journal {
            for entry in &group.entries {
                if authorized_nodes.is_some() {
                    if !node_ok(&entry.label) {
                        continue;
                    }
                    if let Some(newest) = scoped_latest(&entry.batch)? {
                        freshness
                            .entry(entry.label.clone())
                            .and_modify(|v| *v = (*v).max(newest))
                            .or_insert(newest);
                        received = (
                            received.0.min(entry.received_unix_nano),
                            received.1.max(entry.received_unix_nano),
                        );
                    }
                    extract(group.group_sequence, entry, &mut journal_rows)?;
                    continue;
                }
                received = (
                    received.0.min(entry.received_unix_nano),
                    received.1.max(entry.received_unix_nano),
                );
                extract(group.group_sequence, entry, &mut journal_rows)?;
            }
        }
        for r in journal_rows
            .logs
            .iter()
            .filter(|_| authorized_nodes.is_none())
        {
            let f = freshness.entry(r.node.clone()).or_default();
            *f = (*f).max(r.observed_ns);
        }
        for r in journal_rows
            .metrics
            .iter()
            .filter(|_| authorized_nodes.is_none())
        {
            let f = freshness.entry(r.node.clone()).or_default();
            *f = (*f).max(r.time_ns);
        }
        for r in journal_rows
            .spans
            .iter()
            .filter(|_| authorized_nodes.is_none())
        {
            let f = freshness.entry(r.node.clone()).or_default();
            *f = (*f).max(r.start_ns);
        }
        if let Some((r, f)) = &sources.tail_evidence {
            received = (received.0.min(r.0), received.1.max(r.1));
            for (n, t) in f {
                let e = freshness.entry(n.clone()).or_default();
                *e = (*e).max(*t);
            }
        }
        gaps.extend(
            journal_rows
                .gaps
                .iter()
                .filter(|g| {
                    authorized_nodes.is_none()
                        || (node_ok(&g.node)
                            && window.contains(g.received_ns)
                            && snapshot.contains(g.group))
                })
                .cloned(),
        );
        let mut scoped_gap_bytes = gaps.iter().fold(0usize, |n, g| {
            n.saturating_add(g.text.len())
                .saturating_add(g.node.len())
                .saturating_add(128)
        });
        let mut scoped_gap_overflow =
            authorized_nodes.is_some() && (gaps.len() > 4096 || scoped_gap_bytes > 1024 * 1024);
        for (dir, manifest) in &sources.segments {
            if let Err(e) = segment::check_raw_available(dir, manifest) {
                if !dir.exists() {
                    return Err(
                        io::Error::new(io::ErrorKind::Interrupted, "segment removed").into(),
                    );
                }
                unavailable
                    .push(json!({"segment": manifest.journal_label, "error": e.to_string()}));
            }
            let evidence = if let Some(allowed) = authorized_nodes {
                let table = match query {
                    Query::Logs { .. } => Table::Logs,
                    Query::Metrics { .. } | Query::Rate { .. } => Table::Metrics,
                    Query::Spans { .. } => Table::Spans,
                };
                self.catalog
                    .scoped_segment_evidence(dir, manifest, snapshot, table, allowed, node)
                    .map(|(bounds, by_node)| (bounds, Cow::Owned(by_node)))
            } else {
                segment_evidence(dir, manifest, snapshot)
            };
            match evidence {
                Ok((bounds, evidence)) => {
                    received = (received.0.min(bounds.0), received.1.max(bounds.1));
                    for (node, newest) in evidence.iter() {
                        let current = freshness.entry(node.clone()).or_default();
                        *current = (*current).max(*newest);
                    }
                }
                Err(error) => {
                    if !dir.exists() {
                        return Err(interrupted("segment removed").into());
                    }
                    unavailable.push(
                        json!({"segment": manifest.journal_label, "error": error.to_string()}),
                    );
                }
            }
            if let Err(e) = segment::scan_gaps(dir, manifest, |g| {
                if authorized_nodes.is_some() {
                    if scoped_gap_overflow
                        || !node_ok(&g.node)
                        || !window.contains(g.received_ns)
                        || !snapshot.contains(g.group)
                    {
                        return;
                    }
                    scoped_gap_bytes = scoped_gap_bytes
                        .saturating_add(g.text.len())
                        .saturating_add(g.node.len())
                        .saturating_add(128);
                    if gaps.len() >= 4096 || scoped_gap_bytes > 1024 * 1024 {
                        scoped_gap_overflow = true;
                        return;
                    }
                }
                gaps.push(g);
            }) {
                if !dir.exists() {
                    return Err(
                        io::Error::new(io::ErrorKind::Interrupted, "segment removed").into(),
                    );
                }
                unavailable
                    .push(json!({"segment": manifest.journal_label, "error": e.to_string()}));
            }
        }
        if scoped_gap_overflow {
            return Err(QueryError::Invalid(
                "query gap evidence budget exceeded; narrow the window".into(),
            ));
        }
        gaps.retain(|g| {
            node_ok(&g.node) && window.contains(g.received_ns) && snapshot.contains(g.group)
        });
        gaps.sort_by_key(|g| (g.received_ns, g.node_id, g.sequence));

        #[cfg(feature = "phase-probe")]
        let _execution = fabric_frame::probe::span("query_execute_inclusive_loading");
        let mut next_page = Value::Null;
        let rows_json = match query {
            Query::Logs {
                contains, limit, ..
            } => {
                let mut best = Smallest::new(*limit as usize + 1);
                if authorized_nodes.is_some() {
                    best = best.with_byte_budget(1024 * 1024, LogRow::retained_bytes);
                }
                let key_first = Self::key_first_enabled();
                let can_enter = |k: &Key, threshold: Option<Key>| {
                    key_can_enter_page(k, after.as_ref(), threshold)
                };
                let keep = |r: &LogRow| {
                    node_ok(&r.node)
                        && window.contains(r.observed_ns)
                        && snapshot.contains(r.group)
                        && contains.as_deref().is_none_or(|c| r.body.contains(c))
                };
                let key = |r: &LogRow| (r.observed_ns, r.node_id, r.sequence, r.index);
                for r in &journal_rows.logs {
                    if (!key_first || can_enter(&key(r), best.threshold()))
                        && keep(r)
                        && (key_first || kernel::after_page(&key(r), after.as_ref()))
                    {
                        best.offer(key(r), r.clone());
                    }
                }
                if walking {
                    let items = Self::walk_order(
                        &sources,
                        &window,
                        |e| e.logs,
                        contains.as_deref(),
                        false,
                        &mut unavailable,
                    )?;
                    let mut reader = TailReader::new(&sources.tail_paths);
                    let mut dctx = zstd::bulk::Decompressor::new()?;
                    for (min, max, source) in items {
                        if after.as_ref().is_some_and(|a| max < a.0) {
                            continue;
                        }
                        if best.threshold().is_some_and(|t| min > t.0) {
                            break;
                        }
                        match source {
                            Source::Block(j) => {
                                let b = &sources.blocks[j];
                                crate::tail::visit_block(
                                    &b.block,
                                    &b.wanted,
                                    &mut dctx,
                                    |e, o| {
                                        let fabric_observation::SignalRef::Log { body, .. } =
                                            o.signal
                                        else {
                                            return;
                                        };
                                        if !window.contains(o.time_ns)
                                            || !node_ok(&e.node)
                                            || !snapshot.contains(e.group)
                                        {
                                            return;
                                        }
                                        let k = (o.time_ns, o.strand.node_id, o.sequence, o.index);
                                        if key_first && !can_enter(&k, best.threshold()) {
                                            return;
                                        }
                                        if contains.as_deref().is_some_and(|c| !body.contains(c)) {
                                            return;
                                        }
                                        if key_first || can_enter(&k, best.threshold()) {
                                            best.offer(k, crate::tail::log_row(e, o, body));
                                        }
                                    },
                                )?;
                            }
                            Source::Tail(i) => {
                                for r in reader.rows(&sources.tail[i])?.logs {
                                    if (!key_first || can_enter(&key(&r), best.threshold()))
                                        && keep(&r)
                                        && (key_first
                                            || kernel::after_page(&key(&r), after.as_ref()))
                                    {
                                        best.offer(key(&r), r);
                                    }
                                }
                            }
                            Source::Group(si, rg) => {
                                let (dir, manifest) = &sources.segments[si];
                                let scanned = if Self::borrowed_logs_enabled() {
                                    segment::scan_logs_groups_borrowed(
                                        dir,
                                        manifest,
                                        vec![rg],
                                        from,
                                        to,
                                        |r| {
                                            let k = (r.observed_ns, r.node_id, r.sequence, r.index);
                                            if (!key_first || can_enter(&k, best.threshold()))
                                                && node_ok(r.node)
                                                && window.contains(r.observed_ns)
                                                && snapshot.contains(r.group)
                                                && contains
                                                    .as_deref()
                                                    .is_none_or(|c| r.body.contains(c))
                                                && (key_first || can_enter(&k, best.threshold()))
                                            {
                                                best.offer(k, r.into_owned());
                                            }
                                        },
                                    )
                                } else {
                                    segment::scan_logs_groups(
                                        dir,
                                        manifest,
                                        vec![rg],
                                        from,
                                        to,
                                        |r| {
                                            if (!key_first || can_enter(&key(&r), best.threshold()))
                                                && keep(&r)
                                                && (key_first
                                                    || kernel::after_page(&key(&r), after.as_ref()))
                                            {
                                                best.offer(key(&r), r);
                                            }
                                        },
                                    )
                                };
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
                        let scanned = if Self::borrowed_logs_enabled() {
                            segment::scan_logs_borrowed(dir, manifest, from, to, |r| {
                                let k = (r.observed_ns, r.node_id, r.sequence, r.index);
                                if (!key_first || can_enter(&k, best.threshold()))
                                    && node_ok(r.node)
                                    && window.contains(r.observed_ns)
                                    && snapshot.contains(r.group)
                                    && contains.as_deref().is_none_or(|c| r.body.contains(c))
                                    && (key_first || can_enter(&k, best.threshold()))
                                {
                                    best.offer(k, r.into_owned());
                                }
                            })
                        } else {
                            segment::scan_logs(dir, manifest, from, to, |r| {
                                if (!key_first || can_enter(&key(&r), best.threshold()))
                                    && keep(&r)
                                    && (key_first || kernel::after_page(&key(&r), after.as_ref()))
                                {
                                    best.offer(key(&r), r);
                                }
                            })
                        };
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
                if best.budget_exceeded() {
                    return Err(QueryError::Invalid(
                        "query payload budget exceeded; reduce row limit".into(),
                    ));
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
                #[cfg(feature = "phase-probe")]
                let _construct = fabric_frame::probe::span("query_log_json_construct");
                sorted.iter().map(|(_, r)| log_json(r)).collect()
            }
            Query::Metrics { name, limit, .. } => {
                let mut best = Smallest::new(*limit as usize + 1);
                if authorized_nodes.is_some() {
                    best = best.with_byte_budget(1024 * 1024, MetricRow::retained_bytes);
                }
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
                    let items = Self::walk_order(
                        &sources,
                        &window,
                        |e| e.metrics,
                        None,
                        false,
                        &mut unavailable,
                    )?;
                    let mut reader = TailReader::new(&sources.tail_paths);
                    let mut dctx = zstd::bulk::Decompressor::new()?;
                    for (min, max, source) in items {
                        if after.as_ref().is_some_and(|a| max < a.0) {
                            continue;
                        }
                        if best.threshold().is_some_and(|t| min > t.0) {
                            break;
                        }
                        match source {
                            Source::Block(j) => {
                                let b = &sources.blocks[j];
                                crate::tail::visit_block(
                                    &b.block,
                                    &b.wanted,
                                    &mut dctx,
                                    |e, o| {
                                        if let Some(r) = crate::tail::metric_row(e, o)
                                            && keep(&r)
                                            && kernel::after_page(&key(&r), after.as_ref())
                                        {
                                            best.offer(key(&r), r);
                                        }
                                    },
                                )?;
                            }
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
                if best.budget_exceeded() {
                    return Err(QueryError::Invalid(
                        "query payload budget exceeded; reduce row limit".into(),
                    ));
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
            Query::Spans {
                trace_id,
                name,
                limit,
                ..
            } => {
                let mut best = Smallest::new(*limit as usize + 1);
                if authorized_nodes.is_some() {
                    best = best.with_byte_budget(1024 * 1024, SpanRow::retained_bytes);
                }
                let keep = |r: &SpanRow| {
                    node_ok(&r.node)
                        && trace_id.as_deref().is_none_or(|t| r.trace_id == t)
                        && name.as_deref().is_none_or(|n| r.name == n)
                        && window.contains(r.start_ns)
                        && snapshot.contains(r.group)
                };
                let key = |r: &SpanRow| (r.start_ns, r.node_id, r.sequence, r.index);
                for r in journal_rows.spans.iter().filter(|r| keep(r)) {
                    if kernel::after_page(&key(r), after.as_ref()) {
                        best.offer(key(r), r.clone());
                    }
                }
                if walking {
                    let items = Self::walk_order(
                        &sources,
                        &window,
                        |e| e.spans,
                        trace_id.as_deref(),
                        true,
                        &mut unavailable,
                    )?;
                    let mut reader = TailReader::new(&sources.tail_paths);
                    let mut dctx = zstd::bulk::Decompressor::new()?;
                    for (min, max, source) in items {
                        if after.as_ref().is_some_and(|a| max < a.0) {
                            continue;
                        }
                        if best.threshold().is_some_and(|t| min > t.0) {
                            break;
                        }
                        match source {
                            Source::Block(j) => {
                                let b = &sources.blocks[j];
                                crate::tail::visit_block(
                                    &b.block,
                                    &b.wanted,
                                    &mut dctx,
                                    |e, o| {
                                        if let Some(r) = crate::tail::span_row(e, o)
                                            && keep(&r)
                                            && kernel::after_page(&key(&r), after.as_ref())
                                        {
                                            best.offer(key(&r), r);
                                        }
                                    },
                                )?;
                            }
                            Source::Tail(i) => {
                                for r in reader.rows(&sources.tail[i])?.spans {
                                    if keep(&r) && kernel::after_page(&key(&r), after.as_ref()) {
                                        best.offer(key(&r), r);
                                    }
                                }
                            }
                            Source::Group(si, rg) => {
                                let (dir, manifest) = &sources.segments[si];
                                let scanned = segment::scan_spans_groups(
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
                        let scanned = segment::scan_spans(dir, manifest, from, to, |r| {
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
                if best.budget_exceeded() {
                    return Err(QueryError::Invalid(
                        "query payload budget exceeded; reduce row limit".into(),
                    ));
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
                sorted.iter().map(|(_, r)| span_json(r)).collect()
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
                    let items = Self::walk_order(
                        &sources,
                        &window,
                        |e| e.metrics,
                        None,
                        false,
                        &mut unavailable,
                    )?;
                    let mut reader = TailReader::new(&sources.tail_paths);
                    let mut dctx = zstd::bulk::Decompressor::new()?;
                    for (_, _, source) in items {
                        match source {
                            Source::Block(j) => {
                                let b = &sources.blocks[j];
                                crate::tail::visit_block(
                                    &b.block,
                                    &b.wanted,
                                    &mut dctx,
                                    |e, o| {
                                        if let Some(r) = crate::tail::metric_row(e, o)
                                            && keep(&r)
                                        {
                                            points.push(r);
                                        }
                                    },
                                )?;
                            }
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
        #[cfg(feature = "phase-probe")]
        drop(_execution);
        #[cfg(feature = "phase-probe")]
        let _output = fabric_frame::probe::span("query_envelope_construct");
        // Shared damaged sources cannot be attributed to one enrollment. Reveal
        // neither their path nor global count; preserve explicit uncertainty.
        if authorized_nodes.is_some() && !unavailable.is_empty() {
            unavailable = vec![
                json!({"reason": "authorized coverage cannot be established for an unavailable shared source"}),
            ];
        }
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
                "node": g.node, "sequence": g.sequence, "receive_ns": g.received_ns,
                "gap": if authorized_nodes.is_some() { "collection coverage gap; signal-specific details unavailable" } else { &g.text },
            })).collect::<Vec<_>>(),
            "snapshot": if authorized_nodes.is_some() { "scoped snapshot".to_owned() } else { format!("g{oldest}-{newest}") },
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
            Query::Spans {
                node,
                from_ns,
                to_ns,
                trace_id,
                name,
                limit,
                ..
            } => json!({
                "kind": "spans", "node": node, "from_ns": from_ns, "to_ns": to_ns,
                "trace_id": trace_id, "name": name, "limit": limit,
            }),
        };
        v.serialize(s)
    }
}

/// Per series of a monotonic cumulative sum: a rate per consecutive pair
/// sharing `start_ns` with no decrease, otherwise a reset marker.
fn rates(mut points: Vec<MetricRow>) -> Vec<Value> {
    #[cfg(feature = "phase-probe")]
    let _phase = fabric_frame::probe::span("query_rates");

    let series = |r: &MetricRow| {
        (
            r.node.clone(),
            r.attributes
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>(),
        )
    };
    // Cache the legacy presentation key once. Structural identity breaks
    // delimiter collisions without changing the order of distinct legacy keys.
    points.sort_by_cached_key(|r| {
        (
            series(r),
            r.attributes.clone(),
            r.time_ns,
            r.node_id,
            r.sequence,
            r.index,
        )
    });
    let mut out = Vec::new();
    for pair in points.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if a.node != b.node || a.attributes != b.attributes {
            continue;
        }
        let base = json!({"node": b.node, "name": b.name, "attributes": b.attributes, "time_ns": b.time_ns});
        let mut row = base.as_object().unwrap().clone();
        let point = |r: &MetricRow| CounterSample {
            start_ns: r.start_ns,
            time_ns: r.time_ns,
            value: match r.value {
                Number::Int(i) => kernel::CounterNumber::Int(i),
                Number::Double(d) => kernel::CounterNumber::Double(d),
            },
        };
        match kernel::counter_step_numbers(point(a), point(b)) {
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
