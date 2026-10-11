//! Literal edge cases and a bounded relational reference for the October 11
//! mutation survivors. The reference never calls a Fabric selection function.

use fabric_core::query::spec::{
    LogRow, LogsQuery, Page, Source, budgeted_walk, logs, threshold_walk,
};
use fabric_core::query::{RowKey, Snapshot, Window};

fn row(group: u64, time: u64, id: u8, node: &'static str, body: &'static str) -> LogRow<'static> {
    LogRow {
        group,
        key: (time, [id; 16], 1, 0),
        node,
        body,
    }
}

fn source(rows: Vec<LogRow<'static>>) -> Source<'static> {
    Source {
        min_ns: rows.iter().map(|r| r.key.0).min().unwrap(),
        max_ns: rows.iter().map(|r| r.key.0).max().unwrap(),
        rows,
    }
}

#[test]
fn source_bounds_are_inclusive_and_reject_either_unsound_endpoint() {
    let rows = vec![row(1, 10, 1, "a", "x"), row(1, 20, 2, "b", "y")];
    for (min_ns, max_ns, sound) in [
        (10, 20, true),
        (0, 100, true),
        (11, 20, false),
        (10, 19, false),
        (21, 9, false),
    ] {
        assert_eq!(
            Source {
                min_ns,
                max_ns,
                rows: rows.clone()
            }
            .bounds_hold(),
            sound
        );
    }
    assert!(
        Source {
            min_ns: 10,
            max_ns: 20,
            rows: vec![]
        }
        .bounds_hold()
    );
}

fn reference(
    rows: &[LogRow<'static>],
    q: &LogsQuery<'_>,
    snap: &Snapshot,
    after: Option<&RowKey>,
) -> Page<LogRow<'static>> {
    let mut selected: Vec<_> = rows
        .iter()
        .copied()
        .filter(|r| {
            r.group >= snap.oldest_group
                && r.group <= snap.newest_group
                && r.key.0 >= q.window.from_ns
                && r.key.0 < q.window.to_ns
                && q.node.is_none_or(|n| n == r.node)
                && q.contains.is_none_or(|text| r.body.contains(text))
                && after.is_none_or(|key| r.key > *key)
        })
        .collect();
    selected.sort_by_key(|r| r.key);
    let count = q.limit as usize;
    let next = if selected.len() > count {
        Some(selected[count - 1].key)
    } else {
        None
    };
    selected.truncate(count);
    Page {
        rows: selected,
        next,
    }
}

#[test]
fn filters_ties_overlaps_and_continuations_match_the_relational_reference() {
    let sources = vec![
        source(vec![
            row(0, 0, 9, "a", "x"),
            row(1, 10, 9, "b", "y"),
            row(2, 20, 9, "a", "xy"),
            row(1, 100, 9, "a", "x"),
        ]),
        source(vec![
            row(1, 10, 1, "a", "x"),
            row(2, 10, 2, "b", "x"),
            row(3, 30, 1, "a", "y"),
        ]),
        source(vec![
            row(1, 10, 3, "a", "xy"),
            row(2, 20, 2, "b", "y"),
            row(4, 50, 1, "a", "x"),
        ]),
        source(vec![
            row(2, 20, 1, "a", "x"),
            row(1, 40, 2, "b", "xy"),
            row(3, 60, 1, "b", "y"),
        ]),
    ];
    let rows: Vec<_> = sources
        .iter()
        .flat_map(|s| s.rows.iter().copied())
        .collect();
    let mut cursors = vec![None];
    cursors.extend(rows.iter().map(|r| Some(r.key)));
    cursors.push(Some((10, [0; 16], 0, 0)));
    for (from, to) in [(0, 101), (10, 21), (10, 11), (11, 61)] {
        for (oldest_group, newest_group) in [(0, 4), (1, 3), (2, 2)] {
            let snap = Snapshot {
                oldest_group,
                newest_group,
            };
            for node in [None, Some("a"), Some("b"), Some("absent")] {
                for contains in [None, Some("x"), Some("absent")] {
                    for limit in 1..=5 {
                        let q = LogsQuery {
                            window: Window::new(from, to).unwrap(),
                            node,
                            contains,
                            limit,
                        };
                        for after in &cursors {
                            let expected = reference(&rows, &q, &snap, after.as_ref());
                            assert_eq!(logs(&rows, &q, &snap, after.as_ref()), expected);
                            assert_eq!(
                                threshold_walk(&sources, &q, &snap, after.as_ref()),
                                expected
                            );
                            assert_eq!(
                                budgeted_walk(&sources, &q, &snap, after.as_ref(), usize::MAX),
                                (expected.clone(), None)
                            );
                            for budget in [0, 1, 2, 4, 10] {
                                let (page, boundary) =
                                    budgeted_walk(&sources, &q, &snap, after.as_ref(), budget);
                                if let Some(m) = boundary {
                                    assert!(m > after.map_or(from, |key| key.0.saturating_add(1)));
                                    let cost: usize = sources
                                        .iter()
                                        .filter(|s| {
                                            s.min_ns < m
                                                && after.is_none_or(|key| s.max_ns >= key.0)
                                        })
                                        .map(|s| s.rows.len())
                                        .sum();
                                    assert!(cost >= budget);
                                    let before: Vec<_> =
                                        rows.iter().copied().filter(|r| r.key.0 < m).collect();
                                    let mut prefix = reference(&before, &q, &snap, after.as_ref());
                                    prefix.next = Some((m - 1, [0xff; 16], u64::MAX, u32::MAX));
                                    assert_eq!(page, prefix);
                                } else {
                                    assert_eq!(page, expected);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a_spent_budget_crosses_equal_lower_bounds_before_cutting() {
    let sources = vec![
        source(vec![row(1, 10, 1, "a", "x"), row(1, 11, 1, "a", "x")]),
        source(vec![row(1, 10, 2, "a", "x"), row(1, 12, 2, "a", "x")]),
        source(vec![row(1, 20, 3, "a", "x")]),
    ];
    let q = LogsQuery {
        window: Window::new(0, 100).unwrap(),
        node: None,
        contains: None,
        limit: 100,
    };
    let snap = Snapshot {
        oldest_group: 1,
        newest_group: 1,
    };
    let (page, boundary) = budgeted_walk(&sources, &q, &snap, None, 2);
    assert_eq!(boundary, Some(20));
    assert_eq!(
        page.rows.iter().map(|r| r.key.0).collect::<Vec<_>>(),
        vec![10, 10, 11, 12]
    );
    assert_eq!(page.next, Some((19, [0xff; 16], u64::MAX, u32::MAX)));
}

#[test]
fn equal_time_upper_bound_can_hold_rows_after_the_full_cursor() {
    let rows = vec![row(1, 20, 1, "a", "x"), row(1, 20, 2, "a", "x")];
    let sources = vec![source(rows.clone())];
    let q = LogsQuery {
        window: Window::new(0, 100).unwrap(),
        node: None,
        contains: None,
        limit: 10,
    };
    let snap = Snapshot {
        oldest_group: 1,
        newest_group: 1,
    };
    let after = (20, [0; 16], 0, 0);
    let expected = Page { rows, next: None };
    assert_eq!(threshold_walk(&sources, &q, &snap, Some(&after)), expected);
    assert_eq!(
        budgeted_walk(&sources, &q, &snap, Some(&after), usize::MAX),
        (expected, None)
    );
}
