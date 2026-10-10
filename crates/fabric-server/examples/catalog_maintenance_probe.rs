//! CQ2 synchronous derived-catalog preparation probe; no server scheduler changes.
use fabric_frame::envelope::Batch;
use fabric_frame::frame::{FileRef, FrameLog};
use fabric_server::{
    query::{History, Plan, Query},
    segment,
    store::{Answer, CommitMode, Entry, Group, Store, Submission, identify_strand},
};
use opentelemetry_proto::tonic::{
    collector::logs::v1::ExportLogsServiceRequest,
    common::v1::{AnyValue, any_value},
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
};
use prost::Message;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::Instant,
};
#[cfg(feature = "responsibility-alloc-probe")]
mod allocation {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    struct Counted;
    static LIVE: AtomicUsize = AtomicUsize::new(0);
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
                TOTAL.fetch_add(layout.size(), Relaxed);
                add(layout.size());
            }
            p
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { System.alloc_zeroed(layout) };
            if !p.is_null() {
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
        n
    }
    pub fn facts() -> (usize, usize, usize) {
        (LIVE.load(Relaxed), PEAK.load(Relaxed), TOTAL.load(Relaxed))
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
fn records(path: &Path, groups: &[Group]) {
    let mut f = fs::File::create(path.join("records.jsonl")).unwrap();
    for g in groups {
        for e in &g.entries {
            writeln!(f,"{}",json!({"label":e.label,"received_ns":e.received_unix_nano,"hex":hex(&e.batch),"sha256":hash(&e.batch)})).unwrap();
        }
    }
}
fn submit(intake: &fabric_server::store::Intake, bytes: &[u8]) -> Answer {
    let (strand, sequence) = identify_strand(bytes).unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    intake.submit(Submission {
        label: "fixture".into(),
        strand,
        sequence,
        bytes: bytes.to_vec(),
        reply: tx,
    });
    rx.blocking_recv().unwrap()
}
fn recover(path: &Path) -> Vec<Group> {
    let mut out = Vec::new();
    let mut seq = 0;
    let store =
        Store::open_with(path, 1024 * 1024 * 1024, 512 * 1024, CommitMode::GROUPED).unwrap();
    drop(store);
    Store::replay(path, 1024 * 1024 * 1024, |e| {
        seq += 1;
        out.push(Group {
            group_sequence: seq,
            entries: vec![Entry {
                label: e.label.to_string(),
                batch: e.batch.to_vec(),
                received_unix_nano: e.received_unix_nano,
            }],
        });
        Ok(())
    })
    .unwrap();
    out
}

fn measure<T>(rows: &mut Vec<Value>, name: &str, f: impl FnOnce() -> T) -> T {
    #[cfg(feature = "responsibility-alloc-probe")]
    let base = allocation::reset();
    let cpu = process_clock::now();
    let wall = Instant::now();
    let value = f();
    let wall_ns = wall.elapsed().as_nanos() as u64;
    let cpu_ns = process_clock::now() - cpu;
    #[cfg(feature = "responsibility-alloc-probe")]
    let (live, peak, total) = allocation::facts();
    #[cfg(feature = "responsibility-alloc-probe")]
    let allocation = json!({"live_delta":live as i128-base as i128,"peak_increment":peak.saturating_sub(base),"allocated_bytes":total});
    #[cfg(not(feature = "responsibility-alloc-probe"))]
    let allocation = Value::Null;
    rows.push(json!({"stage":name,"wall_ns":wall_ns,"cpu_ns":cpu_ns,"allocation":allocation}));
    value
}
fn history(state: &Path) -> History {
    History::with_plan(state, Plan::Walk)
}
fn drain(root: &Path, id: usize) {
    let wrapper: Value =
        serde_json::from_slice(&fs::read(root.join(format!("answer-{id}.json"))).unwrap()).unwrap();
    let mut page = wrapper["answer"].clone();
    let snapshot = page["snapshot"].clone();
    let newest = wrapper["newest"].as_u64().unwrap();
    let h = history(&root.join("state"));
    let mut out = BufWriter::new(fs::File::create(root.join("chain.jsonl")).unwrap());
    let mut pages = 0;
    let mut total = 0;
    loop {
        let bytes = serde_json::to_vec(&page).unwrap();
        total += bytes.len() + 1;
        assert!(total <= 256 * 1024 * 1024);
        out.write_all(&bytes).unwrap();
        out.write_all(b"\n").unwrap();
        pages += 1;
        if page["next_page"].is_null() {
            break;
        }
        assert!(pages <= wrapper["records"].as_u64().unwrap() as usize / 10000 + 1);
        let mut q = wrapper["query"].clone();
        q["page"] = page["next_page"].clone();
        page = h
            .run(&serde_json::from_value::<Query>(q).unwrap(), newest)
            .unwrap();
        assert_eq!(page["snapshot"], snapshot);
        assert_eq!(page["complete"], true);
    }
    out.flush().unwrap();
}
fn main() {
    let a: Vec<_> = std::env::args().collect();
    let root = PathBuf::from(&a[1]);
    if a[2] == "drain" {
        drain(&root, a[3].parse().unwrap());
        return;
    }
    let eager = match a[2].as_str() {
        "lazy" => false,
        "eager" => true,
        _ => panic!("lazy/eager/drain only"),
    };
    let rate: usize = a[3].parse().unwrap();
    assert!([0, 1, 4].contains(&rate));
    let seed: usize = std::env::var("BENCH_SEED_RECORDS").map_or(65536, |s| s.parse().unwrap());
    assert!([128, 65536].contains(&seed));
    let skip: usize = std::env::var("BENCH_SKIP_REFRESH_AT").map_or(0, |s| s.parse().unwrap());
    assert!([0, 32].contains(&skip));
    fs::create_dir(&root).unwrap();
    let state = root.join("state");
    let count = seed + 32 * 128;
    let texts = bodies(1024, count);
    let ranks: Vec<_> = (0..count).collect();
    let groups = fixture(&texts, &ranks);
    let initial = seed / 128;
    fs::write(root.join("source.log"), texts.join("\n") + "\n").unwrap();
    let (intake, writer) =
        Store::open_with(&state, 1024 * 1024 * 1024, 512 * 1024, CommitMode::GROUPED)
            .unwrap()
            .spawn_joinable()
            .unwrap();
    for g in &groups[..initial] {
        assert_eq!(
            submit(&intake, &g.entries[0].batch),
            Answer::Ack(g.group_sequence)
        );
    }
    drop(intake);
    writer.join().unwrap();
    let seed_actual = recover(&state);
    assert_eq!(seed_actual.len(), initial);
    assert!(
        seed_actual
            .iter()
            .zip(&groups)
            .all(|(a, b)| a.entries[0].batch == b.entries[0].batch)
    );
    // Origin: catalog-maintenance-0-preflight-01 recovered 30 of 33 Groups.
    // Publishing a seed-only Segment while that seed's journal remains active
    // lets later appends share its label and be reclaimed on Store reopen.
    // Close the seed at a real file boundary before any Segment is published.
    let mut active_first = None;
    let mut seed_log = FrameLog::open(
        &state.join("journal"),
        1024 * 1024 * 1024,
        4 * 1024 * 1024,
        |payload, position| {
            let group = Group::decode(payload).unwrap();
            assert!(group.group_sequence <= initial as u64);
            if position.file == FileRef::Active && active_first.is_none() {
                active_first = Some(group.group_sequence);
            }
            Ok(())
        },
    )
    .unwrap();
    if let Some(first) = active_first {
        seed_log.rotate(first).unwrap();
    }
    drop(seed_log);
    let segments = if seed == 65536 { 64 } else { 1 };
    for (index, chunk) in seed_actual.chunks(initial / segments).enumerate() {
        segment::build(&state, index as u64 + 1, chunk).unwrap();
    }
    let (intake, writer) =
        Store::open_with(&state, 1024 * 1024 * 1024, 512 * 1024, CommitMode::GROUPED)
            .unwrap()
            .spawn_joinable()
            .unwrap();
    let h = history(&state);
    // Both treatments acquire exactly the same initial view before append timing.
    let broad = json!({"kind":"logs","from_ns":START,"to_ns":START+count as u64,"limit":10000});
    let prime: Query = serde_json::from_value(broad.clone()).unwrap();
    h.run(&prime, initial as u64).unwrap();
    let mut rows = Vec::with_capacity(32 * (rate * 2 + 2));
    let mut answers = Vec::<(Value, Vec<u8>)>::with_capacity(32 * rate + 2);
    let mut retained = 0;
    let mut refreshes = 0;
    let mut queries = 0;
    let total_cpu = process_clock::now();
    let total_wall = Instant::now();
    for append in 1..=32 {
        let g = &groups[initial + append - 1];
        measure(&mut rows, "append_ack", || {
            assert_eq!(
                submit(&intake, &g.entries[0].batch),
                Answer::Ack(g.group_sequence)
            )
        });
        let newest = intake.committed_group();
        assert_eq!(newest, g.group_sequence);
        if eager && append != skip {
            measure(&mut rows, "refresh", || h.refresh_catalog(newest).unwrap());
            refreshes += 1;
        }
        for _ in 0..rate {
            let mut q = broad.clone();
            let shape = if queries % 2 == 0 {
                "broad"
            } else {
                q["contains"] = json!("bench-0007 ");
                "selective"
            };
            let parsed: Query = serde_json::from_value(q.clone()).unwrap();
            let answer = measure(&mut rows, "query", || h.run(&parsed, newest).unwrap());
            assert_eq!(answer["complete"], true);
            let bytes = measure(&mut rows, "serialize", || {
                serde_json::to_vec(&answer).unwrap()
            });
            retained += bytes.len();
            assert!(retained <= 1024 * 1024 * 1024);
            answers.push((json!({"id":answers.len(),"shape":shape,"query":q,"newest":newest,"records":(initial+append)*128}),bytes));
            queries += 1;
        }
    }
    let wall_ns = total_wall.elapsed().as_nanos() as u64;
    let cpu_ns = process_clock::now() - total_cpu;
    rows.push(json!({"stage":"total_schedule","wall_ns":wall_ns,"cpu_ns":cpu_ns,"appends":32,"queries":queries,"refreshes":refreshes,"includes":"append_ack+refresh+query+serialize+bookkeeping"}));
    // Quiet broad/selective controls are outside the schedule, including rate zero.
    for shape in ["broad", "selective"] {
        let mut q = broad.clone();
        if shape == "selective" {
            q["contains"] = json!("bench-0007 ");
        }
        let newest = intake.committed_group();
        let answer = h
            .run(&serde_json::from_value(q.clone()).unwrap(), newest)
            .unwrap();
        answers.push((json!({"id":answers.len(),"shape":shape,"query":q,"newest":newest,"records":count,"quiet_control":true}),serde_json::to_vec(&answer).unwrap()));
    }
    drop(intake);
    writer.join().unwrap();
    let actual = recover(&state);
    assert_eq!(actual.len(), groups.len());
    assert!(
        actual
            .iter()
            .zip(&groups)
            .all(|(a, b)| a.entries[0].batch == b.entries[0].batch)
    );
    records(&root, &actual);
    for (mut wrapper, bytes) in answers {
        wrapper["answer"] = serde_json::from_slice(&bytes).unwrap();
        fs::write(
            root.join(format!("answer-{}.json", wrapper["id"].as_u64().unwrap())),
            serde_json::to_vec(&wrapper).unwrap(),
        )
        .unwrap();
    }
    let mut timings = BufWriter::new(fs::File::create(root.join("timings.jsonl")).unwrap());
    for row in rows {
        writeln!(timings, "{row}").unwrap();
    }
    timings.flush().unwrap();
    println!(
        "{}",
        json!({"complete":true,"eager":eager,"seed":42,"seed_records":seed,"seed_segments":segments,"append_records":4096,"query_rate":rate,"refreshes":refreshes,"skip_refresh_at":skip,"borrowed_logs":History::borrowed_logs_enabled(),"shared_catalog":false,"counted":cfg!(feature="responsibility-alloc-probe"),"cpu_clock_resolution_ns":process_clock::resolution(),"answers":queries+2,"retained_first_page_bytes":retained})
    );
}
