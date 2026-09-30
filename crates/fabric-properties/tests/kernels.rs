//! Properties of the control, query, retention and collection kernels, each
//! stated from its contract rather than from the code.

use fabric_core::collection::{
    CounterValue, CursorCheck, CursorFacts, FileFacts, MAX_PREFIX_BYTES, bounded_text,
    check_cursor, counter_start,
};
use fabric_core::control::{
    ControlRejection, MAX_DESIRED_LOGS, MAX_DESIRED_PATH_BYTES, MAX_METRIC_INTERVAL_S, Status,
    authorizes, check_desired, may_set_config, set_status, valid_name,
};
use fabric_core::query::{CounterPoint, CounterStep, RowKey, after_page, counter_step};
use fabric_core::retention::{Retention, SegmentFacts, age_cutoff_ns, segments_to_delete};
use proptest::prelude::*;

fn status() -> impl Strategy<Value = Status> {
    prop_oneof![
        Just(Status::Active),
        Just(Status::Paused),
        Just(Status::Revoked)
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    /// CTRL-2: once revoked, every later request is refused and the
    /// credential never authenticates again, whatever the request sequence.
    #[test]
    fn revocation_is_terminal_over_any_request_sequence(
        requests in proptest::collection::vec(status(), 1..20),
    ) {
        let mut current = Status::Active;
        let mut revoked = false;
        for target in requests {
            match set_status(current, target) {
                Ok(next) => {
                    prop_assert!(!revoked, "a revoked enrollment accepted {:?}", target);
                    current = next;
                }
                Err(rejection) => {
                    prop_assert!(revoked);
                    prop_assert_eq!(rejection, ControlRejection::Revoked);
                }
            }
            revoked |= current == Status::Revoked;
            prop_assert_eq!(authorizes(current), !revoked);
            prop_assert_eq!(may_set_config(current).is_ok(), !revoked);
        }
    }

    /// Enrollment names: 1 to 64 characters of `[A-Za-z0-9._-]`.
    #[test]
    fn names_are_accepted_exactly_when_they_match_the_documented_shape(name in "[ -~é]{0,70}") {
        let documented = !name.is_empty()
            && name.chars().count() <= 64
            && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        prop_assert_eq!(valid_name(&name), documented);
    }

    /// CTRL-1: a desired configuration is accepted exactly when its interval,
    /// log count and every path are within the documented limits.
    #[test]
    fn desired_configurations_are_accepted_exactly_within_the_limits(
        interval in 0..4_000_u64,
        paths in proptest::collection::vec("(/?)[a-z/\n\u{0}]{0,250}", 0..20),
    ) {
        let within = (1..=MAX_METRIC_INTERVAL_S).contains(&interval)
            && paths.len() <= MAX_DESIRED_LOGS
            && paths.iter().all(|p| {
                p.starts_with('/') && p.len() <= MAX_DESIRED_PATH_BYTES && !p.contains('\n') && !p.contains('\0')
            });
        prop_assert_eq!(check_desired(interval, &paths).is_ok(), within);
    }

    /// HIST-4: paging through a snapshot with any limit returns every row
    /// exactly once, in order, never repeating or skipping one.
    #[test]
    fn pages_cover_every_row_once_in_order(
        mut keys in proptest::collection::btree_set(
            (0..4_u64, any::<[u8; 16]>(), 0..3_u64, 0..3_u32), 0..60),
        limit in 1..8_usize,
    ) {
        let rows: Vec<RowKey> = std::mem::take(&mut keys).into_iter().collect();
        let mut seen = Vec::new();
        let mut after: Option<RowKey> = None;
        // Every page must make progress, so more pages than rows means the
        // continuation repeats rows forever.
        for _ in 0..=rows.len() {
            let page: Vec<RowKey> = rows
                .iter()
                .filter(|k| after_page(k, after.as_ref()))
                .take(limit)
                .copied()
                .collect();
            if page.is_empty() {
                break;
            }
            prop_assert!(page.first() != after.as_ref(), "a page repeats the last row of the previous one");
            after = page.last().copied();
            seen.extend(page);
        }
        prop_assert_eq!(seen, rows);
    }

    /// HIST-6: a rate appears only between two points of one uninterrupted
    /// series (same start, time advances, value does not decrease), and it
    /// is the value increase per second; everything else is a reset.
    #[test]
    fn rates_come_only_from_uninterrupted_series(
        start_a in 0..3_u64, start_b in 0..3_u64,
        t1 in 0..10_u64, t2 in 0..10_u64,
        v1 in 0..1_000_u32, v2 in 0..1_000_u32,
    ) {
        let a = CounterPoint { start_ns: start_a, time_ns: t1 * 1_000_000_000, value: f64::from(v1) };
        let b = CounterPoint { start_ns: start_b, time_ns: t2 * 1_000_000_000, value: f64::from(v2) };
        match counter_step(a, b) {
            CounterStep::Rate(rate) => {
                prop_assert!(start_a == start_b && t2 > t1 && v2 >= v1);
                let expected = f64::from(v2 - v1) / (t2 - t1) as f64;
                prop_assert!((rate - expected).abs() <= 1e-9 * expected.max(1.0));
            }
            CounterStep::Reset => prop_assert!(!(start_a == start_b && t2 > t1 && v2 >= v1)),
        }
    }

    /// HIST-5: retention deletes the shortest oldest-first prefix after which
    /// the oldest remaining Segment is within the age limit and the remaining
    /// Segments fit the byte limit.
    #[test]
    fn retention_deletes_the_shortest_sufficient_oldest_prefix(
        segments in proptest::collection::vec((0..100_u64, 0..50_u64), 0..12),
        max_age_s in 0..60_u64,
        max_bytes in 0..300_u64,
        now_s in 0..120_u64,
    ) {
        let facts: Vec<SegmentFacts> = segments
            .iter()
            .map(|(t, b)| SegmentFacts { received_max_ns: t * 1_000_000_000, bytes: *b })
            .collect();
        let retention = Retention { max_age_s, max_bytes };
        let now_ns = now_s * 1_000_000_000;
        let cutoff = age_cutoff_ns(now_ns, max_age_s);
        let sufficient = |n: usize| {
            let rest = &facts[n..];
            let total: u64 = rest.iter().map(|s| s.bytes).sum();
            rest.first().is_none_or(|s| s.received_max_ns >= cutoff) && total <= max_bytes
        };
        let deleted = segments_to_delete(&facts, retention, now_ns);
        prop_assert!(deleted <= facts.len());
        prop_assert!(sufficient(deleted), "deleting {} leaves the limits exceeded", deleted);
        prop_assert!(deleted == 0 || !sufficient(deleted - 1), "deleting {} is more than needed", deleted);
    }

    /// Gap texts: the longest prefix of at most `cap` bytes that is still
    /// valid UTF-8 ending on a character boundary.
    #[test]
    fn bounded_text_is_the_longest_valid_prefix_within_the_cap(message in "\\PC{0,40}", cap in 0..80_usize) {
        let bounded = bounded_text(&message, cap);
        prop_assert!(message.starts_with(bounded));
        prop_assert!(bounded.len() <= cap);
        let next = message[bounded.len()..].chars().next();
        prop_assert!(next.is_none_or(|c| bounded.len() + c.len_utf8() > cap));
    }

    /// SPOOL-1: a cursor never lets the reader skip bytes of a file it cannot
    /// show is the same file: a different identity, a shorter file or a
    /// missing witness restarts from byte zero, and a witness is checked.
    #[test]
    fn cursors_never_skip_a_file_they_cannot_identify(
        old_device in 0..2_u64, old_inode in 0..2_u64, offset in 0..200_u64, prefix_len in 0..100_u32,
        device in 0..2_u64, inode in 0..2_u64, len in 0..200_u64,
    ) {
        let old = CursorFacts { device: old_device, inode: old_inode, offset, prefix_len };
        let file = FileFacts { device, inode, len };
        let same_file = old_device == device && old_inode == inode && len >= offset;
        match check_cursor(old, file) {
            Ok(CursorCheck::Continue) => prop_assert!(same_file && offset == 0),
            Ok(CursorCheck::VerifyPrefix { len: n }) => {
                prop_assert!(same_file && n == prefix_len && n <= MAX_PREFIX_BYTES && u64::from(n) <= offset);
            }
            Ok(CursorCheck::Restart) => prop_assert!(!same_file || (offset > 0 && prefix_len == 0)),
            Err(_) => prop_assert!(same_file && prefix_len > 0 && (prefix_len > MAX_PREFIX_BYTES || u64::from(prefix_len) > offset)),
        }
    }

    /// SPOOL-1 fidelity: a counter that did not decrease keeps its start; a
    /// decrease or a change of value type starts a new series now.
    #[test]
    fn counter_starts_follow_the_series(
        old in 0..100_u64, new in 0..100_u64, as_double in any::<bool>(), type_change in any::<bool>(),
        prior_start in 0..1_000_u64, now in 1_000..2_000_u64,
    ) {
        let value = |v: u64, double: bool| if double { CounterValue::Double(v as f64) } else { CounterValue::Int(v) };
        let previous = value(old, as_double);
        let current = value(new, as_double ^ type_change);
        let start = counter_start(Some((previous, prior_start)), current, None, now);
        let continues = !type_change && new >= old;
        prop_assert_eq!(start, if continues { prior_start } else { now });
    }
}
