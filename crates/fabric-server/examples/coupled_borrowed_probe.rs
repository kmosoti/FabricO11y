//! C4 attribute-associated query attribution; no production semantic changes.
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
    #[cfg(feature = "phase-probe")]
    pub fn thread_now() -> u64 {
        let mut ts = Timespec {
            seconds: 0,
            nanoseconds: 0,
        };
        // Linux CLOCK_THREAD_CPUTIME_ID, initialized writable Timespec.
        assert_eq!(unsafe { clock_gettime(3, &mut ts) }, 0);
        ts.seconds as u64 * 1_000_000_000 + ts.nanoseconds as u64
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
            let mut b = format!(
                "{}-{j:04} ",
                if j % 128 == 7 {
                    "needle-C4"
                } else {
                    "other-C4"
                }
            )
            .into_bytes();
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
fn fixture(text: &[String], ranks: &[usize], attrs: usize) -> Vec<Group> {
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
                                attributes: (0..attrs)
                                    .map(|k| {
                                        let mut x = 42u64 ^ (k as u64 + 1); // Same per-key value on every row: explicit compressible control.
                                        let value = (0..128)
                                            .map(|_| {
                                                x ^= x << 13;
                                                x ^= x >> 7;
                                                x ^= x << 17;
                                                (b'A' + (x % 26) as u8) as char
                                            })
                                            .collect::<String>();
                                        opentelemetry_proto::tonic::common::v1::KeyValue {
                                            key: format!("attr-{k}"),
                                            value: Some(AnyValue {
                                                value: Some(any_value::Value::StringValue(value)),
                                            }),
                                            ..Default::default()
                                        }
                                    })
                                    .collect(),
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

fn parquet_bytes(root: &std::path::Path) -> u64 {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            if path.is_dir() {
                parquet_bytes(&path)
            } else if path.extension().is_some_and(|e| e == "parquet") {
                fs::metadata(path).unwrap().len()
            } else {
                0
            }
        })
        .sum()
}
#[cfg(feature = "phase-probe")]
fn flush_phases(path: &std::path::Path) {
    let rows = fabric_frame::probe::take();
    assert!(rows.len() <= 8192, "C4 phase ledger bound");
    let mut out = BufWriter::new(fs::File::create(path).unwrap());
    for row in rows {
        writeln!(out,"{}",json!({"phase":row.name,"depth":row.depth,"start_ns":row.start_ns,"wall_ns":row.wall_ns,"thread_cpu_ns":row.after[0].saturating_sub(row.before[0]),"before":row.before,"after":row.after})).unwrap();
    }
    out.flush().unwrap();
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 3, "ROOT ATTRS");
    let root = PathBuf::from(&args[1]);
    let attrs: usize = args[2].parse().unwrap();
    assert!([0, 8].contains(&attrs));
    let phase_on = std::env::var("BENCH_PHASES").is_ok_and(|s| s == "1");
    #[cfg(feature = "phase-probe")]
    if phase_on {
        fabric_frame::probe::install(|| {
            #[cfg(feature = "responsibility-alloc-probe")]
            let (live, peak, total, _) = allocation::facts();
            #[cfg(not(feature = "responsibility-alloc-probe"))]
            let (live, peak, total) = (0, 0, 0);
            [
                process_clock::thread_now(),
                live as u64,
                peak as u64,
                total as u64,
            ]
        });
    }
    #[cfg(not(feature = "phase-probe"))]
    assert!(!phase_on, "observer unavailable in plain build");
    assert!(root.starts_with(PathBuf::from(std::env::var("FABRIC_SCRATCH_ROOT").unwrap())));
    fs::create_dir(&root).unwrap();
    let state = root.join("state");
    drop(Store::open(&state, 1024 * 1024 * 1024, CommitMode::INDIVIDUAL).unwrap());
    let text = bodies(1024, 4096);
    let groups = fixture(&text, &timestamp_ranks(4096, "shuffled"), attrs);
    let encoded: usize = groups
        .iter()
        .flat_map(|g| &g.entries)
        .map(|e| e.batch.len())
        .sum();
    let mut ledger = BufWriter::new(fs::File::create(root.join("records.jsonl")).unwrap());
    for g in &groups {
        for e in &g.entries {
            writeln!(ledger,"{}",json!({"label":e.label,"received_ns":e.received_unix_nano,"hex":hex(&e.batch),"sha256":hash(&e.batch)})).unwrap();
        }
    }
    ledger.flush().unwrap();
    drop(ledger);
    segment::build(&state, 1, &groups).unwrap();
    drop(groups);
    drop(text);
    #[cfg(feature = "phase-probe")]
    if phase_on {
        flush_phases(&root.join("fixture-phases.jsonl"));
    }
    let mut results = Vec::new();
    for (name, plan) in [("walk", Plan::Walk), ("scan", Plan::Scan)] {
        let history = History::with_plan(&state, plan);
        for shape in ["selective", "broad"] {
            let mut q = json!({"kind":"logs","from_ns":START,"to_ns":START+4096,"limit":50});
            if shape == "selective" {
                q["contains"] = json!("needle-C4");
            }
            let parsed: Query = serde_json::from_value(q.clone()).unwrap();
            let first = serde_json::to_vec(&history.run(&parsed, 32).unwrap()).unwrap();
            #[cfg(feature = "phase-probe")]
            if phase_on {
                flush_phases(&root.join(format!("warmup-{name}-{shape}.jsonl")));
            }
            let mut calls = BufWriter::new(
                fs::File::create(root.join(format!("calls-{name}-{shape}.jsonl"))).unwrap(),
            );
            for iteration in 0..3 {
                let before_io = io();
                #[cfg(feature = "responsibility-alloc-probe")]
                let base = allocation::reset();
                let cpu = process_clock::now();
                let wall = Instant::now();
                let answer = history.run(&parsed, 32).unwrap();
                let wall_ns = wall.elapsed().as_nanos() as u64;
                let cpu_ns = process_clock::now() - cpu;
                #[cfg(feature = "responsibility-alloc-probe")]
                let alloc = {
                    let (live, peak, total, count) = allocation::facts();
                    Some(json!({"base":base,"live":live,"peak":peak,"total":total,"calls":count}))
                };
                #[cfg(not(feature = "responsibility-alloc-probe"))]
                let alloc: Option<serde_json::Value> = None;
                let after_io = io();
                #[cfg(feature = "phase-probe")]
                if phase_on {
                    flush_phases(&root.join(format!("measured-{name}-{shape}-{iteration}.jsonl")));
                }
                let bytes = serde_json::to_vec(&answer).unwrap();
                assert_eq!(bytes, first);
                calls.write_all(&bytes).unwrap();
                calls.write_all(b"\n").unwrap();
                results.push(json!({"plan":name,"shape":shape,"iteration":iteration,"query":q,"wall_ns":wall_ns,"cpu_ns":cpu_ns,"allocation":alloc,"io_before":before_io,"io_after":after_io}));
            }
            calls.flush().unwrap();
            drop(calls);
            let mut chain = BufWriter::new(
                fs::File::create(root.join(format!("chain-{name}-{shape}.jsonl"))).unwrap(),
            );
            chain.write_all(&first).unwrap();
            chain.write_all(b"\n").unwrap();
            let mut page: serde_json::Value = serde_json::from_slice(&first).unwrap();
            let mut pages = 1;
            while !page["next_page"].is_null() {
                assert!(pages < 84);
                let mut next = q.clone();
                next["page"] = page["next_page"].clone();
                page = history
                    .run(&serde_json::from_value(next).unwrap(), 32)
                    .unwrap();
                assert_eq!(page["complete"], true);
                writeln!(chain, "{}", page).unwrap();
                pages += 1;
            }
            chain.flush().unwrap();
            #[cfg(feature = "phase-probe")]
            if phase_on {
                flush_phases(&root.join(format!("continuation-{name}-{shape}.jsonl")));
            }
        }
    }
    println!(
        "{}",
        json!({"fixture":{"rows":4096,"body_bytes":1024,"attrs":attrs,"attr_value_bytes":128,"cpu_clock_resolution_ns":process_clock::resolution(),"seed":42,"encoded_batch_bytes":encoded,"parquet_bytes":parquet_bytes(&state),"borrowed":History::borrowed_logs_enabled(),"phase_on":phase_on,"counted":cfg!(feature="responsibility-alloc-probe"),"limit":50},"results":results})
    );
}
