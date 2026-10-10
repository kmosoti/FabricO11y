use fabric_ui::model::*;

#[test]
fn boundary_walk_equals_direct_floor_for_every_small_grid() {
    for span in 1..100u64 {
        for buckets in 1..64usize {
            let samples: Vec<_> = (0..=span)
                .map(|offset| Sample {
                    time_ns: 1_700_000_000_000_000_000 + offset,
                    value: if offset % 7 == 3 {
                        None
                    } else {
                        Some(offset as f64)
                    },
                })
                .collect();
            let window = TimeWindow {
                start_ns: samples[0].time_ns,
                end_ns: samples.last().unwrap().time_ns,
            };
            let actual = chart_envelope(&samples, window, buckets).unwrap();
            for bucket in actual {
                let members: Vec<_> = samples
                    .iter()
                    .filter(|s| {
                        (((s.time_ns - window.start_ns) as usize * buckets) / span as usize)
                            .min(buckets - 1)
                            == bucket.index
                    })
                    .collect();
                assert_eq!(bucket.has_gap, members.iter().any(|s| s.value.is_none()));
                let numeric: Vec<_> = members.into_iter().filter(|s| s.value.is_some()).collect();
                assert_eq!(
                    bucket.min.map(|p| (p.time_ns, p.value)),
                    numeric.first().map(|s| (s.time_ns, s.value.unwrap()))
                );
                assert_eq!(
                    bucket.max.map(|p| (p.time_ns, p.value)),
                    numeric.last().map(|s| (s.time_ns, s.value.unwrap()))
                );
            }
        }
    }
}

fn row(sequence: u64, text: &str) -> TailRow {
    TailRow {
        id: RowId {
            enrollment: [1; 16],
            generation: 1,
            batch_sequence: sequence,
            ordinal: 0,
            signal: Signal::Logs,
        },
        text: text.to_owned(),
    }
}

#[test]
fn old_account_cannot_publish_or_free_a_new_request() {
    let mut c = RequestCoordinator::new();
    assert_eq!(c.start(), Err(ModelError::LoggedOut));
    c.login().unwrap();
    let old = c.start().unwrap();
    c.logout().unwrap();
    c.login().unwrap();
    assert!(c.is_busy());
    assert_eq!(c.start(), Err(ModelError::Busy));
    let forged = RequestToken {
        request_id: old.request_id + 1,
        ..old
    };
    assert_eq!(c.complete(forged), Completion::Unknown);
    assert!(c.is_busy());
    assert_eq!(c.complete(old), Completion::Stale);
    let new = c.start().unwrap();
    assert_eq!(c.complete(old), Completion::Unknown);
    assert!(c.is_busy());
    assert_eq!(c.complete(new), Completion::Accepted);
}

#[test]
fn changing_query_invalidates_without_releasing_work() {
    let mut c = RequestCoordinator::new();
    c.login().unwrap();
    let old = c.start().unwrap();
    c.change_query().unwrap();
    assert_eq!(c.start(), Err(ModelError::Busy));
    assert_eq!(c.complete(old), Completion::Stale);
    let next = c.start().unwrap();
    assert!(next.query_epoch > old.query_epoch);
    assert_eq!(c.complete(next), Completion::Accepted);
}

#[test]
fn utf8_limit_refuses_oversize_without_evicting() {
    let mut tail = TailBuffer::new(2, 3).unwrap();
    tail.push(row(1, "abc"));
    assert_eq!(
        tail.push(row(2, "😀")),
        InsertResult {
            status: InsertStatus::Oversize,
            evicted: 0
        }
    );
    assert_eq!(
        tail.rows().map(|r| r.id.batch_sequence).collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(tail.utf8_bytes(), 3);
    assert_eq!(
        tail.dropped(),
        DropCount {
            count: 1,
            saturated: false
        }
    );
    // Representative faulty character-count admission would accept this input.
    assert!("😀".chars().count() <= 3);
    assert!("😀".len() > 3);
}

#[test]
fn exact_identity_deduplicates_and_conflicts_do_not_replace() {
    let mut tail = TailBuffer::new(2, 10).unwrap();
    tail.push(row(1, "a"));
    assert_eq!(tail.push(row(1, "a")).status, InsertStatus::Duplicate);
    assert_eq!(tail.push(row(1, "b")).status, InsertStatus::Conflict);
    let mut another_signal = row(1, "c");
    another_signal.id.signal = Signal::Traces;
    assert_eq!(tail.push(another_signal).status, InsertStatus::Inserted);
    assert_eq!(
        tail.rows().map(|r| r.text.as_str()).collect::<Vec<_>>(),
        vec!["a", "c"]
    );
    assert_eq!(tail.dropped().count, 0);
}

#[test]
fn evicts_whole_oldest_rows_for_both_limits() {
    let mut tail = TailBuffer::new(2, 5).unwrap();
    tail.push(row(1, "ab"));
    tail.push(row(2, "cd"));
    assert_eq!(
        tail.push(row(3, "xyz")),
        InsertResult {
            status: InsertStatus::Inserted,
            evicted: 1
        }
    );
    assert_eq!(tail.utf8_bytes(), 5);
    assert_eq!(tail.push(row(4, "12345")).evicted, 2);
    assert_eq!(
        tail.rows().map(|r| r.id.batch_sequence).collect::<Vec<_>>(),
        vec![4]
    );
    assert_eq!(tail.dropped().count, 3);
    // Evicted identities can return; the dedup horizon is explicitly retained-only.
    assert_eq!(tail.push(row(1, "")).status, InsertStatus::Inserted);
    assert_eq!(tail.push(row(5, "")).evicted, 1);
    assert_eq!(tail.utf8_bytes(), 0);
    assert_eq!(tail.rows().count(), 2);
}

#[test]
fn zero_and_hard_tail_budgets_are_explicit() {
    let mut tail = TailBuffer::new(0, 0).unwrap();
    assert_eq!(tail.push(row(1, "")).status, InsertStatus::Oversize);
    assert_eq!(tail.dropped().count, 1);
    assert!(matches!(
        TailBuffer::new(MAX_TAIL_ROWS + 1, 0),
        Err(ModelError::Budget)
    ));
    assert!(matches!(
        TailBuffer::new(1, MAX_TAIL_BYTES + 1),
        Err(ModelError::Budget)
    ));
    let mut zero_bytes = TailBuffer::new(2, 0).unwrap();
    assert_eq!(zero_bytes.push(row(1, "")).status, InsertStatus::Oversize);
}

#[test]
fn retained_capacity_and_unicode_are_bounded_independently_of_input_capacity() {
    let mut oversized_capacity = String::with_capacity(1_000_000);
    oversized_capacity.push('a');
    let mut incoming = row(1, "");
    incoming.text = oversized_capacity;
    let mut tail = TailBuffer::new(3, 10).unwrap();
    assert_eq!(tail.push(incoming).status, InsertStatus::Inserted);
    let retained = tail.rows().next().unwrap();
    assert_eq!(retained.text.len(), 1);
    assert_eq!(retained.text.capacity(), 1);
    assert_eq!(tail.push(row(2, "é😀")).status, InsertStatus::Inserted);
    assert_eq!(tail.utf8_bytes(), 7);
    assert_eq!(tail.push(row(2, "é😀")).status, InsertStatus::Duplicate);
    assert_eq!(tail.push(row(2, "e😀")).status, InsertStatus::Conflict);
    assert!(tail.rows().all(|r| r.text.capacity() == r.text.len()));
}

#[test]
fn all_small_prefixes_obey_the_two_closed_form_bounds() {
    // Independent longest-suffix oracle, rather than an eviction simulation.
    let texts = ["", "a", "é", "😀"];
    for encoded in 0..256usize {
        let input: Vec<_> = (0..4).map(|i| texts[(encoded >> (2 * i)) & 3]).collect();
        for rows in 1..=3 {
            for bytes in 1..=5 {
                let mut tail = TailBuffer::new(rows, bytes).unwrap();
                let mut accepted = Vec::new();
                for (i, text) in input.iter().enumerate() {
                    tail.push(row(i as u64, text));
                    if text.len() <= bytes {
                        accepted.push((i as u64, *text));
                    }
                    let first = (0..=accepted.len())
                        .find(|first| {
                            accepted.len() - first <= rows
                                && accepted[*first..]
                                    .iter()
                                    .map(|(_, t)| t.len())
                                    .sum::<usize>()
                                    <= bytes
                        })
                        .unwrap();
                    let expected = &accepted[first..];
                    assert_eq!(
                        tail.rows()
                            .map(|r| (r.id.batch_sequence, r.text.as_str()))
                            .collect::<Vec<_>>(),
                        expected
                    );
                    assert_eq!(
                        tail.utf8_bytes(),
                        expected.iter().map(|(_, t)| t.len()).sum::<usize>()
                    );
                    assert!(tail.rows().count() <= rows);
                }
            }
        }
    }
}

#[test]
fn polling_is_completion_based_and_honors_retry_after() {
    let mut poll = PollSchedule::new(15_000).unwrap();
    poll.begin(100).unwrap();
    assert_eq!(poll.begin(101), Err(ModelError::NotDue));
    poll.finish(
        200,
        PollOutcome::Retry {
            retry_after_ms: Some(100_000),
        },
    )
    .unwrap();
    assert_eq!(poll.next_due_ms(), Some(100_200));
    assert_eq!(poll.begin(100_199), Err(ModelError::NotDue));
    poll.begin(100_200).unwrap();
    poll.finish(
        100_300,
        PollOutcome::Retry {
            retry_after_ms: None,
        },
    )
    .unwrap();
    assert_eq!(poll.next_due_ms(), Some(115_300));
    poll.begin(115_300).unwrap();
    poll.finish(115_400, PollOutcome::Success).unwrap();
    assert_eq!(poll.next_due_ms(), Some(130_400));
    poll.begin(130_400).unwrap();
    poll.finish(
        130_500,
        PollOutcome::Retry {
            retry_after_ms: Some(0),
        },
    )
    .unwrap();
    assert_eq!(poll.next_due_ms(), Some(145_500));
}

#[test]
fn overload_cannot_accelerate_the_healthy_poll_rate() {
    // Review counterexample: a 15 s healthy interval previously retried a 503
    // after 1 s, increasing pressure precisely when the server refused work.
    for interval in [1, 999, 1_000, 15_000, 60_000] {
        let mut poll = PollSchedule::new(interval).unwrap();
        poll.begin(0).unwrap();
        poll.finish(
            0,
            PollOutcome::Retry {
                retry_after_ms: None,
            },
        )
        .unwrap();
        assert!(poll.next_due_ms().unwrap() >= interval);
    }
}

#[test]
fn polling_backoff_caps_and_terminal_errors_never_spin() {
    let mut poll = PollSchedule::new(10).unwrap();
    let mut now = 0;
    for delay in [1_000, 2_000, 4_000, 8_000, 16_000, 32_000, 60_000, 60_000] {
        poll.begin(now).unwrap();
        poll.finish(
            now,
            PollOutcome::Retry {
                retry_after_ms: None,
            },
        )
        .unwrap();
        now += delay;
        assert_eq!(poll.next_due_ms(), Some(now));
    }
    poll.begin(now).unwrap();
    poll.finish(now, PollOutcome::Stopped).unwrap();
    assert_eq!(poll.next_due_ms(), None);
    assert_eq!(poll.begin(now + 1), Err(ModelError::NotDue));
    let mut overflow = PollSchedule::new(10).unwrap();
    overflow.begin(u64::MAX).unwrap();
    assert_eq!(
        overflow.finish(u64::MAX, PollOutcome::Success),
        Err(ModelError::Exhausted)
    );
    assert_eq!(overflow.next_due_ms(), None);
    let mut rollback = PollSchedule::new(10).unwrap();
    rollback.begin(20).unwrap();
    assert_eq!(
        rollback.finish(19, PollOutcome::Success),
        Err(ModelError::ClockRollback)
    );
    assert_eq!(rollback.next_due_ms(), None);
    assert_eq!(
        rollback.finish(21, PollOutcome::Success),
        Err(ModelError::NotPending)
    );
}

#[test]
fn integer_normalization_keeps_nearby_large_times_distinct() {
    let start = 1_800_000_000_000_000_000;
    let samples = [
        Sample {
            time_ns: start + 1,
            value: Some(1.0),
        },
        Sample {
            time_ns: start + 2,
            value: Some(2.0),
        },
    ];
    let buckets = chart_envelope(
        &samples,
        TimeWindow {
            start_ns: start,
            end_ns: start + 10,
        },
        1,
    )
    .unwrap();
    assert_eq!(buckets[0].min.unwrap().x, 0.1);
    assert_eq!(buckets[0].max.unwrap().x, 0.2);
    // Negative control: early float subtraction loses both nanosecond offsets.
    assert_eq!((start + 1) as f64 - start as f64, 0.0);
    assert_eq!((start + 2) as f64 - start as f64, 0.0);
}

#[test]
fn closed_form_boundaries_extrema_and_gaps() {
    let samples = [
        Sample {
            time_ns: 0,
            value: Some(0.0),
        },
        Sample {
            time_ns: 1,
            value: Some(100.0),
        },
        Sample {
            time_ns: 2,
            value: None,
        },
        Sample {
            time_ns: 4,
            value: Some(-2.0),
        },
        Sample {
            time_ns: 8,
            value: None,
        },
    ];
    let b = chart_envelope(
        &samples,
        TimeWindow {
            start_ns: 0,
            end_ns: 8,
        },
        4,
    )
    .unwrap();
    assert_eq!(b.len(), 4);
    assert_eq!(b[0].min.unwrap().value, 0.0);
    assert_eq!(b[0].max.unwrap().value, 100.0);
    assert!(b[1].has_gap && b[1].min.is_none());
    assert_eq!(b[2].min.unwrap().x, 0.5);
    assert_eq!(b[2].max.unwrap().value, -2.0);
    assert!(b[3].has_gap && b[3].max.is_none());
    // Negative control: an average is not the retained spike envelope.
    assert_ne!((0.0 + 100.0) / 2.0, b[0].max.unwrap().value);
}

#[test]
fn mixed_gaps_ties_and_full_u64_range_are_stable() {
    let samples = [
        Sample {
            time_ns: 0,
            value: Some(3.0),
        },
        Sample {
            time_ns: 1,
            value: None,
        },
        Sample {
            time_ns: u64::MAX,
            value: Some(3.0),
        },
    ];
    let b = chart_envelope(
        &samples,
        TimeWindow {
            start_ns: 0,
            end_ns: u64::MAX,
        },
        1,
    )
    .unwrap();
    assert!(b[0].has_gap);
    assert_eq!(b[0].min.unwrap().time_ns, 0);
    assert_eq!(b[0].max.unwrap().time_ns, 0);
    let empty = chart_envelope(
        &[],
        TimeWindow {
            start_ns: 0,
            end_ns: 1,
        },
        2,
    )
    .unwrap();
    assert!(
        empty
            .iter()
            .all(|b| b.min.is_none() && b.max.is_none() && !b.has_gap)
    );
}

#[test]
fn malformed_or_over_budget_chart_is_rejected_whole() {
    let w = TimeWindow {
        start_ns: 10,
        end_ns: 20,
    };
    for samples in [
        vec![Sample {
            time_ns: 9,
            value: Some(1.0),
        }],
        vec![Sample {
            time_ns: 21,
            value: None,
        }],
        vec![
            Sample {
                time_ns: 12,
                value: None,
            },
            Sample {
                time_ns: 11,
                value: None,
            },
        ],
        vec![Sample {
            time_ns: 11,
            value: Some(f64::NAN),
        }],
        vec![Sample {
            time_ns: 11,
            value: Some(f64::INFINITY),
        }],
    ] {
        assert_eq!(
            chart_envelope(&samples, w, 2),
            Err(ModelError::InvalidSample)
        );
    }
    assert_eq!(
        chart_envelope(
            &[],
            TimeWindow {
                start_ns: 1,
                end_ns: 1
            },
            1
        ),
        Err(ModelError::InvalidWindow)
    );
    assert_eq!(chart_envelope(&[], w, 0), Err(ModelError::Budget));
    assert_eq!(
        chart_envelope(&[], w, MAX_CHART_BUCKETS + 1),
        Err(ModelError::Budget)
    );
    assert_eq!(
        chart_envelope(
            &vec![
                Sample {
                    time_ns: 11,
                    value: None
                };
                MAX_CHART_INPUT + 1
            ],
            w,
            1
        ),
        Err(ModelError::Budget)
    );
}

#[test]
fn value_permutations_preserve_exact_extrema_without_averaging() {
    for values in [
        [-4., 9., 2.],
        [-4., 2., 9.],
        [9., -4., 2.],
        [9., 2., -4.],
        [2., -4., 9.],
        [2., 9., -4.],
    ] {
        let samples: Vec<_> = values
            .iter()
            .enumerate()
            .map(|(i, value)| Sample {
                time_ns: i as u64,
                value: Some(*value),
            })
            .collect();
        let b = chart_envelope(
            &samples,
            TimeWindow {
                start_ns: 0,
                end_ns: 3,
            },
            1,
        )
        .unwrap();
        assert_eq!(b[0].min.unwrap().value, -4.);
        assert_eq!(b[0].max.unwrap().value, 9.);
        assert_eq!(
            b[0].min.unwrap().time_ns,
            values.iter().position(|v| *v == -4.).unwrap() as u64
        );
        assert_eq!(
            b[0].max.unwrap().time_ns,
            values.iter().position(|v| *v == 9.).unwrap() as u64
        );
    }
}
