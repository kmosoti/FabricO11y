//! Private many-Segment screen: real queries, fixed work, separate chain evidence.
use fabric_frame::envelope::Batch;
use fabric_server::{
    query::{History, Plan, Query},
    segment,
    store::{CommitMode, Entry, Group, Store},
};
use opentelemetry_proto::tonic::{
    collector::logs::v1::ExportLogsServiceRequest,
    common::v1::{AnyValue, any_value},
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
};
use prost::Message;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::{Arc, Barrier},
    time::Instant,
};
#[cfg(feature = "responsibility-alloc-probe")]
mod allocation {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    struct Counted;
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static TOTAL: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);
    fn add(n: usize) {
        let live = LIVE.fetch_add(n, Relaxed) + n;
        PEAK.fetch_max(live, Relaxed);
    }
    // These methods delegate allocation/layout validity to System. Counters never
    // allocate, and unsuccessful allocations/reallocations do not change the ledger.
    unsafe impl GlobalAlloc for Counted {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { System.alloc(layout) };
            if !p.is_null() {
                CALLS.fetch_add(1, Relaxed);
                TOTAL.fetch_add(layout.size(), Relaxed);
                add(layout.size());
            }
            p
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { System.alloc_zeroed(layout) };
            if !p.is_null() {
                CALLS.fetch_add(1, Relaxed);
                TOTAL.fetch_add(layout.size(), Relaxed);
                add(layout.size());
            }
            p
        }
        unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
            unsafe { System.dealloc(p, layout) };
            LIVE.fetch_sub(layout.size(), Relaxed);
        }
        unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            let q = unsafe { System.realloc(p, layout, size) };
            if !q.is_null() {
                CALLS.fetch_add(1, Relaxed);
                TOTAL.fetch_add(size, Relaxed);
                if size >= layout.size() {
                    add(size - layout.size());
                } else {
                    LIVE.fetch_sub(layout.size() - size, Relaxed);
                }
            }
            q
        }
    }

    pub fn reset() -> usize {
        let n = LIVE.load(Relaxed);
        PEAK.store(n, Relaxed);
        TOTAL.store(0, Relaxed);
        CALLS.store(0, Relaxed);
        n
    }
    pub fn facts() -> (usize, usize, usize, usize) {
        (
            LIVE.load(Relaxed),
            PEAK.load(Relaxed),
            TOTAL.load(Relaxed),
            CALLS.load(Relaxed),
        )
    }
    #[global_allocator]
    static ALLOCATOR: Counted = Counted;
}
// Linux's process CPU clock includes all threads in this probe process. The
// platform ABI uses C long seconds/nanoseconds; this example does not pretend
// that /proc's jiffy counters resolve small operations. Each pointer is valid
// for the duration of the call. No FFI is introduced into production crates.
#[cfg(target_os = "linux")]
mod process_clock {
    use std::ffi::{c_int, c_long};
    #[repr(C)]
    struct Timespec {
        seconds: c_long,
        nanoseconds: c_long,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: c_int, out: *mut Timespec) -> c_int;
        fn clock_getres(clock: c_int, out: *mut Timespec) -> c_int;
    }
    const PROCESS_CPU: c_int = 2;
    fn read(resolution: bool) -> u64 {
        let mut ts = Timespec {
            seconds: 0,
            nanoseconds: 0,
        };
        // Linux CLOCK_PROCESS_CPUTIME_ID and writable initialized Timespec.
        let rc = unsafe {
            if resolution {
                clock_getres(PROCESS_CPU, &mut ts)
            } else {
                clock_gettime(PROCESS_CPU, &mut ts)
            }
        };
        assert_eq!(rc, 0, "Linux process CPU clock unavailable");
        assert!(ts.seconds >= 0 && (0..1_000_000_000).contains(&ts.nanoseconds));
        ts.seconds as u64 * 1_000_000_000 + ts.nanoseconds as u64
    }
    pub fn now() -> u64 {
        read(false)
    }
    pub fn resolution() -> u64 {
        read(true)
    }
}
#[cfg(not(target_os = "linux"))]
compile_error!("responsibility_probe requires Linux process CPU clock semantics");
fn hash(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}
const START: u64 = 1_600_000_000_000_000_000;
fn bodies(size: usize, count: usize) -> Vec<String> {
    assert!((16..=3500).contains(&size), "body size must be 16..=3500");
    (0..count)
        .map(|j| {
            let mut b = format!("bench-{j:04} ").into_bytes();
            let mut x = 42u64 ^ (j as u64 + 1);
            while b.len() < size {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                b.push(if j % 2 == 0 {
                    b'R'
                } else {
                    b'!' + (x % 94) as u8
                });
            }
            String::from_utf8(b).unwrap()
        })
        .collect()
}
// Timestamp ranks are a permutation, independently reproducible from seed 42.
// Source order and per-Batch sequence/index remain unchanged, so the sort's
// output assertion must follow the rank permutation rather than source order.
fn timestamp_ranks(count: usize, order: &str) -> Vec<usize> {
    let mut ranks: Vec<_> = (0..count).collect();
    match order {
        "sorted" => {}
        "reversed" => ranks.reverse(),
        "shuffled" => {
            let mut state = 42u64;
            for upper in (1..count).rev() {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ranks.swap(upper, state as usize % (upper + 1));
            }
        }
        _ => panic!("BENCH_ORDER must be sorted, reversed or shuffled"),
    }
    let mut check = ranks.clone();
    check.sort_unstable();
    assert_eq!(check, (0..count).collect::<Vec<_>>());
    ranks
}
fn fixture(text: &[String], ranks: &[usize]) -> Vec<Group> {
    text.chunks(128)
        .enumerate()
        .map(|(i, lines)| {
            let req = ExportLogsServiceRequest {
                resource_logs: vec![ResourceLogs {
                    scope_logs: vec![ScopeLogs {
                        log_records: lines
                            .iter()
                            .enumerate()
                            .map(|(j, s)| LogRecord {
                                observed_time_unix_nano: START + ranks[i * 128 + j] as u64,
                                body: Some(AnyValue {
                                    value: Some(any_value::Value::StringValue(s.clone())),
                                }),
                                ..Default::default()
                            })
                            .collect(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
            };
            let b = Batch {
                version: 1,
                node_id: vec![7; 16],
                generation: 1,
                sequence: i as u64 + 1,
                logs: req.encode_to_vec(),
                ..Default::default()
            };
            Group {
                group_sequence: i as u64 + 1,
                entries: vec![Entry {
                    label: "fixture".into(),
                    batch: b.encode_to_vec(),
                    received_unix_nano: START + i as u64 + 1,
                }],
            }
        })
        .collect()
}

fn io() -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for line in fs::read_to_string("/proc/self/io").unwrap().lines() {
        let (k, v) = line.split_once(':').unwrap();
        out.insert(k.into(), json!(v.trim().parse::<u64>().unwrap()));
    }
    out.into()
}
fn main() {
    let a: Vec<_> = std::env::args().collect();
    assert_eq!(a.len(), 5, "ROOT SEGMENTS READERS clone|shared");
    let root = PathBuf::from(&a[1]);
    let segments: usize = a[2].parse().unwrap();
    let readers: usize = a[3].parse().unwrap();
    assert!([1, 64].contains(&segments) && [1, 4].contains(&readers));
    assert!(["clone", "shared"].contains(&a[4].as_str()));
    assert!(
        !History::borrowed_logs_enabled(),
        "isolate catalog from borrowed-log candidate"
    );
    let scratch = PathBuf::from(std::env::var("FABRIC_SCRATCH_ROOT").unwrap());
    assert!(root.starts_with(scratch));
    fs::create_dir(&root).unwrap();
    let state = root.join("state");
    drop(Store::open(&state, 1024 * 1024 * 1024, CommitMode::INDIVIDUAL).unwrap());
    let text = bodies(1024, 65536);
    let groups = fixture(&text, &timestamp_ranks(65536, "shuffled"));
    let mut ledger = BufWriter::new(fs::File::create(root.join("records.jsonl")).unwrap());
    for group in &groups {
        for e in &group.entries {
            writeln!(
                ledger,
                "{}",
                json!({"label":e.label,"received_ns":e.received_unix_nano,
            "hex":hex(&e.batch),"sha256":hash(&e.batch)})
            )
            .unwrap();
        }
    }
    ledger.flush().unwrap();
    drop(ledger);
    for (index, chunk) in groups.chunks(groups.len() / segments).enumerate() {
        segment::build(&state, index as u64 + 1, chunk).unwrap();
    }
    drop(groups);
    drop(text);
    let mut history = History::with_plan(&state, Plan::Walk);
    if a[4] == "shared" {
        history = history.with_shared_catalog();
    }
    let history = Arc::new(history);
    let mut results = Vec::new();
    for shape in ["absent", "broad"] {
        let mut q = json!({"kind":"logs","from_ns":START,"to_ns":START+65536,"limit":1000});
        if shape == "absent" {
            q["contains"] = json!("__CR3_ABSENT__");
        }
        let parsed: Query = serde_json::from_value(q.clone()).unwrap();
        // One warm-up establishes identical derived-cache state; excluded from timing.
        let first = serde_json::to_vec(&history.run(&parsed, 512).unwrap()).unwrap();
        let ready = Arc::new(Barrier::new(readers + 1));
        let start = Arc::new(Barrier::new(readers + 1));
        let handles: Vec<_> = (0..readers)
            .map(|_| {
                let history = history.clone();
                let parsed = parsed.clone();
                let ready = ready.clone();
                let start = start.clone();
                std::thread::spawn(move || {
                    ready.wait();
                    start.wait();
                    (0..32 / readers)
                        .map(|_| {
                            let begin = Instant::now();
                            let answer =
                                serde_json::to_vec(&history.run(&parsed, 512).unwrap()).unwrap();
                            (answer, begin.elapsed().as_nanos() as u64)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        ready.wait();
        let before_io = io();
        #[cfg(feature = "responsibility-alloc-probe")]
        let base = allocation::reset();
        let cpu_start = process_clock::now();
        let wall = Instant::now();
        start.wait();
        let observed: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let wall_ns = wall.elapsed().as_nanos() as u64;
        let cpu_ns = process_clock::now() - cpu_start;
        #[cfg(feature = "responsibility-alloc-probe")]
        let alloc = {
            let (live, peak, total, calls) = allocation::facts();
            Some(json!({"base":base,"live":live,"peak":peak,"total":total,"calls":calls}))
        };
        #[cfg(not(feature = "responsibility-alloc-probe"))]
        let alloc: Option<serde_json::Value> = None;
        let after_io = io();
        let mut calls =
            BufWriter::new(fs::File::create(root.join(format!("calls-{shape}.jsonl"))).unwrap());
        let mut count = 0;
        let mut latencies = Vec::new();
        for (answer, latency_ns) in observed.into_iter().flatten() {
            assert_eq!(
                answer, first,
                "measured page differs from canonical first page"
            );
            calls.write_all(&answer).unwrap();
            calls.write_all(b"\n").unwrap();
            count += 1;
            latencies.push(latency_ns);
        }
        calls.flush().unwrap();
        assert_eq!(count, 32);
        // Full continuation chain is independent evidence outside the timed loops.
        let mut chain =
            BufWriter::new(fs::File::create(root.join(format!("chain-{shape}.jsonl"))).unwrap());
        chain.write_all(&first).unwrap();
        chain.write_all(b"\n").unwrap();
        let mut page: serde_json::Value = serde_json::from_slice(&first).unwrap();
        let mut pages = 1;
        while !page["next_page"].is_null() {
            assert!(pages < 68, "page-count guard");
            let mut next = q.clone();
            next["page"] = page["next_page"].clone();
            page = history
                .run(&serde_json::from_value(next).unwrap(), 512)
                .unwrap();
            assert_eq!(page["complete"], true);
            writeln!(chain, "{}", page).unwrap();
            pages += 1;
        }
        chain.flush().unwrap();
        results.push(json!({"shape":shape,"query":q,"calls":count,"pages":pages,
            "wall_ns":wall_ns,"cpu_ns":cpu_ns,"latency_ns":latencies,"allocation":alloc,"io_before":before_io,"io_after":after_io}));
    }
    println!(
        "{}",
        json!({"fixture":{"rows":65536,"body_bytes":1024,"seed":42,"segments":segments,
        "readers":readers,"variant":a[4],"borrowed_logs":false,"plan":"walk","limit":1000},
        "cpu_resolution_ns":process_clock::resolution(),"results":results})
    );
}
