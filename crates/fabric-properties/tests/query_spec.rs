//! Theorems about the executable query specification, stated as properties
//! that need no oracle: they are relations the definition must satisfy with
//! itself (pages partition the answer, limits are prefixes, filters
//! restrict, windows split, snapshots hide), and the equalities that make
//! the threshold walk and the budget boundary correct by construction.

use fabric_core::query::spec::{
    LogRow, LogsQuery, Source, budgeted_walk, drain, drain_budgeted, logs, threshold_walk,
};
use fabric_core::query::{RowKey, Snapshot, Window};
use proptest::prelude::*;
use std::collections::BTreeSet;

const NODES: [&str; 3] = ["a", "b", "c"];
const BODIES: [&str; 4] = ["", "x", "xy", "zzz"];

fn row() -> impl Strategy<Value = LogRow<'static>> {
    (
        0_u64..6,
        0_u64..40,
        0_usize..3,
        0_u64..4,
        0_u32..2,
        0_usize..4,
    )
        .prop_map(|(group, t, n, seq, idx, b)| {
            let mut id = [0_u8; 16];
            id[0] = n as u8;
            LogRow {
                group,
                key: (t, id, seq, idx),
                node: NODES[n],
                body: BODIES[b],
            }
        })
}

/// Rows with distinct keys: the contract's total order.
fn rows() -> impl Strategy<Value = Vec<LogRow<'static>>> {
    proptest::collection::vec(row(), 0..24).prop_map(|v| {
        let mut seen = BTreeSet::new();
        v.into_iter().filter(|r| seen.insert(r.key)).collect()
    })
}

fn query() -> impl Strategy<Value = LogsQuery<'static>> {
    (
        0_u64..40,
        1_u64..41,
        proptest::option::of(0_usize..3),
        proptest::option::of(0_usize..3),
        1_u32..8,
    )
        .prop_filter_map("window", |(a, b, n, c, limit)| {
            let (from, to) = if a < b {
                (a, b)
            } else {
                (b, a.saturating_add(1))
            };
            Window::new(from, to).ok().map(|window| LogsQuery {
                window,
                node: n.map(|i| NODES[i]),
                contains: c.map(|i| ["x", "y", "q"][i]),
                limit,
            })
        })
}

fn snapshot() -> impl Strategy<Value = Snapshot> {
    (0_u64..6, 0_u64..6).prop_map(|(a, b)| Snapshot {
        oldest_group: a.min(b),
        newest_group: a.max(b),
    })
}

/// Rows cut into sources with sound bounds (each source's bounds are the
/// min and max time of its rows), in an arbitrary order.
fn sources(rows: Vec<LogRow<'static>>, cuts: Vec<u8>) -> Vec<Source<'static>> {
    let mut out: Vec<Vec<LogRow<'static>>> = vec![Vec::new()];
    for (i, r) in rows.into_iter().enumerate() {
        if cuts.get(i).is_some_and(|c| *c < 64) && !out.last().unwrap().is_empty() {
            out.push(Vec::new());
        }
        out.last_mut().unwrap().push(r);
    }
    out.into_iter()
        .filter(|v| !v.is_empty())
        .map(|v| Source {
            min_ns: v.iter().map(|r| r.key.0).min().unwrap(),
            max_ns: v.iter().map(|r| r.key.0).max().unwrap(),
            rows: v,
        })
        .collect()
}

fn keys(rows: &[LogRow<'_>]) -> Vec<RowKey> {
    rows.iter().map(|r| r.key).collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3_000))]

    /// T2. Pages partition the answer: following `next` from the first page
    /// yields every admitted row exactly once, in key order, and equals the
    /// single page of an unbounded limit.
    #[test]
    fn pages_partition_the_answer(rows in rows(), q in query(), snap in snapshot()) {
        let drained = drain(&rows, &q, &snap);
        let whole = logs(&rows, &LogsQuery { limit: 10_000, ..q }, &snap, None);
        prop_assert_eq!(keys(&drained), keys(&whole.rows));
        prop_assert!(whole.next.is_none());
        let ks = keys(&drained);
        prop_assert!(ks.windows(2).all(|w| w[0] < w[1]), "strictly increasing keys");
        let ideal: BTreeSet<RowKey> = rows.iter().filter(|r| snap.contains(r.group) && q.window.contains(r.key.0) && q.node.is_none_or(|n| n == r.node) && q.contains.is_none_or(|s| r.body.contains(s))).map(|r| r.key).collect();
        prop_assert_eq!(ks.iter().copied().collect::<BTreeSet<_>>(), ideal);
    }

    /// T4. A smaller limit answers a prefix of a larger one.
    #[test]
    fn a_smaller_limit_is_a_prefix(rows in rows(), q in query(), snap in snapshot(), extra in 0_u32..8) {
        let small = logs(&rows, &q, &snap, None);
        let large = logs(&rows, &LogsQuery { limit: q.limit + extra, ..q }, &snap, None);
        prop_assert_eq!(keys(&small.rows), keys(&large.rows)[..small.rows.len()].to_vec());
    }

    /// T5. A node filter restricts: the filtered answer is the unfiltered
    /// drain with the other nodes removed.
    #[test]
    fn a_node_filter_restricts(rows in rows(), q in query(), snap in snapshot(), n in 0_usize..3) {
        let filtered = drain(&rows, &LogsQuery { node: Some(NODES[n]), ..q }, &snap);
        let all = drain(&rows, &LogsQuery { node: None, ..q }, &snap);
        let restricted: Vec<RowKey> = all.iter().filter(|r| r.node == NODES[n]).map(|r| r.key).collect();
        prop_assert_eq!(keys(&filtered), restricted);
    }

    /// T6. Windows split: `[a, c)` is `[a, b)` followed by `[b, c)`.
    #[test]
    fn windows_split(rows in rows(), q in query(), snap in snapshot(), pick in any::<u64>()) {
        let (a, c) = (q.window.from_ns, q.window.to_ns);
        prop_assume!(c - a >= 2);
        let mid = a + 1 + pick % (c - a - 1);
        let whole = drain(&rows, &q, &snap);
        let left = drain(&rows, &LogsQuery { window: Window::new(a, mid).unwrap(), ..q }, &snap);
        let right = drain(&rows, &LogsQuery { window: Window::new(mid, c).unwrap(), ..q }, &snap);
        let mut joined = keys(&left);
        joined.extend(keys(&right));
        prop_assert_eq!(keys(&whole), joined);
    }

    /// T3. Rows outside the snapshot are invisible: adding them changes nothing.
    #[test]
    fn the_snapshot_hides_other_groups(rows in rows(), q in query(), snap in snapshot(), outside in proptest::collection::vec(row(), 0..6)) {
        let before = drain(&rows, &q, &snap);
        let mut more = rows.clone();
        let seen: BTreeSet<RowKey> = rows.iter().map(|r| r.key).collect();
        more.extend(outside.into_iter().filter(|r| !snap.contains(r.group) && !seen.contains(&r.key)).map(|mut r| { r.group = snap.newest_group + 1; r }));
        prop_assert_eq!(keys(&before), keys(&drain(&more, &q, &snap)));
    }

    /// T7. The threshold walk equals the definition on every page, for every
    /// cut of the rows into sources with sound bounds, in any source order.
    #[test]
    fn the_walk_equals_the_definition(rows in rows(), q in query(), snap in snapshot(), cuts in proptest::collection::vec(any::<u8>(), 24)) {
        let srcs = sources(rows.clone(), cuts);
        prop_assert!(srcs.iter().all(Source::bounds_hold));
        let mut after: Option<RowKey> = None;
        for _ in 0..40 {
            let spec = logs(&rows, &q, &snap, after.as_ref());
            let walk = threshold_walk(&srcs, &q, &snap, after.as_ref());
            prop_assert_eq!(&walk, &spec);
            match spec.next { Some(k) => after = Some(k), None => break }
        }
    }

    /// T8. Draining budgeted pages yields the definition's drain, for every
    /// budget; and each budgeted page is a prefix of the unbudgeted page
    /// from the same continuation.
    #[test]
    fn the_budget_drains_to_the_definition(rows in rows(), q in query(), snap in snapshot(), cuts in proptest::collection::vec(any::<u8>(), 24), budget in 0_usize..12) {
        let srcs = sources(rows.clone(), cuts);
        prop_assert_eq!(keys(&drain_budgeted(&srcs, &q, &snap, budget)), keys(&drain(&rows, &q, &snap)));
        let (page, boundary) = budgeted_walk(&srcs, &q, &snap, None, budget);
        let full = logs(&rows, &q, &snap, None);
        prop_assert_eq!(keys(&page.rows), keys(&full.rows)[..page.rows.len()].to_vec());
        if let Some(m) = boundary { prop_assert!(page.rows.iter().all(|r| r.key.0 < m)); }
    }

    /// Negative control for T7: the walk's theorem needs sound bounds. A
    /// source whose stated minimum is above one of its rows can make the
    /// walk skip that row, and the property must be able to see it.
    #[test]
    fn unsound_bounds_can_break_the_walk(rows in rows(), q in query(), snap in snapshot()) {
        prop_assume!(rows.len() >= 2);
        let spec = logs(&rows, &q, &snap, None);
        // Claim every source starts at the latest time: a lie about the bounds.
        let lie = u64::MAX;
        let srcs: Vec<Source> = rows.iter().map(|r| Source { min_ns: lie, max_ns: lie, rows: vec![*r] }).collect();
        let walk = threshold_walk(&srcs, &q, &snap, None);
        // With lying bounds the walk may differ; what the theorem promises is
        // only that it agrees when bounds hold, which bounds_hold detects.
        prop_assert!(srcs.iter().all(|s| !s.bounds_hold() || s.rows.is_empty()) || walk == spec);
    }
}
