//! Key-first query admission screen; exact ledgers and complete continuations.
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
const ROWS: usize = 2048;
const LIMIT: usize = 64;
fn bodies(width: usize, seed: u64) -> Vec<String> {
    assert!([16, 1024].contains(&width));
    (0..ROWS)
        .map(|j| {
            let mut x = seed ^ (j as u64 + 1);
            let mut body = format!("{j:04x}").into_bytes();
            while body.len() < width - 8 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                body.push(b'a' + (x % 26) as u8);
            }
            body.extend_from_slice(if j % 16 == 7 { b"SEL!" } else { b"oth!" });
            body.extend_from_slice(if j % 2 == 0 { b"COM!" } else { b"oth!" });
            body[4] = 0xce;
            body[5] = 0xbb; // Positive UTF8 literal control, fixed total byte width.
            String::from_utf8(body).unwrap()
        })
        .collect()
}
fn ranks(order: &str, seed: u64) -> Vec<usize> {
    let mut ranks: Vec<_> = (0..ROWS).collect();
    if order == "shuffled" {
        let mut state = seed;
        for upper in (1..ROWS).rev() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ranks.swap(upper, state as usize % (upper + 1));
        }
    } else {
        assert_eq!(order, "sorted");
    }
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
                            .map(|(j, body)| LogRecord {
                                // Equal-time ties exercise the remaining full canonical key.
                                observed_time_unix_nano: START + (ranks[i * 128 + j] / 4) as u64,
                                attributes: (0..attrs)
                                    .map(|k| opentelemetry_proto::tonic::common::v1::KeyValue {
                                        key: format!("key-{k}-λ"),
                                        value: Some(AnyValue {
                                            value: Some(any_value::Value::StringValue(format!(
                                                "{k:04}:{}",
                                                "V".repeat(123)
                                            ))),
                                        }),
                                        ..Default::default()
                                    })
                                    .collect(),
                                body: Some(AnyValue {
                                    value: Some(any_value::Value::StringValue(body.clone())),
                                }),
                                ..Default::default()
                            })
                            .collect(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
            };
            let batch = Batch {
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
                    label: "key-first-λ".into(),
                    batch: batch.encode_to_vec(),
                    received_unix_nano: START + i as u64 + 1,
                }],
            }
        })
        .collect()
}
#[cfg(feature = "phase-probe")]
fn phases(path: &std::path::Path) {
    let rows = fabric_frame::probe::take();
    assert!(rows.len() <= 8192, "phase ledger bound");
    let mut out = BufWriter::new(fs::File::create(path).unwrap());
    for row in rows {
        writeln!(
            out,
            "{}",
            json!({"phase":row.name,"depth":row.depth,"start_ns":row.start_ns,
            "wall_ns":row.wall_ns,"thread_cpu_ns":row.after[0].saturating_sub(row.before[0]),
            "before":row.before,"after":row.after})
        )
        .unwrap();
    }
    out.flush().unwrap();
}
fn query(shape: &str) -> serde_json::Value {
    let mut query = json!({"kind":"logs","from_ns":START,"to_ns":START+ROWS as u64,"limit":LIMIT});
    match shape {
        "absent" => query["contains"] = json!("NO-HIT!"),
        "selective" => query["contains"] = json!("SEL!"),
        "common" => query["contains"] = json!("COM!"),
        "broad" => {}
        "empty-literal" => query["contains"] = json!(""),
        "unicode" => query["contains"] = json!("λ"),
        _ => unreachable!(),
    }
    query
}
fn main() {
    use fabric_frame::frame::FrameLog;
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 6, "ROOT WIDTH ATTRS ORDER SEED");
    let root = PathBuf::from(&args[1]);
    let width: usize = args[2].parse().unwrap();
    let attrs: usize = args[3].parse().unwrap();
    assert!([0, 8].contains(&attrs));
    let order = &args[4];
    let seed: u64 = args[5].parse().unwrap();
    let phase_on = std::env::var("BENCH_PHASES").is_ok_and(|v| v == "1");
    assert!(
        !History::borrowed_logs_enabled(),
        "both arms must use owned readers"
    );
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
    assert!(!phase_on);
    assert!(root.starts_with(PathBuf::from(std::env::var("FABRIC_SCRATCH_ROOT").unwrap())));
    fs::create_dir(&root).unwrap();
    let texts = bodies(width, seed);
    let groups = fixture(&texts, &ranks(order, seed), attrs);
    let mut ledger = BufWriter::new(fs::File::create(root.join("records.jsonl")).unwrap());
    for group in &groups {
        for entry in &group.entries {
            writeln!(
                ledger,
                "{}",
                json!({"label":entry.label,"received_ns":entry.received_unix_nano,
            "hex":hex(&entry.batch),"sha256":hash(&entry.batch)})
            )
            .unwrap();
        }
    }
    ledger.flush().unwrap();
    drop(ledger);
    for layout in ["tail", "segment"] {
        let state = root.join(layout);
        drop(Store::open(&state, 128 * 1024 * 1024, CommitMode::INDIVIDUAL).unwrap());
        if layout == "segment" {
            segment::build(&state, 1, &groups).unwrap();
        } else {
            let mut log = FrameLog::open(
                &state.join("journal"),
                128 * 1024 * 1024,
                segment::MAX_GROUP_PAYLOAD,
                |_, _| Ok(()),
            )
            .unwrap();
            for group in &groups {
                log.append(&group.encode_to_vec()).unwrap();
            }
        }
    }
    drop(groups);
    drop(texts);
    #[cfg(feature = "phase-probe")]
    if phase_on {
        phases(&root.join("fixture-phases.jsonl"));
    }
    let mut results = Vec::new();
    for layout in ["tail", "segment"] {
        for (plan_name, plan) in [("scan", Plan::Scan), ("walk", Plan::Walk)] {
            for shape in [
                "absent",
                "selective",
                "common",
                "broad",
                "empty-literal",
                "unicode",
            ] {
                let history = History::with_plan(&root.join(layout), plan);
                let q = query(shape);
                let parsed: Query = serde_json::from_value(q.clone()).unwrap();
                let mut calls = BufWriter::new(
                    fs::File::create(
                        root.join(format!("calls-{layout}-{plan_name}-{shape}.jsonl")),
                    )
                    .unwrap(),
                );
                let mut first = None;
                for iteration in 0..4 {
                    #[cfg(feature = "responsibility-alloc-probe")]
                    let base = allocation::reset();
                    let cpu = process_clock::now();
                    let wall = Instant::now();
                    let answer = history.run(&parsed, 16).unwrap();
                    let wall_ns = wall.elapsed().as_nanos() as u64;
                    let cpu_ns = process_clock::now() - cpu;
                    #[cfg(feature = "responsibility-alloc-probe")]
                    let alloc = {
                        let (live, peak, total, count) = allocation::facts();
                        Some(
                            json!({"base":base,"live":live,"peak":peak,"total":total,"calls":count}),
                        )
                    };
                    #[cfg(not(feature = "responsibility-alloc-probe"))]
                    let alloc: Option<serde_json::Value> = None;
                    #[cfg(feature = "phase-probe")]
                    if phase_on {
                        phases(&root.join(format!(
                            "phases-{layout}-{plan_name}-{shape}-{iteration}.jsonl"
                        )));
                    }
                    assert_eq!(answer["complete"], true);
                    let raw = serde_json::to_vec(&answer).unwrap();
                    if let Some(expected) = &first {
                        assert_eq!(&raw, expected);
                    } else {
                        first = Some(raw.clone());
                    }
                    calls.write_all(&raw).unwrap();
                    calls.write_all(b"\n").unwrap();
                    results.push(
                        json!({"layout":layout,"plan":plan_name,"shape":shape,"query":q,
                    "iteration":iteration,"wall_ns":wall_ns,"cpu_ns":cpu_ns,"allocation":alloc}),
                    );
                }
                calls.flush().unwrap();
                let mut chain = BufWriter::new(
                    fs::File::create(
                        root.join(format!("chain-{layout}-{plan_name}-{shape}.jsonl")),
                    )
                    .unwrap(),
                );
                let raw = first.unwrap();
                chain.write_all(&raw).unwrap();
                chain.write_all(b"\n").unwrap();
                let mut page: serde_json::Value = serde_json::from_slice(&raw).unwrap();
                let mut pages = 1;
                while !page["next_page"].is_null() {
                    assert!(pages < 34, "full chain bound");
                    let mut next = q.clone();
                    next["page"] = page["next_page"].clone();
                    page = history
                        .run(&serde_json::from_value(next).unwrap(), 16)
                        .unwrap();
                    assert_eq!(page["complete"], true);
                    writeln!(chain, "{}", page).unwrap();
                    pages += 1;
                    #[cfg(feature = "phase-probe")]
                    if phase_on {
                        phases(&root.join(format!(
                            "continuation-{layout}-{plan_name}-{shape}-{pages}.jsonl"
                        )));
                    }
                }
                chain.flush().unwrap();
            }
        }
    }
    println!(
        "{}",
        json!({"fixture":{"records":ROWS,"width":width,"attrs":attrs,"order":order,"seed":seed,
        "limit":LIMIT,"timestamp_ties":4,"key_first":History::key_first_enabled(),"borrowed":false,
        "counted":cfg!(feature="responsibility-alloc-probe"),"phase_on":phase_on,
        "cpu_clock_resolution_ns":process_clock::resolution()},"results":results})
    );
}
