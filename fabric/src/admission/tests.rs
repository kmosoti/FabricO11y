use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, atomic::AtomicU64},
    time::{Duration, Instant},
};
pub(super) fn sample() -> Observation {
    Observation {
        version: 1,
        id: "a".into(),
        producer: "b".into(),
        timestamp: 0,
        kind: "c".into(),
        text: "x".into(),
        value: 0.0,
        vector: vec![],
        related: vec![],
    }
}
#[test]
fn construction_reservations_leases_and_shutdown() {
    for (bytes, record) in [(0, 0), (0, 1), (1, 0), (1, 2), (usize::MAX, 1)] {
        assert!(Queue::new(bytes, record).is_err());
    }
    let item = sample();
    let expected = serde_json::to_vec(&item).unwrap();
    let q = Queue::new(2048, 1024).unwrap();
    let allocation = q.storage();
    assert_eq!(allocation.slots, 2);
    assert!(allocation.payload_bytes <= 2048);
    assert_eq!(q.try_record(&item), Admission::Accepted);
    assert_eq!(q.try_record(&item), Admission::Accepted);
    assert_eq!(q.try_record(&item), Admission::Full);
    let first = q.try_dequeue().unwrap().unwrap();
    assert_eq!(first.sequence(), 0);
    assert_eq!(&*first, expected);
    assert_eq!(q.try_record(&item), Admission::Full);
    drop(first);
    assert_eq!(q.try_record(&item), Admission::Accepted);
    assert_eq!(q.rejected().unwrap(), 2);
    q.shutdown().unwrap();
    assert_eq!(q.try_record(&item), Admission::Closed);
    let second = q.try_dequeue().unwrap().unwrap();
    let third = q.try_dequeue().unwrap().unwrap();
    assert_eq!((second.sequence(), third.sequence()), (1, 2));
    assert!(q.try_dequeue().unwrap().is_none());
    assert_eq!(q.storage(), allocation);
    drop(q);
    assert_eq!(&*second, expected);
    assert_eq!(&*third, expected);
    drop((second, third));
}
#[test]
fn encoding_failure_restores_credit_and_matches_reference_bytes() {
    for text in [
        "x".repeat(128),
        "\"\\\n\u{0}".repeat(128),
        "é猫".repeat(128),
    ] {
        let mut item = sample();
        item.text = text;
        let reference = serde_json::to_vec(&item).unwrap();
        for limit in [reference.len() - 1, reference.len(), reference.len() + 1] {
            let q = Queue::new(limit * 2, limit).unwrap();
            if limit < reference.len() {
                assert_eq!(q.try_record(&item), Admission::TooLarge);
                assert!(q.try_dequeue().unwrap().is_none());
            } else {
                assert_eq!(q.try_record(&item), Admission::Accepted);
                let lease = q.try_dequeue().unwrap().unwrap();
                assert_eq!(&*lease, reference);
                drop(lease);
            }
            assert_eq!(q.try_record(&sample()), Admission::Accepted);
            assert_eq!(q.try_record(&sample()), Admission::Accepted);
            assert_eq!(q.try_record(&sample()), Admission::Full);
        }
    }
    let q = Queue::new(2048, 1024).unwrap();
    let mut invalid = sample();
    invalid.value = f64::NAN;
    assert_eq!(q.try_record(&invalid), Admission::Invalid);
    assert_eq!(q.try_record(&sample()), Admission::Accepted);
    assert_eq!(q.try_dequeue().unwrap().unwrap().sequence(), 0);
}

#[derive(Clone, Copy)]
struct Event {
    id: usize,
    start: u64,
    end: u64,
    ticket: u64,
}
fn fifo_witness_exists(enqueues: &[Event], dequeues: &[Event]) -> bool {
    let n = enqueues.len();
    if dequeues.len() != n {
        return false;
    }
    let offers: BTreeMap<_, _> = enqueues.iter().map(|e| (e.id, *e)).collect();
    if offers.len() != n {
        return false;
    }
    let mut takes = dequeues.to_vec();
    takes.sort_by_key(|e| e.ticket);
    if !takes.iter().map(|e| e.ticket).eq(0..n as u64) {
        return false;
    }
    let mut events = Vec::new();
    for take in &takes {
        let Some(offer) = offers.get(&take.id) else {
            return false;
        };
        events.push(*offer);
    }
    if takes.iter().map(|e| e.id).collect::<BTreeSet<_>>().len() != n {
        return false;
    }
    events.extend(takes);
    if events.iter().any(|e| e.start >= e.end) {
        return false;
    }
    let mut edges = vec![Vec::new(); 2 * n];
    let mut indegree = vec![0usize; 2 * n];
    let mut edge = |a: usize, b: usize| {
        edges[a].push(b);
        indegree[b] += 1;
    };
    for i in 0..n {
        edge(i, n + i); // A record must be enqueued before its dequeue.
        if i + 1 < n {
            edge(i, i + 1);
            edge(n + i, n + i + 1);
        }
    }
    for (i, a) in events.iter().enumerate() {
        for (j, b) in events.iter().enumerate() {
            if a.end < b.start {
                edge(i, j);
            }
        }
    }
    let mut ready: std::collections::VecDeque<_> = indegree
        .iter()
        .enumerate()
        .filter_map(|(i, d)| (*d == 0).then_some(i))
        .collect();
    let mut visited = 0;
    while let Some(i) = ready.pop_front() {
        visited += 1;
        for &j in &edges[i] {
            indegree[j] -= 1;
            if indegree[j] == 0 {
                ready.push_back(j);
            }
        }
    }
    visited == 2 * n
}

#[test]
fn history_checker_rejects_cross_operation_cycles_but_allows_overlapping_returns() {
    let takes = [
        Event {
            id: 0,
            start: 1,
            end: 7,
            ticket: 0,
        },
        Event {
            id: 1,
            start: 2,
            end: 4,
            ticket: 1,
        },
    ];
    let impossible = [
        Event {
            id: 0,
            start: 5,
            end: 8,
            ticket: 0,
        },
        Event {
            id: 1,
            start: 0,
            end: 9,
            ticket: 0,
        },
    ];
    assert!(!fifo_witness_exists(&impossible, &takes));
    let valid = [
        Event {
            id: 0,
            start: 0,
            end: 8,
            ticket: 0,
        },
        Event {
            id: 1,
            start: 1,
            end: 9,
            ticket: 0,
        },
    ];
    assert!(fifo_witness_exists(&valid, &takes));
}
#[test]
fn mpmc_operation_histories_preserve_fifo_and_exact_delivery() {
    const PRODUCERS: usize = 4;
    const CONSUMERS: usize = 4;
    const EACH: usize = 200;
    let q = Queue::new(32 * 1024, 1024).unwrap();
    let clock = Arc::new(AtomicU64::new(0));
    let received = Arc::new(AtomicUsize::new(0));
    let enqueues = Arc::new(Mutex::new(Vec::new()));
    let dequeues = Arc::new(Mutex::new(Vec::new()));
    std::thread::scope(|scope| {
        for producer in 0..PRODUCERS {
            let (q, clock, enqueues) = (q.clone(), Arc::clone(&clock), Arc::clone(&enqueues));
            scope.spawn(move || {
                for i in 0..EACH {
                    let id = producer * EACH + i;
                    let mut item = sample();
                    item.id = id.to_string();
                    let deadline = Instant::now() + Duration::from_secs(20);
                    loop {
                        assert!(Instant::now() < deadline, "producer made no progress");
                        let start = clock.fetch_add(1, Ordering::SeqCst);
                        let result = q.try_record(&item);
                        let end = clock.fetch_add(1, Ordering::SeqCst);
                        match result {
                            Admission::Accepted => {
                                enqueues.lock().unwrap().push(Event {
                                    id,
                                    start,
                                    end,
                                    ticket: 0,
                                });
                                break;
                            }
                            Admission::Full | Admission::Contended => std::thread::yield_now(),
                            other => panic!("unexpected admission: {other:?}"),
                        }
                    }
                }
            });
        }
        for _ in 0..CONSUMERS {
            let (q, clock, dequeues, received) = (
                q.clone(),
                Arc::clone(&clock),
                Arc::clone(&dequeues),
                Arc::clone(&received),
            );
            scope.spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(20);
                while received.load(Ordering::Acquire) < PRODUCERS * EACH {
                    assert!(Instant::now() < deadline, "consumer made no progress");
                    let start = clock.fetch_add(1, Ordering::SeqCst);
                    let result = q.try_dequeue();
                    let end = clock.fetch_add(1, Ordering::SeqCst);
                    match result {
                        Ok(Some(lease)) => {
                            let item: Observation = serde_json::from_slice(&lease).unwrap();
                            dequeues.lock().unwrap().push(Event {
                                id: item.id.parse().unwrap(),
                                start,
                                end,
                                ticket: lease.sequence(),
                            });
                            if lease.sequence() % 7 == 0 {
                                std::thread::yield_now();
                            }
                            drop(lease);
                            received.fetch_add(1, Ordering::Release);
                        }
                        Ok(None) | Err(QueueError::Contended) => std::thread::yield_now(),
                        Err(other) => panic!("unexpected dequeue: {other:?}"),
                    }
                }
            });
        }
    });
    let enqueues = enqueues.lock().unwrap();
    let dequeues = dequeues.lock().unwrap();
    assert_eq!(enqueues.len(), PRODUCERS * EACH);
    assert_eq!(dequeues.len(), PRODUCERS * EACH);
    let ids: BTreeSet<_> = dequeues.iter().map(|e| e.id).collect();
    assert_eq!(ids.len(), PRODUCERS * EACH);
    let by_id: BTreeMap<_, _> = dequeues.iter().map(|e| (e.id, e.ticket)).collect();
    let tickets: BTreeSet<_> = by_id.values().copied().collect();
    assert!(tickets.into_iter().eq(0..(PRODUCERS * EACH) as u64));
    assert!(
        fifo_witness_exists(&enqueues, &dequeues),
        "accepted history has no FIFO linearization consistent with call intervals"
    );
    assert!(q.try_dequeue().unwrap().is_none());
}
use std::sync::atomic::AtomicUsize;

#[test]
fn caller_unwind_releases_lease_without_poisoning_unmodified_queue_state() {
    let q = Queue::new(1024, 1024).unwrap();
    assert_eq!(q.try_record(&sample()), Admission::Accepted);
    let lease = q.try_dequeue().unwrap().unwrap();
    assert!(
        std::thread::spawn(move || {
            let _held = lease;
            panic!("caller panic outside synchronization");
        })
        .join()
        .is_err()
    );
    assert_eq!(q.try_record(&sample()), Admission::Accepted);
    drop(q.try_dequeue().unwrap().unwrap());
}
