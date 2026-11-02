use fabric::{
    admission::{Admission, Queue, QueueError},
    collection::Observation,
};
use serde::Deserialize;
use std::{
    hint::black_box,
    sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Deserialize)]
struct Case {
    name: String,
    text: String,
    vector_len: usize,
    slots: usize,
    producers: usize,
    consumers: usize,
}
fn setting(name: &str, fallback: usize) -> usize {
    let n = std::env::var(name)
        .map(|s| s.parse().expect("invalid benchmark setting"))
        .unwrap_or(fallback);
    assert!(n > 0);
    n
}
fn percentile(values: &mut [u128], percent: usize) -> u128 {
    values.sort_unstable();
    values[(values.len() - 1) * percent / 100]
}
fn main() {
    let family = std::env::var("FABRIC_QUEUE_FAMILY").unwrap_or_else(|_| "sdk".into());
    let records = setting("FABRIC_BENCH_RECORDS", 2000);
    let repeats = setting("FABRIC_BENCH_REPEATS", 5);
    let seconds = setting("FABRIC_BENCH_SECONDS", 30);
    let case_file = std::env::var("FABRIC_BENCH_CASES_FILE").ok();
    let cases: Vec<Case> = if let Some(path) = &case_file {
        serde_json::from_slice(&std::fs::read(path).expect("read benchmark cases"))
            .expect("parse benchmark cases")
    } else {
        let mut cases = Vec::new();
        for text_bytes in [64, 512] {
            for slots in [8, 64] {
                for producers in [1, 4] {
                    cases.push(Case {
                        name: format!("ascii-{text_bytes}-slots-{slots}-workers-{producers}"),
                        text: "x".repeat(text_bytes),
                        vector_len: 8,
                        slots,
                        producers,
                        consumers: producers,
                    });
                }
            }
        }
        cases
    };
    assert!(!cases.is_empty());
    println!(
        "{}",
        serde_json::json!({"event":"configuration", "expected_samples":cases.len().checked_mul(repeats).unwrap(),
        "records_per_producer":records,"repeats":repeats,"deadline_seconds":seconds,"cases_file":case_file,"family":family})
    );
    for case in cases {
        let Case {
            name,
            text,
            vector_len,
            slots,
            producers,
            consumers,
        } = case;
        assert!(slots > 0 && producers > 0 && consumers > 0 && vector_len <= 256);
        let item = Observation {
            version: 1,
            id: "observation-000".into(),
            producer: "service".into(),
            timestamp: 42,
            kind: "request".into(),
            text,
            value: 1.5,
            vector: vec![0.5; vector_len],
            related: vec![],
        };
        let encoded = serde_json::to_vec(&item).unwrap().len();
        assert!(encoded <= 2048);
        let mut first_construction_ns = 0;
        for repeat in 0..=repeats {
            let setup = Instant::now();
            let q = Queue::new(slots.checked_mul(2048).unwrap(), 2048).unwrap();
            let setup_ns = setup.elapsed().as_nanos();
            let storage = q.storage();
            let total = records
                .checked_mul(producers)
                .expect("record count overflow");
            let consumed = Arc::new(AtomicUsize::new(0));
            let barrier = Arc::new(Barrier::new(
                producers
                    .checked_add(consumers)
                    .and_then(|n| n.checked_add(1))
                    .unwrap(),
            ));
            let (elapsed_ns, mut latencies, mut end_to_end, attempts, full, contention, polls) =
                std::thread::scope(|scope| {
                    let mut writers = Vec::new();
                    let mut readers = Vec::new();
                    for _ in 0..producers {
                        let (q, barrier, item) = (q.clone(), Arc::clone(&barrier), &item);
                        writers.push(scope.spawn(move || {
                            let mut latencies = Vec::with_capacity(records);
                            let mut end_to_end = Vec::with_capacity(records);
                            let (mut attempts, mut full, mut contention) = (0u64, 0u64, 0u64);
                            barrier.wait();
                            let deadline = Instant::now() + Duration::from_secs(seconds as u64);
                            for _ in 0..records {
                                let begin = Instant::now();
                                loop {
                                    if attempts.is_multiple_of(1024) {
                                        assert!(
                                            Instant::now() < deadline,
                                            "producer deadline exceeded"
                                        );
                                    }
                                    attempts += 1;
                                    let call = Instant::now();
                                    let result = q.try_record(black_box(item));
                                    let latency = call.elapsed().as_nanos();
                                    match result {
                                        Admission::Accepted => {
                                            latencies.push(latency);
                                            end_to_end.push(begin.elapsed().as_nanos());
                                            break;
                                        }
                                        Admission::Full => full += 1,
                                        Admission::Contended => contention += 1,
                                        other => panic!("benchmark admission failed: {other:?}"),
                                    }
                                    std::thread::yield_now();
                                }
                            }
                            (latencies, end_to_end, attempts, full, contention)
                        }));
                    }
                    for _ in 0..consumers {
                        let (q, barrier, consumed) =
                            (q.clone(), Arc::clone(&barrier), Arc::clone(&consumed));
                        readers.push(scope.spawn(move || {
                            let mut polls = 0u64;
                            barrier.wait();
                            let deadline = Instant::now() + Duration::from_secs(seconds as u64);
                            while consumed.load(Ordering::Acquire) < total {
                                if polls.is_multiple_of(1024) {
                                    assert!(
                                        Instant::now() < deadline,
                                        "consumer deadline exceeded"
                                    );
                                }
                                polls += 1;
                                match q.try_dequeue() {
                                    Ok(Some(lease)) => {
                                        black_box(&*lease);
                                        drop(lease);
                                        consumed.fetch_add(1, Ordering::Release);
                                    }
                                    Ok(None) | Err(QueueError::Contended) => {
                                        std::thread::yield_now()
                                    }
                                    Err(other) => panic!("benchmark dequeue failed: {other:?}"),
                                }
                            }
                            polls
                        }));
                    }
                    let start = Instant::now();
                    barrier.wait();
                    let results: Vec<_> = writers.into_iter().map(|w| w.join().unwrap()).collect();
                    let polls: u64 = readers.into_iter().map(|r| r.join().unwrap()).sum();
                    let elapsed_ns = start.elapsed().as_nanos();
                    let (mut latencies, mut end_to_end) = (Vec::new(), Vec::new());
                    let (mut attempts, mut full, mut contention) = (0, 0, 0);
                    for (l, e, a, f, c) in results {
                        latencies.extend(l);
                        end_to_end.extend(e);
                        attempts += a;
                        full += f;
                        contention += c;
                    }
                    (
                        elapsed_ns, latencies, end_to_end, attempts, full, contention, polls,
                    )
                });
            assert_eq!(consumed.load(Ordering::Acquire), total);
            assert!(q.try_dequeue().unwrap().is_none());
            if repeat == 0 {
                first_construction_ns = setup_ns;
                continue;
            }
            println!(
                "{}",
                serde_json::json!({
                    "event":"measurement", "case":name, "family":family, "repeat":repeat,"text_bytes":item.text.len(),"vector_len":vector_len,"encoded_bytes":encoded,
                    "slots":slots,"max_record":2048,"producers":producers,"consumers":consumers,
                    "records_per_producer":records,"records":total,"setup_ns":setup_ns,"first_process_local_construction_ns":first_construction_ns,
                    "elapsed_ns":elapsed_ns,"records_per_second":total as f64*1e9/elapsed_ns as f64,
                    "accepted_call_p50_ns":percentile(&mut latencies,50),"accepted_call_p99_ns":percentile(&mut latencies,99),
                    "record_with_retries_p50_ns":percentile(&mut end_to_end,50),"record_with_retries_p99_ns":percentile(&mut end_to_end,99),
                    "attempts":attempts,"full":full,"contention":contention,"consumer_polls":polls,
                    "payload_bytes":storage.payload_bytes,"collection_bytes":storage.collection_bytes,"control_bytes":storage.control_bytes,"control_blocks":storage.control_blocks,
                    "warmup_samples":1,"deadline_seconds_per_sample":seconds,
                    "boundary":"after thread creation through final lease release; construction separately reported; caller fixture creation and result sorting excluded"
                })
            );
        }
    }
}
