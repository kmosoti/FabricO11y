//! The executable specification of a query answer.
//!
//! A definition, not an implementation: given every row the retained
//! history holds, in any order, these functions say what the answer *is*,
//! by the shortest reading of the [retained-history contract] that a reader
//! can check line by line. Adapters (the journal tail, Parquet Segments, the
//! key index, the threshold walk, the budget boundary) are correct when they
//! answer exactly as these functions do; the properties in
//! `fabric-properties` and the harnesses in `proofs.rs` state the theorems
//! that make the definition trustworthy on its own terms: pages partition
//! the answer, limits are prefixes, filters restrict, windows split, and
//! the walk and the budget boundary equal the definition.
//!
//! No oracle program stands behind this module. Its claim to be right is
//! that it is short, pure, and consists of the contract's own words: filter,
//! sort by the total key, take `limit`.
//!
//! Rows are abstract: the spec does not know bytes, OTLP, Parquet or files,
//! only a row's group, key and the fields a query mentions. Deriving rows
//! from records is a separate refinement (the server's `rows.rs`).
//!
//! [retained-history contract]: ../../../../docs/architecture/retained-history.md

use super::{RowKey, Snapshot, Window, after_page};
use alloc::vec::Vec;

/// A log row as the contract sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogRow<'a> {
    /// The journal group the row's record was committed in.
    pub group: u64,
    /// `(observed_ns, node_id, sequence, index)`.
    pub key: RowKey,
    /// The credential label.
    pub node: &'a str,
    pub body: &'a str,
}

/// A log search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogsQuery<'a> {
    pub window: Window,
    pub node: Option<&'a str>,
    pub contains: Option<&'a str>,
    /// 1 to `MAX_LIMIT`, checked by `check_limit`.
    pub limit: u32,
}

/// One page: the rows, and the key the next page continues after, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub rows: Vec<T>,
    pub next: Option<RowKey>,
}

/// Whether a row belongs to the answer of `q` over `snapshot`, continuing
/// after `after`: the contract's filter, nothing else.
pub fn admits(
    q: &LogsQuery<'_>,
    snapshot: &Snapshot,
    after: Option<&RowKey>,
    row: &LogRow<'_>,
) -> bool {
    snapshot.contains(row.group)
        && q.window.contains(row.key.0)
        && q.node.is_none_or(|n| n == row.node)
        && q.contains.is_none_or(|s| row.body.contains(s))
        && after_page(&row.key, after)
}

/// The definition of a page: the admitted rows in key order, the first
/// `limit` of them, and a continuation key when more remain.
///
/// Precondition: keys are distinct across `rows` (the total order of the
/// contract). The definition sorts; it does not depend on the input order.
pub fn logs<'a>(
    rows: &[LogRow<'a>],
    q: &LogsQuery<'_>,
    snapshot: &Snapshot,
    after: Option<&RowKey>,
) -> Page<LogRow<'a>> {
    let mut admitted: Vec<LogRow<'a>> = rows
        .iter()
        .filter(|r| admits(q, snapshot, after, r))
        .copied()
        .collect();
    admitted.sort_by_key(|r| r.key);
    let limit = q.limit as usize;
    let next = if admitted.len() > limit {
        admitted.get(limit.wrapping_sub(1)).map(|r| r.key)
    } else {
        None
    };
    admitted.truncate(limit);
    Page {
        rows: admitted,
        next,
    }
}

/// Every page in order until the last: the whole answer, as a client that
/// follows `next_page` receives it. Terminates because each page's
/// continuation key is strictly above the last, and keys are finite.
pub fn drain<'a>(rows: &[LogRow<'a>], q: &LogsQuery<'_>, snapshot: &Snapshot) -> Vec<LogRow<'a>> {
    let mut out = Vec::new();
    let mut after: Option<RowKey> = None;
    loop {
        let page = logs(rows, q, snapshot, after.as_ref());
        let next = page.next;
        out.extend(page.rows);
        match next {
            Some(k) => after = Some(k),
            None => return out,
        }
    }
}

// ---- the mechanisms, in the specification's own terms ---------------------

/// A source: a set of rows with the time bounds a manifest, a Parquet
/// footer or the tail index would state for it. The bounds must be sound:
/// every row's time lies within them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source<'a> {
    pub min_ns: u64,
    pub max_ns: u64,
    pub rows: Vec<LogRow<'a>>,
}

impl Source<'_> {
    /// Sound bounds: no row lies outside them.
    pub fn bounds_hold(&self) -> bool {
        self.rows
            .iter()
            .all(|r| r.key.0 >= self.min_ns && r.key.0 <= self.max_ns)
    }
}

/// Keeps the `capacity` smallest keys offered; the heap of the walk, written
/// as a sorted vector so that its meaning is plain.
struct Smallest<'a> {
    rows: Vec<LogRow<'a>>,
    capacity: usize,
}

impl<'a> Smallest<'a> {
    fn offer(&mut self, row: LogRow<'a>) {
        if self.rows.len() == self.capacity
            && self.rows.last().is_some_and(|last| row.key >= last.key)
        {
            return;
        }
        self.rows.push(row);
        self.rows.sort_by_key(|r| r.key);
        self.rows.truncate(self.capacity);
    }
    /// The largest held key once full: nothing at or above it can enter.
    fn threshold(&self) -> Option<RowKey> {
        if self.rows.len() == self.capacity {
            self.rows.last().map(|r| r.key)
        } else {
            None
        }
    }
}

/// The threshold walk (ledger L-04): sources in order of their lower time
/// bound; a source whose upper bound is below the page start is skipped; the
/// walk stops once the heap is full and the next source's lower bound is
/// above the threshold's time. Equal to [`logs`] whenever every source's
/// bounds hold (a theorem, stated in the properties and proofs).
pub fn threshold_walk<'a>(
    sources: &[Source<'a>],
    q: &LogsQuery<'_>,
    snapshot: &Snapshot,
    after: Option<&RowKey>,
) -> Page<LogRow<'a>> {
    let mut order: Vec<&Source<'a>> = sources.iter().collect();
    order.sort_by_key(|s| s.min_ns);
    let limit = q.limit as usize;
    let mut heap = Smallest {
        rows: Vec::new(),
        capacity: limit.saturating_add(1),
    };
    for source in order {
        if after.is_some_and(|a| source.max_ns < a.0) {
            continue;
        }
        if heap.threshold().is_some_and(|t| source.min_ns > t.0) {
            break;
        }
        for row in source.rows.iter().filter(|r| admits(q, snapshot, after, r)) {
            heap.offer(*row);
        }
    }
    let mut rows = heap.rows;
    let next = if rows.len() > limit {
        rows.get(limit.wrapping_sub(1)).map(|r| r.key)
    } else {
        None
    };
    rows.truncate(limit);
    Page { rows, next }
}

/// A budgeted page (ledger L-05): the walk with a budget of rows examined.
/// When the budget is spent before a source whose lower bound is strictly
/// above the page start and above the last source read, the page ends at
/// that bound `m`: it holds exactly the admitted rows with time below `m`,
/// and continues at `m`. Returns the page and the boundary, if one was taken.
///
/// Theorem (properties and proofs): draining budgeted pages yields the same
/// rows, in the same order, as [`drain`]; every budgeted page's rows are a
/// prefix of the unbudgeted page's rows from the same continuation.
pub fn budgeted_walk<'a>(
    sources: &[Source<'a>],
    q: &LogsQuery<'_>,
    snapshot: &Snapshot,
    after: Option<&RowKey>,
    budget: usize,
) -> (Page<LogRow<'a>>, Option<u64>) {
    let mut order: Vec<&Source<'a>> = sources.iter().collect();
    order.sort_by_key(|s| s.min_ns);
    let limit = q.limit as usize;
    let mut heap = Smallest {
        rows: Vec::new(),
        capacity: limit.saturating_add(1),
    };
    let mut examined = 0usize;
    let mut last_min: Option<u64> = None;
    let mut boundary: Option<u64> = None;
    for source in order {
        if after.is_some_and(|a| source.max_ns < a.0) {
            continue;
        }
        if heap.threshold().is_some_and(|t| source.min_ns > t.0) {
            break;
        }
        if examined >= budget
            && after.is_none_or(|a| source.min_ns > a.0.saturating_add(1))
            && last_min.is_none_or(|l| source.min_ns > l)
        {
            boundary = Some(source.min_ns);
            break;
        }
        last_min = Some(source.min_ns);
        examined = examined.saturating_add(source.rows.len());
        for row in source.rows.iter().filter(|r| admits(q, snapshot, after, r)) {
            heap.offer(*row);
        }
    }
    let mut rows = heap.rows;
    let mut next = if rows.len() > limit {
        rows.get(limit.wrapping_sub(1)).map(|r| r.key)
    } else {
        None
    };
    rows.truncate(limit);
    if let Some(m) = boundary {
        rows.retain(|r| r.key.0 < m);
        if rows.len() <= limit {
            next = Some((m.saturating_sub(1), [0xff; 16], u64::MAX, u32::MAX));
        }
    }
    (Page { rows, next }, boundary)
}

/// Every budgeted page in order until the last.
pub fn drain_budgeted<'a>(
    sources: &[Source<'a>],
    q: &LogsQuery<'_>,
    snapshot: &Snapshot,
    budget: usize,
) -> Vec<LogRow<'a>> {
    let mut out = Vec::new();
    let mut after: Option<RowKey> = None;
    loop {
        let (page, _) = budgeted_walk(sources, q, snapshot, after.as_ref(), budget);
        let next = page.next;
        out.extend(page.rows);
        match next {
            Some(k) if after.is_none_or(|a| k > a) => after = Some(k),
            _ => return out,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn row(
        group: u64,
        t: u64,
        node: &'static str,
        seq: u64,
        body: &'static str,
    ) -> LogRow<'static> {
        let mut id = [0_u8; 16];
        id[0] = node.len() as u8;
        LogRow {
            group,
            key: (t, id, seq, 0),
            node,
            body,
        }
    }

    fn q(from: u64, to: u64, limit: u32) -> LogsQuery<'static> {
        LogsQuery {
            window: Window::new(from, to).unwrap(),
            node: None,
            contains: None,
            limit,
        }
    }

    #[test]
    fn a_page_is_the_sorted_filtered_prefix() {
        let rows = vec![
            row(3, 30, "b", 1, "x"),
            row(1, 10, "a", 1, "x"),
            row(2, 20, "a", 2, "y"),
            row(4, 40, "a", 3, "x"),
        ];
        let snap = Snapshot {
            oldest_group: 1,
            newest_group: 3,
        };
        let page = logs(&rows, &q(0, 100, 2), &snap, None);
        assert_eq!(
            page.rows.iter().map(|r| r.key.0).collect::<Vec<_>>(),
            vec![10, 20]
        );
        assert_eq!(page.next, Some(rows[2].key));
        let page2 = logs(&rows, &q(0, 100, 2), &snap, page.next.as_ref());
        assert_eq!(
            page2.rows.iter().map(|r| r.key.0).collect::<Vec<_>>(),
            vec![30],
            "group 4 is outside the snapshot"
        );
        assert_eq!(page2.next, None);
        assert_eq!(drain(&rows, &q(0, 100, 2), &snap).len(), 3);
        let filtered = logs(
            &rows,
            &LogsQuery {
                contains: Some("y"),
                ..q(0, 100, 10)
            },
            &snap,
            None,
        );
        assert_eq!(filtered.rows.len(), 1);
    }

    #[test]
    fn the_walk_and_the_budget_equal_the_definition_on_an_overlapping_case() {
        let a = vec![row(1, 10, "a", 1, "x"), row(1, 50, "a", 2, "x")];
        let b = vec![row(2, 20, "b", 1, "x"), row(2, 60, "b", 2, "x")];
        let c = vec![row(3, 70, "c", 1, "x")];
        let sources = vec![
            Source {
                min_ns: 10,
                max_ns: 50,
                rows: a.clone(),
            },
            Source {
                min_ns: 20,
                max_ns: 60,
                rows: b.clone(),
            },
            Source {
                min_ns: 70,
                max_ns: 70,
                rows: c.clone(),
            },
        ];
        let all: Vec<LogRow> = a.into_iter().chain(b).chain(c).collect();
        let snap = Snapshot {
            oldest_group: 1,
            newest_group: 3,
        };
        for limit in 1..=5 {
            let spec = logs(&all, &q(0, 100, limit), &snap, None);
            assert_eq!(
                threshold_walk(&sources, &q(0, 100, limit), &snap, None),
                spec
            );
            for budget in [0, 1, 2, 3, 10] {
                assert_eq!(
                    drain_budgeted(&sources, &q(0, 100, limit), &snap, budget),
                    drain(&all, &q(0, 100, limit), &snap),
                    "limit {limit} budget {budget}"
                );
            }
        }
    }
}
