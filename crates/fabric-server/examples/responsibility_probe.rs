//! Native public-boundary isolation. See responsibility-isolation-protocol.md.
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
    fs,
    io::Write,
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
fn cpu() -> u64 {
    let s = fs::read_to_string("/proc/self/stat").unwrap();
    let a: Vec<_> = s.rsplit_once(") ").unwrap().1.split_whitespace().collect();
    a[11].parse::<u64>().unwrap() + a[12].parse::<u64>().unwrap()
}
fn io() -> Value {
    let s = fs::read_to_string("/proc/self/io").unwrap();
    let mut m = serde_json::Map::new();
    for line in s.lines() {
        let (k, v) = line.split_once(':').unwrap();
        m.insert(k.into(), json!(v.trim().parse::<u64>().unwrap()));
    }
    Value::Object(m)
}
fn measure<T>(name: &str, units: usize, f: impl FnOnce() -> T) -> T {
    let c = cpu();
    let before = io();
    #[cfg(feature = "responsibility-alloc-probe")]
    let base = allocation::reset();
    let t = Instant::now();
    let result = f();
    let ns = t.elapsed().as_nanos();
    #[cfg(feature = "responsibility-alloc-probe")]
    let (live, peak, total) = allocation::facts();
    let dc = cpu() - c;
    let after = io();
    let mut delta = serde_json::Map::new();
    for (k, v) in before.as_object().unwrap() {
        delta.insert(
            k.clone(),
            json!(after[k].as_u64().unwrap() - v.as_u64().unwrap()),
        );
    }
    let mut row =
        json!({"stage":name,"units":units,"wall_ns":ns,"cpu_ticks":dc,"proc_io_delta":delta});
    #[cfg(feature = "responsibility-alloc-probe")]
    {
        row["allocation"] = json!({"baseline_live_bytes":base,"end_live_bytes":live,"peak_live_bytes":peak,"incremental_peak_bytes":peak.saturating_sub(base),"cumulative_requested_bytes":total});
    }
    #[cfg(not(feature = "responsibility-alloc-probe"))]
    {
        row["allocation"] = Value::Null;
    }
    println!("{row}");
    result
}
fn hash(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}
const START: u64 = 1_600_000_000_000_000_000;
fn bodies(size: usize) -> Vec<String> {
    (0..4096)
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
fn fixture(text: &[String]) -> Vec<Group> {
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
                                observed_time_unix_nano: START + ((i * 128 + j) as u64),
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
fn seed_store(path: &Path, groups: &[Group], rotate: u64, timed: bool) {
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
            measure("server_submit_to_durable_answer", 128, f);
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
    let a: Vec<_> = std::env::args().collect();
    let root = PathBuf::from(&a[1]);
    let mode = &a[2];
    let size: usize = a[3].parse().unwrap();
    fs::create_dir_all(&root).unwrap();
    let texts = bodies(if mode == "control" || mode == "scheduling" {
        900
    } else {
        size
    });
    let groups = fixture(&texts);
    records(&root, &groups);
    let source = texts.join("\n") + "\n";
    fs::write(root.join("source.log"), source.as_bytes()).unwrap();
    println!(
        "{}",
        json!({"stage":"fixture","records":texts.len(),"source_bytes":source.len(),"source_sha256":hash(source.as_bytes()),"allocator_counted":cfg!(feature="responsibility-alloc-probe"),"size":size})
    );
    match mode.as_str() {
        "collection" => {
            for _ in 0..10 {
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
            for _ in 0..20 {
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
                committed.push(measure("spool_clone_encode_append_sync", 128, || {
                    spool.append(&b).unwrap()
                }));
            }
            for b in &committed {
                let (seq, bytes) = measure("spool_read_decode", 128, || {
                    spool.next_unacked().unwrap().unwrap()
                });
                assert_eq!(bytes, b.encode_to_vec());
                measure("spool_ack_persist_advance", 128, || {
                    spool.record_ack(seq).unwrap()
                });
            }
            drop(spool);
            let mut reopened = measure("spool_open_recovery", 4096, || {
                Spool::open(root.join("spool"), 256 * 1024 * 1024).unwrap()
            });
            let mut n = 0;
            measure("spool_replay_decode", 4096, || {
                reopened
                    .replay(|b| {
                        assert_eq!(b, committed[n]);
                        n += 1;
                        Ok(())
                    })
                    .unwrap()
            });
            assert_eq!(n, 32);
            drop(reopened);
            let state = root.join("state");
            seed_store(&state, &groups, 512 * 1024, true);
            let recovered = measure("server_open_replay", 4096, || recover(&state));
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
            for _ in 0..5 {
                let mut parsed = Rows::default();
                for g in &groups {
                    measure("batch_decode_project_rows", 128, || {
                        rows::extract(g.group_sequence, &g.entries[0], &mut parsed).unwrap()
                    });
                }
                assert_eq!(parsed.logs.len(), 4096);
                measure("sort_log_rows", 4096, || {
                    parsed
                        .logs
                        .sort_by_key(|r| (r.observed_ns, r.node_id, r.sequence, r.index))
                });
                assert_eq!(
                    parsed
                        .logs
                        .iter()
                        .map(|r| r.body.clone())
                        .collect::<Vec<_>>(),
                    texts
                );
            }
            let state = root.join("state");
            measure("segment_build_inclusive_publish", 4096, || {
                segment::build(&state, 1, &groups).unwrap()
            });
        }
        "query" => {
            for kind in ["tail", "segment"] {
                let state = root.join(kind);
                if kind == "tail" {
                    seed_store(&state, &groups, 128 * 1024 * 1024, false);
                } else {
                    segment::build(&state, 1, &groups).unwrap();
                    fs::create_dir_all(state.join("journal")).unwrap();
                }
                let actual = if kind == "tail" {
                    recover(&state)
                } else {
                    groups.clone()
                };
                let record_dir = root.join(format!("{kind}-records"));
                fs::create_dir(&record_dir).unwrap();
                records(&record_dir, &actual);
                for plan in [Plan::Scan, Plan::Walk] {
                    let h = History::with_plan(&state, plan);
                    let name = if plan == Plan::Scan { "scan" } else { "walk" };
                    for (shape, q) in [
                        (
                            "empty",
                            json!({"kind":"logs","from_ns":1,"to_ns":2,"limit":10000}),
                        ),
                        (
                            "selective",
                            json!({"kind":"logs","from_ns":START,"to_ns":START+10000,"contains":"bench-0007 ","limit":10000}),
                        ),
                        (
                            "common",
                            json!({"kind":"logs","from_ns":START,"to_ns":START+10000,"contains":"RRRR","limit":10000}),
                        ),
                        (
                            "broad",
                            json!({"kind":"logs","from_ns":START,"to_ns":START+10000,"limit":10000}),
                        ),
                    ] {
                        let query: Query = serde_json::from_value(q.clone()).unwrap();
                        for i in 0..12 {
                            let answer =
                                measure(&format!("query_{kind}_{name}_{shape}"), 1, || {
                                    h.run(&query, 32).unwrap()
                                });
                            let bytes = measure(
                                "answer_json_serialize",
                                answer["rows"].as_array().unwrap().len(),
                                || serde_json::to_vec(&answer).unwrap(),
                            );
                            if i == 0 {
                                fs::write(
                                    root.join(format!("answer-{kind}-{name}-{shape}.json")),
                                    serde_json::to_vec(&json!({"query":q,"answer":answer}))
                                        .unwrap(),
                                )
                                .unwrap();
                            }
                            std::hint::black_box(bytes);
                        }
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
            for j in 0..10 {
                let r = measure("control_update_persist", 1, || {
                    c.set_config("node000", cfg.clone()).unwrap()
                });
                assert_eq!(r.revision, j + 2);
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
            assert_eq!(c.inventory()[0].0.revision, 11);
        }
        "scheduling" => {
            let state = root.join("state");
            seed_store(&state, &groups, 512 * 1024, false);
            let store =
                Store::open_with(&state, 256 * 1024 * 1024, 512 * 1024, CommitMode::GROUPED)
                    .unwrap();
            let mut before = Vec::new();
            Store::replay(&state, 256 * 1024 * 1024, |e| {
                before.push(e.batch.to_vec());
                Ok(())
            })
            .unwrap();
            let (intake, thread) = store.spawn_joinable().unwrap();
            measure("native_sealer_pass_inclusive", 4096, || {
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
            for _ in 0..100 {
                let bytes = &groups[0].entries[0].batch;
                measure("observer_sha256", bytes.len(), || hash(bytes));
                measure("observer_hex_format", bytes.len(), || hex(bytes));
            }
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
                let (seq, bytes) = measure("delivery_spool_read_decode", 128, || {
                    spool.next_unacked().unwrap().unwrap()
                });
                assert_eq!(&bytes, b);
                let answer = measure("native_tls_send_server_answer", 128, || sender.send(&bytes));
                assert_eq!(answer, fabric_o11y::spindle::sender::Delivery::Ack(seq));
                measure("delivery_ack_persist", 128, || {
                    spool.record_ack(seq).unwrap()
                });
                println!(
                    "{}",
                    json!({"stage":"acked_hash","sequence":i+1,"sha256":hash(b)})
                );
            }
        }
        _ => panic!("unknown mode"),
    }
    println!("{}", json!({"stage":"complete","mode":mode}));
}
