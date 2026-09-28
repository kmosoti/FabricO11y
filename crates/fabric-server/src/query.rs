//! Exact queries over retained history (retained-history contract).
//!
//! A snapshot is the group range `[oldest retained, newest committed]` at the
//! first page. Records never change group, only move from the journal into a
//! segment, so a later page recomputes the same set from wherever the records
//! now live. If retention removed part of the range, the page answers Gone.
//! Rows are kept in a bounded heap of `limit + 1`, so memory does not grow
//! with the number of matching rows.

use crate::rows::{GapRow, LogRow, MetricRow, Number, Rows, extract};
use crate::segment::{self, MAX_GROUP_PAYLOAD, Manifest};
use crate::store::Group;
use fabric_frame::frame::read_frame;
use prost::Message;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BinaryHeap};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

const MAX_LIMIT: u32 = 10_000;

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

type Key = (u64, [u8; 16], u64, u32);

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
        Self {
            state_dir: state_dir.to_path_buf(),
        }
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
        if from >= to {
            return Err(QueryError::Invalid("from_ns must be below to_ns".into()));
        }
        if let Some(limit) = limit
            && !(1..=MAX_LIMIT).contains(&limit)
        {
            return Err(QueryError::Invalid("limit must be 1-10000".into()));
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
        let sources = self.sources(floor, newest)?;
        let oldest = floor.unwrap_or(sources.oldest_group);
        if floor.is_some_and(|f| sources.oldest_group > f) {
            return Err(QueryError::Gone);
        }
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
            node_ok(&g.node)
                && g.received_ns >= from
                && g.received_ns < to
                && g.group >= oldest
                && g.group <= newest
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
                        && r.observed_ns >= from
                        && r.observed_ns < to
                        && r.group >= oldest
                        && r.group <= newest
                        && contains.as_deref().is_none_or(|c| r.body.contains(c))
                };
                let key = |r: &LogRow| (r.observed_ns, r.node_id, r.sequence, r.index);
                for r in journal_rows.logs.iter().filter(|r| keep(r)) {
                    if after.is_none_or(|a| key(r) > a) {
                        best.offer(key(r), r.clone());
                    }
                }
                for (dir, manifest) in &sources.segments {
                    let scanned = segment::scan_logs(dir, manifest, from, to, |r| {
                        if keep(&r) && after.is_none_or(|a| key(&r) > a) {
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
                        && r.time_ns >= from
                        && r.time_ns < to
                        && r.group >= oldest
                        && r.group <= newest
                };
                let key = |r: &MetricRow| (r.time_ns, r.node_id, r.sequence, r.index);
                for r in journal_rows.metrics.iter().filter(|r| keep(r)) {
                    if after.is_none_or(|a| key(r) > a) {
                        best.offer(key(r), r.clone());
                    }
                }
                for (dir, manifest) in &sources.segments {
                    let scanned = segment::scan_metrics(dir, manifest, from, to, |r| {
                        if keep(&r) && after.is_none_or(|a| key(&r) > a) {
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
                        && r.time_ns >= from
                        && r.time_ns < to
                        && r.group >= oldest
                        && r.group <= newest
                };
                points.extend(journal_rows.metrics.iter().filter(|r| keep(r)).cloned());
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
                rates(points)
            }
        };
        let retained = if received.0 == u64::MAX {
            (json!(0), json!(0))
        } else {
            (json!(received.0), json!(received.1))
        };
        Ok(json!({
            "complete": unavailable.is_empty(),
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
        if a.start_ns == b.start_ns && as_f64(b.value) >= as_f64(a.value) && b.time_ns > a.time_ns {
            let rate = (as_f64(b.value) - as_f64(a.value)) / ((b.time_ns - a.time_ns) as f64 / 1e9);
            row.insert("reset".into(), json!(false));
            row.insert("rate".into(), json!(rate));
        } else {
            row.insert("reset".into(), json!(true));
            row.insert("rate".into(), Value::Null);
        }
        out.push(Value::Object(row));
    }
    out
}
