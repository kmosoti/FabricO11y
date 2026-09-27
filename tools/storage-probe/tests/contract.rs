//! Black-box contract tests for the in-memory storage probe.
//! Expected positions are computed from the input events before Snapshot owns them.

use std::num::NonZeroUsize;

use fabric_o11y::{
    Event, EventId, EventTime, ObservedTime, Payload, ResourceId, SourceId, TenantId,
};
use storage_probe::{Mode, Query, Snapshot};

fn log(id: u64, tenant: u64, time: i64, body: impl Into<String>) -> Event {
    event(id, tenant, time, Payload::Log { body: body.into() })
}

fn gauge(id: u64, tenant: u64, time: i64) -> Event {
    event(
        id,
        tenant,
        time,
        Payload::Gauge {
            name: "temperature".into(),
            value: 1.0,
            unit: "C".into(),
        },
    )
}

fn event(id: u64, tenant: u64, time: i64, payload: Payload) -> Event {
    Event {
        id: EventId(id),
        tenant: TenantId(tenant),
        source: SourceId(1),
        resource: ResourceId(1),
        event_time: EventTime(time),
        observed_time: ObservedTime(0),
        attributes: vec![],
        payload,
    }
}

fn query(start_ns: i64, end_ns: i64, tenant: Option<u64>, token: Option<&str>) -> Query {
    Query {
        start_ns,
        end_ns,
        tenant,
        token: token.map(str::to_owned),
    }
}

fn expected_positions(events: &[Event], q: &Query) -> Vec<usize> {
    events
        .iter()
        .enumerate()
        .filter_map(|(position, event)| {
            let time = event.event_time.0;
            let in_time = q.start_ns <= time && time <= q.end_ns;
            let in_tenant = q.tenant.is_none_or(|tenant| tenant == event.tenant.0);
            let has_token = match (&q.token, &event.payload) {
                (None, _) => true,
                (Some(token), Payload::Log { body }) => {
                    body.split_whitespace().any(|word| word == token)
                }
                (Some(_), Payload::Gauge { .. }) => false,
            };
            (in_time && in_tenant && has_token).then_some(position)
        })
        .collect()
}

fn compare_modes(events: Vec<Event>, block_size: usize, queries: &[Query]) {
    let expected: Vec<_> = queries
        .iter()
        .map(|q| expected_positions(&events, q))
        .collect();
    let event_count = events.len();
    let block_count = event_count.div_ceil(block_size);
    let mut snapshot = Snapshot::new(events, NonZeroUsize::new(block_size).unwrap());
    assert_eq!(snapshot.summary_bytes(), 528 * block_count);

    for (q, positions) in queries.iter().zip(&expected) {
        for mode in [Mode::Scan, Mode::Pruned] {
            let result = snapshot.query(q, mode);
            assert_eq!(result.positions, *positions);
            match mode {
                Mode::Scan => {
                    assert_eq!(result.scanned_events, event_count);
                    assert_eq!(result.skipped_blocks, 0);
                }
                Mode::Pruned => {
                    assert!(result.scanned_events <= event_count);
                    assert!(result.skipped_blocks <= block_count);
                }
            }
        }
    }

    snapshot.disable_summaries();
    assert_eq!(snapshot.summary_bytes(), 0);
    for (q, positions) in queries.iter().zip(&expected) {
        let result = snapshot.query(q, Mode::Pruned);
        assert_eq!(result.positions, *positions);
        assert_eq!(result.scanned_events, event_count);
        assert_eq!(result.skipped_blocks, 0);
    }
}

#[test]
fn empty_and_partial_blocks_preserve_positions() {
    let queries = [
        query(i64::MIN, i64::MAX, None, None),
        query(0, 0, None, None),
        query(1, 0, None, None),
        query(i64::MIN, i64::MAX, Some(7), Some("x")),
    ];
    compare_modes(vec![], 3, &queries);
    compare_modes(
        vec![log(42, 7, 0, "x"), gauge(42, 7, 0), log(42, 7, 0, "x")],
        2,
        &queries,
    );
}

#[test]
fn exhaustive_predicate_combinations_match_independent_evaluator() {
    let events = vec![
        log(9, 1, i64::MAX, "alpha beta"),
        gauge(9, 1, 0),
        log(9, 2, i64::MIN, "Alpha alpha, αλφα"),
        log(2, 2, -1, "alpha\tβeta\u{2003}omega"),
        gauge(2, 2, i64::MAX),
        log(2, 1, 0, "alpha alpha"),
        log(2, 1, 1, ""),
    ];
    let boundaries = [i64::MIN, -1, 0, 1, i64::MAX];
    let mut queries = Vec::new();
    for &start in &boundaries {
        for &end in &boundaries {
            for tenant in [None, Some(1), Some(2), Some(99)] {
                for token in [
                    None,
                    Some("alpha"),
                    Some("Alpha"),
                    Some("alpha,"),
                    Some("βeta"),
                    Some("missing"),
                    Some(""),
                ] {
                    queries.push(query(start, end, tenant, token));
                }
            }
        }
    }
    compare_modes(events, 3, &queries);
}

#[test]
fn unsorted_block_extrema_are_used_for_safe_time_pruning() {
    // The first block's first and last timestamps are 10 and 11, but its
    // actual minimum is -100. Treating endpoints as extrema loses position 1.
    let events = vec![
        log(1, 1, 10, "x"),
        log(1, 1, -100, "x"),
        log(1, 1, 11, "x"),
        log(1, 1, 200, "x"),
        log(1, 1, 201, "x"),
        log(1, 1, 202, "x"),
        log(1, 1, 300, "x"),
    ];
    let q = query(-100, -100, None, None);
    assert_eq!(expected_positions(&events, &q), vec![1]);
    let snapshot = Snapshot::new(events, NonZeroUsize::new(3).unwrap());
    let result = snapshot.query(&q, Mode::Pruned);
    assert_eq!(result.positions, vec![1]);
    assert_eq!(result.scanned_events, 3);
    assert_eq!(result.skipped_blocks, 2);
}

#[test]
fn selective_time_query_prunes_complete_and_partial_blocks() {
    let events = vec![
        gauge(0, 1, -10),
        gauge(1, 1, -9),
        gauge(2, 1, 5),
        gauge(3, 1, 6),
        gauge(4, 1, 100),
    ];
    let snapshot = Snapshot::new(events, NonZeroUsize::new(2).unwrap());
    let result = snapshot.query(&query(5, 5, None, None), Mode::Pruned);
    assert_eq!(result.positions, vec![2]);
    assert_eq!(result.scanned_events, 2);
    assert_eq!(result.skipped_blocks, 2);
}

#[test]
fn saturated_token_sets_can_over_scan_but_cannot_omit_matches() {
    let mut events = Vec::new();
    for index in 0..257_u64 {
        let body = format!("common token{index} repeated common");
        events.push(log(index % 4, index % 3, index as i64 % 17, body));
    }
    events.push(gauge(0, 0, 4));
    let queries = [
        query(0, 16, Some(0), Some("token255")),
        query(0, 16, None, Some("token999")),
        query(0, 16, None, Some("common")),
        query(4, 4, Some(0), None),
    ];
    compare_modes(events, 7, &queries);
}

#[test]
fn varied_seeded_data_matches_independent_evaluator() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut events = Vec::new();
    for _ in 0..101 {
        let id = next() % 8;
        let tenant = next() % 4;
        let time = (next() % 51) as i64 - 25;
        let payload = if next() % 3 == 0 {
            Payload::Gauge {
                name: "g".into(),
                value: 1.0,
                unit: "u".into(),
            }
        } else {
            let token = ["red", "blue", "RED", "red,", "λ"][next() as usize % 5];
            Payload::Log {
                body: format!("head\u{2003}{token} tail"),
            }
        };
        events.push(event(id, tenant, time, payload));
    }
    let queries = [
        query(-25, 25, None, None),
        query(-5, 5, Some(2), Some("red")),
        query(7, 7, Some(1), Some("RED")),
        query(-25, -15, None, Some("λ")),
        query(20, 25, Some(3), Some("missing")),
        query(25, -25, None, None),
    ];
    compare_modes(events, 8, &queries);
}

#[test]
fn dense_single_block_retains_inserted_tokens_and_extreme_tenants() {
    // Many more distinct tokens than filter bits: negative results cannot
    // be assumed even though every inserted token must remain discoverable.
    let mut events = Vec::new();
    for index in 0..8192_u64 {
        events.push(log(0, u64::MAX - index, 0, format!("word{index}")));
    }
    let queries: Vec<_> = (0..128_u64)
        .map(|i| {
            let index = i * 61;
            query(0, 0, Some(u64::MAX - index), Some(&format!("word{index}")))
        })
        .collect();
    compare_modes(events, 8192, &queries);
}
