//! Native public-boundary isolation. See responsibility-experiment-design.md.
//! Benchmark-only observation; no production scheduling or durability changes.
use fabric_frame::envelope::Batch;
use fabric_o11y::spindle::{
    host, log_source,
    sender::{Sender, ServerTarget},
    spool::Spool,
};
use fabric_server::store::Answer;
use fabric_server::{
    control::{Control, DesiredConfig},
    query::{History, Plan, Query},
    rows::{self, Rows},
    sealer, segment,
    store::{CommitMode, Entry, Group, Store, Submission, identify_strand},
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
    cell::RefCell,
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
const IO_NAMES: [&str; 7] = [
    "rchar",
    "wchar",
    "syscr",
    "syscw",
    "read_bytes",
    "write_bytes",
    "cancelled_write_bytes",
];
fn io() -> [u64; 7] {
    let s = fs::read_to_string("/proc/self/io").unwrap();
    let mut values = [0; 7];
    for line in s.lines() {
        let (k, v) = line.split_once(':').unwrap();
        if let Some(index) = IO_NAMES.iter().position(|name| *name == k) {
            values[index] = v.trim().parse().unwrap();
        }
    }
    values
}
struct Measurement {
    name: usize,
    units: usize,
    wall_ns: u64,
    cpu_ns: u64,
    io_delta: Option<[u64; 7]>,
    allocation: Option<[usize; 4]>,
}
enum LedgerEvent {
    Measurement(Measurement),
    Ack { sequence: usize, sha256: String },
}
#[derive(Default)]
struct Ledger {
    names: Vec<String>,
    events: Vec<LedgerEvent>,
    limit: usize,
    detailed: bool,
}
thread_local! {
    static LEDGER: RefCell<Ledger> = RefCell::new(Ledger::default());
}
fn initialize_ledger(limit: usize, detailed: bool) {
    assert!(limit <= 131_072, "measurement ledger bound");
    LEDGER.with(|ledger| {
        *ledger.borrow_mut() = Ledger {
            names: Vec::with_capacity(128),
            events: Vec::with_capacity(limit),
            limit,
            detailed,
        };
    });
}
fn push(event: LedgerEvent) {
    LEDGER.with(|ledger| {
        let mut ledger = ledger.borrow_mut();
        assert!(
            ledger.events.len() < ledger.limit,
            "measurement ledger exhausted"
        );
        ledger.events.push(event);
    });
}
fn measure<T>(name: &str, units: usize, f: impl FnOnce() -> T) -> T {
    // Intern a phase name once, then append fixed-size samples into preallocated
    // storage. JSON formatting/stdout and counter JSON construction happen only
    // after all native operation spans. Detailed IO acquisition is deliberately
    // outside the wall span and is a separately calibrated observer coordinate.
    let (name, detailed) = LEDGER.with(|ledger| {
        let mut ledger = ledger.borrow_mut();
        let index = match ledger.names.iter().position(|s| s == name) {
            Some(index) => index,
            None => {
                assert!(ledger.names.len() < 128, "phase name bound");
                ledger.names.push(name.to_owned());
                ledger.names.len() - 1
            }
        };
        (index, ledger.detailed)
    });
    let before = detailed.then(io);
    #[cfg(feature = "responsibility-alloc-probe")]
    let base = allocation::reset();
    let cpu_start = process_clock::now();
    let t = Instant::now();
    let result = f();
    let wall_ns = u64::try_from(t.elapsed().as_nanos()).unwrap();
    let cpu_ns = process_clock::now() - cpu_start;
    #[cfg(feature = "responsibility-alloc-probe")]
    let allocation = {
        let (live, peak, total) = allocation::facts();
        Some([base, live, peak, total])
    };
    #[cfg(not(feature = "responsibility-alloc-probe"))]
    let allocation = None;
    let io_delta = before.map(|before| {
        let after = io();
        std::array::from_fn(|i| after[i].saturating_sub(before[i]))
    });
    push(LedgerEvent::Measurement(Measurement {
        name,
        units,
        wall_ns,
        cpu_ns,
        io_delta,
        allocation,
    }));
    result
}
fn flush_ledger() -> usize {
    LEDGER.with(|ledger| {
        let ledger = ledger.borrow();
        let count = ledger.events.len();
        let mut out = BufWriter::new(std::io::stdout().lock());
        for event in &ledger.events {
            let row = match event {
                LedgerEvent::Ack { sequence, sha256 } => json!({
                    "stage":"acked_hash", "sequence":sequence, "sha256":sha256,
                }),
                LedgerEvent::Measurement(m) => {
                    let mut delta = serde_json::Map::new();
                    if let Some(values) = m.io_delta {
                        for (name, value) in IO_NAMES.iter().zip(values) {
                            delta.insert((*name).into(), json!(value));
                        }
                    }
                    let allocation = m.allocation.map(|[base, live, peak, total]| json!({
                        "baseline_live_bytes":base, "end_live_bytes":live,
                        "peak_live_bytes":peak, "incremental_peak_bytes":peak.saturating_sub(base),
                        "cumulative_requested_bytes":total,
                    }));
                    json!({"stage":ledger.names[m.name],"units":m.units,
                        "wall_ns":m.wall_ns,"cpu_ns":m.cpu_ns,
                        "proc_io_delta":delta,"allocation":allocation})
                }
            };
            writeln!(out, "{row}").unwrap();
        }
        out.flush().unwrap();
        count
    })
}
fn high_water_kib() -> u64 {
    fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")
                .map(|v| v.split_whitespace().next().unwrap().parse().unwrap())
        })
        .expect("Linux VmHWM unavailable")
}
fn env_count(name: &str, default: usize) -> usize {
    std::env::var(name).map_or(default, |s| s.parse().expect("invalid benchmark count"))
}
fn preflight() -> bool {
    std::env::var("BENCH_PREFLIGHT").is_ok_and(|s| s == "1")
}
fn repeats(default: usize) -> usize {
    if preflight() { 1 } else { default }
}
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
fn batch_log_count(bytes: &[u8]) -> usize {
    let batch = Batch::decode(bytes).unwrap();
    let request = ExportLogsServiceRequest::decode(batch.logs.as_slice()).unwrap();
    request
        .resource_logs
        .iter()
        .flat_map(|r| &r.scope_logs)
        .map(|s| s.log_records.len())
        .sum()
}
fn seed_store(path: &Path, groups: &[Group], rotate: u64, timed: bool, per_batch: usize) {
    let (intake, thread) = Store::open_with(path, 256 * 1024 * 1024, rotate, CommitMode::GROUPED)
        .unwrap()
        .spawn_joinable()
        .unwrap();
    for g in groups {
        let e = &g.entries[0];
        let f = || {
            let a = submit(&intake, &e.batch);
            assert_eq!(a, Answer::Ack(g.group_sequence));
        };
        if timed {
            measure("server_submit_to_durable_answer", per_batch, f);
        } else {
            f();
        }
    }
    drop(intake);
    thread.join().unwrap();
}
fn recover(path: &Path) -> Vec<Group> {
    let mut out = Vec::new();
    let mut seq = 0;
    let store = Store::open_with(path, 256 * 1024 * 1024, 512 * 1024, CommitMode::GROUPED).unwrap();
    drop(store);
    Store::replay(path, 256 * 1024 * 1024, |e| {
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
fn main() {
    // Preserve completed ledger rows when native/assertion failure unwinds.
    // No successful completion marker is printed by this hook. A kernel kill
    // can still lose the in-memory ledger, which the runner records as partial.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        static FLUSHING_FAILURE: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        let can_flush = LEDGER.with(|ledger| ledger.try_borrow().is_ok());
        if can_flush && !FLUSHING_FAILURE.swap(true, std::sync::atomic::Ordering::Relaxed) {
            flush_ledger();
        }
        previous_hook(info);
    }));
    let a: Vec<_> = std::env::args().collect();
    let root = PathBuf::from(&a[1]);
    let mode = &a[2];
    let size: usize = a[3].parse().unwrap();
    fs::create_dir_all(&root).unwrap();
    let count = env_count("BENCH_RECORDS", 4096);
    assert!(
        (128..=65_536).contains(&count) && count.is_multiple_of(128),
        "BENCH_RECORDS must be a multiple of 128 in 128..=65536"
    );
    let order = std::env::var("BENCH_ORDER").unwrap_or_else(|_| "sorted".into());
    let observer = std::env::var("BENCH_OBSERVER").unwrap_or_else(|_| "minimal".into());
    assert!(matches!(observer.as_str(), "minimal" | "detailed"));
    let query_repeats = if preflight() {
        1
    } else {
        env_count("BENCH_QUERY_REPEATS", 12)
    };
    assert!(query_repeats <= 1000, "query repetition bound");
    let ledger_capacity =
        (16 * (query_repeats + 1) * 2 + count / 128 * 20 + size * 2 + 2048).max(4096);
    initialize_ledger(ledger_capacity, observer == "detailed");
    let body_size = env_count(
        "BENCH_BODY_SIZE",
        if mode == "control" || mode == "scheduling" {
            900
        } else {
            size
        },
    );
    let texts = bodies(body_size, count);
    let ranks = timestamp_ranks(texts.len(), &order);
    let groups = fixture(&texts, &ranks);
    let per_batch = texts.len() / groups.len();
    assert_eq!(texts.len(), groups.len() * per_batch);
    assert!(
        groups
            .iter()
            .all(|g| batch_log_count(&g.entries[0].batch) == per_batch)
    );
    records(&root, &groups);
    let source = texts.join("\n") + "\n";
    fs::write(root.join("source.log"), source.as_bytes()).unwrap();
    let encoded_batch_bytes: usize = groups.iter().map(|g| g.entries[0].batch.len()).sum();
    let encoded_group_bytes: usize = groups.iter().map(Group::encoded_len).sum();
    let mut rank_hash = Sha256::new();
    for rank in &ranks {
        rank_hash.update((*rank as u64).to_le_bytes());
    }
    let ranks_sha256 = format!("{:x}", rank_hash.finalize());
    println!(
        "{}",
        json!({"stage":"fixture","records":texts.len(),"source_bytes":source.len(),"source_sha256":hash(source.as_bytes()),"allocator_counted":cfg!(feature="responsibility-alloc-probe"),"size":size,"body_bytes":texts[0].len(),"timestamp_order":order,"seed":42,"observer":observer,"cpu_clock":"CLOCK_PROCESS_CPUTIME_ID","cpu_clock_resolution_ns":process_clock::resolution(),"ledger_capacity":ledger_capacity,"query_warm_repeats":query_repeats,"preflight":preflight(),"encoded_batch_bytes":encoded_batch_bytes,"encoded_group_bytes":encoded_group_bytes,"timestamp_ranks_sha256":ranks_sha256,"answer_ledger_limit_bytes":256*1024*1024,"observer_batch_iterations":if preflight() {1} else {env_count("BENCH_OBSERVER_BATCH",20_000)},"scheduling_rotate_bytes":env_count("BENCH_ROTATE_BYTES",512*1024),"scheduling_memory_guard_bytes":env_count("BENCH_MEMORY_GUARD_BYTES",1024*1024*1024)})
    );
    match mode.as_str() {
        "collection" => {
            for _ in 0..repeats(10) {
                let mut cursor = None;
                let mut got = Vec::new();
                loop {
                    let r = measure("file_read_validate_cursor", 1, || {
                        log_source::read_lines_costed(
                            &root.join("source.log"),
                            cursor.as_ref(),
                            928 * 1024,
                            256,
                        )
                        .unwrap()
                    });
                    assert!(r.gaps.is_empty());
                    got.extend(r.lines.iter().map(|l| l.body.clone()));
                    cursor = Some(r.cursor);
                    if r.backlog_bytes == 0 {
                        break;
                    }
                }
                assert_eq!(got, texts);
            }
            for _ in 0..repeats(20) {
                let s = measure("host_sample", 1, || {
                    host::sample(&host::Paths::default()).unwrap()
                });
                assert!(!s.points.is_empty());
            }
        }
        "storage" => {
            let mut spool = Spool::open(root.join("spool"), 256 * 1024 * 1024).unwrap();
            let mut committed = Vec::new();
            for g in &groups {
                let b = Batch::decode(g.entries[0].batch.as_slice()).unwrap();
                committed.push(measure("spool_clone_encode_append_sync", per_batch, || {
                    spool.append(&b).unwrap()
                }));
            }
            for b in &committed {
                let (seq, bytes) = measure("spool_read_decode", per_batch, || {
                    spool.next_unacked().unwrap().unwrap()
                });
                assert_eq!(bytes, b.encode_to_vec());
                measure("spool_ack_persist_advance", per_batch, || {
                    spool.record_ack(seq).unwrap()
                });
            }
            drop(spool);
            let mut reopened = measure("spool_open_recovery", texts.len(), || {
                Spool::open(root.join("spool"), 256 * 1024 * 1024).unwrap()
            });
            let mut n = 0;
            measure("spool_replay_decode", texts.len(), || {
                reopened
                    .replay(|b| {
                        assert_eq!(b, committed[n]);
                        n += 1;
                        Ok(())
                    })
                    .unwrap()
            });
            assert_eq!(n, groups.len());
            drop(reopened);
            let state = root.join("state");
            seed_store(&state, &groups, 512 * 1024, true, per_batch);
            let recovered = measure("server_open_replay", texts.len(), || recover(&state));
            assert_eq!(
                recovered
                    .iter()
                    .map(|g| g.entries[0].batch.clone())
                    .collect::<Vec<_>>(),
                groups
                    .iter()
                    .map(|g| g.entries[0].batch.clone())
                    .collect::<Vec<_>>()
            );
            records(&root, &recovered);
        }
        "processing" | "buffers" => {
            for _ in 0..repeats(5) {
                let mut parsed = Rows::default();
                for g in &groups {
                    measure("batch_decode_project_rows", per_batch, || {
                        rows::extract(g.group_sequence, &g.entries[0], &mut parsed).unwrap()
                    });
                }
                assert_eq!(parsed.logs.len(), texts.len());
                // Keep decoding and reset/copy ownership outside the sort span.
                // Each sort sees the original disorder, never the prior result.
                let mut sorted = parsed.logs.clone();
                measure("sort_log_rows", texts.len(), || {
                    sorted.sort_by_key(|r| (r.observed_ns, r.node_id, r.sequence, r.index))
                });
                assert!(sorted.windows(2).all(|pair| (
                    pair[0].observed_ns,
                    pair[0].node_id,
                    pair[0].sequence,
                    pair[0].index
                ) <= (
                    pair[1].observed_ns,
                    pair[1].node_id,
                    pair[1].sequence,
                    pair[1].index
                )));
                let mut source_positions: Vec<_> = (0..texts.len()).collect();
                source_positions.sort_unstable_by_key(|&j| ranks[j]);
                for (row, position) in sorted.iter().zip(source_positions) {
                    assert_eq!(row.observed_ns, START + ranks[position] as u64);
                    assert_eq!(row.body, texts[position]);
                    assert_eq!(row.sequence, (position / 128 + 1) as u64);
                    assert_eq!(row.index, (position % 128) as u32);
                }
            }
            let state = root.join("state");
            measure("segment_build_inclusive_publish", texts.len(), || {
                segment::build(&state, 1, &groups).unwrap()
            });
            // Native fresh recovery validates the Segment's raw custody; the
            // independent runner grades the recovered records after timing.
            let recovered = recover(&state);
            assert_eq!(recovered.len(), groups.len());
            assert_eq!(
                recovered
                    .iter()
                    .map(|g| &g.entries[0].batch)
                    .collect::<Vec<_>>(),
                groups
                    .iter()
                    .map(|g| &g.entries[0].batch)
                    .collect::<Vec<_>>()
            );
            records(&root, &recovered);
        }
        "query" => {
            const ANSWER_LEDGER_BYTES: usize = 256 * 1024 * 1024;
            for kind in ["tail", "segment"] {
                let state = root.join(kind);
                if kind == "tail" {
                    seed_store(&state, &groups, 128 * 1024 * 1024, false, per_batch);
                } else {
                    // A directory alone is not a legal active journal. Construct
                    // the native empty log before adding the Segment fixture.
                    let store = Store::open_with(
                        &state,
                        256 * 1024 * 1024,
                        128 * 1024 * 1024,
                        CommitMode::GROUPED,
                    )
                    .unwrap();
                    drop(store);
                    segment::build(&state, 1, &groups).unwrap();
                }
                let actual = recover(&state);
                assert_eq!(actual.len(), groups.len());
                assert_eq!(
                    actual
                        .iter()
                        .map(|g| &g.entries[0].batch)
                        .collect::<Vec<_>>(),
                    groups
                        .iter()
                        .map(|g| &g.entries[0].batch)
                        .collect::<Vec<_>>()
                );
                let record_dir = root.join(format!("{kind}-records"));
                fs::create_dir(&record_dir).unwrap();
                records(&record_dir, &actual);
                for plan in [Plan::Scan, Plan::Walk] {
                    let name = if plan == Plan::Scan { "scan" } else { "walk" };
                    let mut shapes = vec![
                        (
                            "empty",
                            json!({"kind":"logs","from_ns":1,"to_ns":2,"limit":10000}),
                        ),
                        (
                            "selective",
                            json!({"kind":"logs","from_ns":START,"to_ns":START+texts.len() as u64,"contains":"bench-0007 ","limit":10000}),
                        ),
                        (
                            "common",
                            json!({"kind":"logs","from_ns":START,"to_ns":START+texts.len() as u64,"contains":"RRRR","limit":10000}),
                        ),
                        (
                            "broad",
                            json!({"kind":"logs","from_ns":START,"to_ns":START+texts.len() as u64,"limit":10000}),
                        ),
                    ];
                    // Rotate shape order deterministically without combining
                    // their first-call and warm populations. "First" resets
                    // History caches; the OS page cache remains buffered/warm.
                    let shift = env_count("BENCH_QUERY_ROTATION", 0) % shapes.len();
                    shapes.rotate_left(shift);
                    for (shape, q) in shapes {
                        let h = History::with_plan(&state, plan);
                        let query: Query = serde_json::from_value(q.clone()).unwrap();
                        let first_stage = format!("query_{kind}_{name}_{shape}_first");
                        let warm_stage = format!("query_{kind}_{name}_{shape}_warm");
                        let first_serial =
                            format!("answer_json_serialize_{kind}_{name}_{shape}_first");
                        let warm_serial =
                            format!("answer_json_serialize_{kind}_{name}_{shape}_warm");
                        let mut answers = Vec::with_capacity(query_repeats + 1);
                        let mut retained_bytes = 0;
                        for i in 0..=query_repeats {
                            let answer =
                                measure(if i == 0 { &first_stage } else { &warm_stage }, 1, || {
                                    h.run(&query, groups.len() as u64).unwrap()
                                });
                            assert!(answer["complete"].as_bool().unwrap());
                            let bytes = measure(
                                if i == 0 { &first_serial } else { &warm_serial },
                                answer["rows"].as_array().unwrap().len(),
                                || serde_json::to_vec(&answer).unwrap(),
                            );
                            retained_bytes += bytes.len();
                            assert!(
                                retained_bytes <= ANSWER_LEDGER_BYTES,
                                "query answer ledger >256 MiB; register a smaller repetition population"
                            );
                            answers.push(bytes);
                        }
                        // Every actual answer, including its envelope, is
                        // retained for independent grading after timing. No
                        // checksum stands in for row/oracle validation.
                        let query_bytes = serde_json::to_vec(&q).unwrap();
                        let file = fs::File::create(
                            root.join(format!("answer-{kind}-{name}-{shape}.jsonl")),
                        )
                        .unwrap();
                        let mut out = BufWriter::new(file);
                        for (i, answer) in answers.iter().enumerate() {
                            out.write_all(b"{\"query\":").unwrap();
                            out.write_all(&query_bytes).unwrap();
                            out.write_all(b",\"answer\":").unwrap();
                            out.write_all(answer).unwrap();
                            writeln!(
                                out,
                                ",\"population\":\"{}\",\"iteration\":{i}}}",
                                if i == 0 { "first" } else { "warm" }
                            )
                            .unwrap();
                        }
                        out.flush().unwrap();
                    }
                }
            }
        }
        "control" => {
            let state = root.join("control");
            let mut c = Control::open(&state).unwrap();
            let cfg = DesiredConfig {
                logs: vec![],
                metric_interval_s: 15,
            };
            for i in 0..size {
                measure("control_enroll_persist", 1, || {
                    c.enroll(&format!("node{i:03}"), cfg.clone()).unwrap()
                });
            }
            for j in 0..repeats(10) {
                let r = measure("control_update_persist", 1, || {
                    c.set_config("node000", cfg.clone()).unwrap()
                });
                assert_eq!(r.revision, j as u64 + 2);
                let p = measure("control_poll", 1, || {
                    c.poll("node000", r.revision, None).unwrap()
                });
                assert_eq!(p.revision, r.revision);
                let v = measure("control_inventory", size, || c.inventory());
                assert_eq!(v.len(), size);
            }
            drop(c);
            let c = measure("control_open_recovery", size, || {
                Control::open(&state).unwrap()
            });
            assert_eq!(c.inventory().len(), size);
            assert_eq!(c.inventory()[0].0.revision, repeats(10) as u64 + 1);
        }
        "scheduling" => {
            assert!([1, 2, 4].contains(&size), "sealing worker cuts are 1/2/4");
            let rotate_bytes = env_count("BENCH_ROTATE_BYTES", 512 * 1024) as u64;
            assert!(rotate_bytes > 0);
            let guard = env_count("BENCH_MEMORY_GUARD_BYTES", 1024 * 1024 * 1024);
            let encoded: usize = groups.iter().map(|g| g.encoded_len()).sum();
            let predicted = source.len() * 4
                + encoded * 3
                + (rotate_bytes as usize).min(encoded) * size * 8
                + ledger_capacity * std::mem::size_of::<LedgerEvent>();
            assert!(
                predicted <= guard,
                "scheduling memory estimate {predicted} exceeds guard {guard}"
            );
            let state = root.join("state");
            seed_store(&state, &groups, rotate_bytes, false, per_batch);
            let store =
                Store::open_with(&state, 256 * 1024 * 1024, rotate_bytes, CommitMode::GROUPED)
                    .unwrap();
            let mut before = Vec::new();
            Store::replay(&state, 256 * 1024 * 1024, |e| {
                before.push(e.batch.to_vec());
                Ok(())
            })
            .unwrap();
            let (intake, thread) = store.spawn_joinable().unwrap();
            measure("native_sealer_pass_inclusive", texts.len(), || {
                sealer::pass(
                    &state,
                    &intake,
                    sealer::Retention {
                        max_age_s: u64::MAX,
                        max_bytes: u64::MAX,
                    },
                    size,
                )
                .unwrap()
            });
            drop(intake);
            thread.join().unwrap();
            let after = recover(&state);
            assert_eq!(
                before,
                after
                    .iter()
                    .map(|g| g.entries[0].batch.clone())
                    .collect::<Vec<_>>()
            );
            records(&root, &after);
        }
        "observation" => {
            for _ in 0..repeats(100) {
                let bytes = &groups[0].entries[0].batch;
                measure("observer_empty_span", 1, || std::hint::black_box(()));
                measure("observer_sha256", bytes.len(), || hash(bytes));
                measure("observer_hex_format", bytes.len(), || hex(bytes));
            }
            // A long batch exposes external sampler/allocator effects without
            // thousands of per-operation clock/IO snapshots. Hash the actual
            // whole Batch, but bound hex formatting to its first 256 bytes.
            let iterations = if preflight() {
                1
            } else {
                env_count("BENCH_OBSERVER_BATCH", 20_000)
            };
            assert!(iterations <= 100_000, "observer batch bound");
            let bytes = &groups[0].entries[0].batch;
            let prefix = &bytes[..bytes.len().min(256)];
            measure("observer_hash_format_batch", iterations, || {
                for _ in 0..iterations {
                    std::hint::black_box(hash(std::hint::black_box(bytes)));
                    std::hint::black_box(hex(std::hint::black_box(prefix)));
                }
            });
        }
        "delivery" => {
            let target = ServerTarget {
                url: a[4].clone(),
                ca: PathBuf::from(&a[5]),
                token_file: PathBuf::from(&a[6]),
            };
            let sender = measure("sender_setup", 1, || Sender::new(&target).unwrap());
            let mut spool = Spool::open(root.join("spool"), 256 * 1024 * 1024).unwrap();
            let mut expected = Vec::new();
            for g in &groups {
                let b = spool
                    .append(&Batch::decode(g.entries[0].batch.as_slice()).unwrap())
                    .unwrap();
                expected.push(b.encode_to_vec());
            }
            for (i, b) in expected.iter().enumerate() {
                let (seq, bytes) = measure("delivery_spool_read_decode", per_batch, || {
                    spool.next_unacked().unwrap().unwrap()
                });
                assert_eq!(&bytes, b);
                let answer = measure("native_tls_send_server_answer", per_batch, || {
                    sender.send(&bytes)
                });
                assert_eq!(answer, fabric_o11y::spindle::sender::Delivery::Ack(seq));
                measure("delivery_ack_persist", per_batch, || {
                    spool.record_ack(seq).unwrap()
                });
                push(LedgerEvent::Ack {
                    sequence: i + 1,
                    sha256: hash(b),
                });
            }
        }
        _ => panic!("unknown mode"),
    }
    let measurements = flush_ledger();
    println!(
        "{}",
        json!({"stage":"complete","mode":mode,
        "vm_hwm_kib":high_water_kib(),"ledger_events":measurements,
        "cpu_clock_resolution_ns":process_clock::resolution(),"observer":observer})
    );
}
