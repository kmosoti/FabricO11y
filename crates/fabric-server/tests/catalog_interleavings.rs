//! Owned stopped-process publication/checkpoint cuts; shared-reader retention.
use fabric_frame::envelope::Batch;
use fabric_frame::frame::FrameLog;
use fabric_server::{
    query::{History, Plan, QueryError},
    sealer::{self, Retention},
    segment,
    store::{CommitMode, Entry, Group, Store},
};
use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
    trace::v1::ExportTraceServiceRequest,
};
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum, metric, number_data_point,
};
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, Status};
use prost::Message;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static CALL: AtomicU64 = AtomicU64::new(0);
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new(origin: &str) -> Self {
        let base = PathBuf::from(
            std::env::var("FABRIC_SCRATCH_ROOT").expect("resource launcher scratch required"),
        );
        let path = base.join(format!(
            "storage-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("origin.json"),json!({"fixture":origin,"seed":"0xCA7A10A1","contract":"HIST retention/completeness and bounded-sealer recovery"}).to_string()).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            if let Ok(root) = std::env::var("FABRIC_STORAGE_EVIDENCE") {
                let out = PathBuf::from(root).join(self.0.file_name().unwrap());
                fs::create_dir_all(&out).unwrap();
                copy_evidence(&self.0, &out);
            }
            fs::remove_dir_all(&self.0).unwrap();
        } else {
            eprintln!("failure fixture retained: {}", self.0.display());
        }
    }
}
fn kv(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.into())),
        }),
        ..Default::default()
    }
}

/// A batch with `n` log lines at `t0 + i` and one monotonic counter point.
fn payload(t0: u64, n: u64, counter: i64, start: u64) -> Batch {
    let logs = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: (0..n)
                    .map(|i| LogRecord {
                        observed_time_unix_nano: t0 + i,
                        body: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(format!(
                                "line {t0}+{i} {}",
                                if i % 3 == 0 { "needle" } else { "hay" }
                            ))),
                        }),
                        attributes: vec![kv("log.file.path", "/var/log/app.log")],
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let metrics = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![Metric {
                    name: "system.network.receive.bytes".into(),
                    unit: "By".into(),
                    data: Some(metric::Data::Sum(Sum {
                        data_points: vec![NumberDataPoint {
                            attributes: vec![kv("interface", "eth0")],
                            start_time_unix_nano: start,
                            time_unix_nano: t0,
                            value: Some(number_data_point::Value::AsInt(counter)),
                            ..Default::default()
                        }],
                        aggregation_temporality: 2,
                        is_monotonic: true,
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    // Three spans per batch: a root and two children, two batches per trace, kinds
    // and statuses varying, and every eleventh batch a span whose kind is outside the
    // OTLP enum (kept as its integer; a tail block cannot carry it).
    let k = (t0 - 1_000_000) / 100;
    let trace_id: Vec<u8> = (0..16).map(|i| (k / 2) as u8 ^ (i * 17)).collect();
    let span_id = |i: u64| (k * 10 + i + 1).to_be_bytes().to_vec();
    let traces = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: (0..3)
                    .map(|i| Span {
                        trace_id: trace_id.clone(),
                        span_id: span_id(i),
                        parent_span_id: if i == 0 { vec![] } else { span_id(0) },
                        name: if i == 0 {
                            "GET /items".into()
                        } else {
                            format!("db.query.{i}")
                        },
                        kind: if k.is_multiple_of(11) && i == 2 {
                            9
                        } else {
                            1 + i as i32
                        },
                        start_time_unix_nano: t0 + 3 * i,
                        end_time_unix_nano: t0 + 3 * i + 5,
                        attributes: vec![kv("http.route", "/items")],
                        status: (i == 1).then(|| Status {
                            code: (k % 3) as i32,
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    Batch {
        version: 1,
        node_id: vec![],
        generation: 0,
        sequence: 0,
        metrics: metrics.encode_to_vec(),
        logs: logs.encode_to_vec(),
        traces: traces.encode_to_vec(),
        cursors: vec![],
        collection_gaps: if t0.is_multiple_of(7) {
            vec![format!("gap at {t0}")]
        } else {
            vec![]
        },
    }
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |a, (i, b)| a | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                T[((n >> (18 - 6 * i)) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

fn copy_evidence(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        // Retain ledgers and every grader input/output, not redundant stores.
        if path.file_name().is_some_and(|n| n == "state") {
            continue;
        }
        let dest = to.join(path.file_name().unwrap());
        if path.is_dir() {
            fs::create_dir_all(&dest).unwrap();
            copy_evidence(&path, &dest);
        } else {
            fs::copy(&path, &dest).unwrap();
        }
    }
}

fn fixture(received: u64) -> Vec<Group> {
    // Eight committed groups, each with one Batch from each of three nodes.
    // Observation times deliberately decrease; node order opposes sort order.
    (1..=8)
        .map(|sequence| Group {
            group_sequence: sequence,
            entries: (1..=3)
                .rev()
                .map(|node| {
                    let t = 1_000_000 + (0xCA7A10A1u64 % 17) * 100 + (8 - sequence) * 100;
                    let mut batch = payload(t, 6, sequence as i64 * 10, 1_000_000);
                    batch.node_id = vec![node; 16];
                    batch.generation = 1;
                    batch.sequence = sequence;
                    batch.validate().unwrap();
                    Entry {
                        label: format!("node-{node}"),
                        batch: batch.encode_to_vec(),
                        received_unix_nano: received + sequence,
                    }
                })
                .collect(),
        })
        .collect()
}

fn ledger(groups: &[Group]) -> String {
    groups
        .iter()
        .flat_map(|g| &g.entries)
        .map(|e| {
            format!(
                "{}\n",
                json!({"label":e.label,"received_ns":e.received_unix_nano,"bytes":b64(&e.batch)})
            )
        })
        .collect()
}

fn grade(
    s: &Scratch,
    groups: &[Group],
    q: &Value,
    answers: &[Value],
    stage: &str,
    plan: &str,
    pass: bool,
) {
    let call = s.0.join(format!(
        "oracle-{:04}",
        CALL.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&call).unwrap();
    fs::write(call.join("ledger.jsonl"), ledger(groups)).unwrap();
    fs::write(call.join("query.json"), q.to_string()).unwrap();
    fs::write(call.join("pages.json"), json!(answers).to_string()).unwrap();
    fs::write(
        call.join("context.json"),
        json!({"seed":"0xCA7A10A1","stage":stage,"plan":plan,"expected_pass":pass}).to_string(),
    )
    .unwrap();
    let result = Command::new("python3")
        .arg("-B")
        .arg(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../tools/qualification/query_oracle.py"),
        )
        .arg("--records")
        .arg(call.join("ledger.jsonl"))
        .arg("--query")
        .arg(call.join("query.json"))
        .arg("--answer")
        .arg(call.join("pages.json"))
        .output()
        .unwrap();
    fs::write(call.join("stdout.json"), &result.stdout).unwrap();
    fs::write(call.join("stderr.txt"), &result.stderr).unwrap();
    fs::write(
        call.join("exit.json"),
        json!({"exit":result.status.code()}).to_string(),
    )
    .unwrap();
    let verdict: Value = serde_json::from_slice(&result.stdout).expect("oracle verdict");
    assert_eq!(
        result.status.code(),
        Some(if pass { 0 } else { 1 }),
        "{verdict}"
    );
    assert_eq!(verdict["passed"], pass, "{verdict}");
}

fn pages(h: &History, mut q: Value, newest: u64) -> Vec<Value> {
    let mut answers = Vec::new();
    for _ in 0..200 {
        let answer = h
            .run(&serde_json::from_value(q.clone()).unwrap(), newest)
            .unwrap();
        let next = answer["next_page"].clone();
        answers.push(answer);
        if next.is_null() {
            return answers;
        }
        q["page"] = next;
    }
    panic!("pagination did not terminate");
}

fn queries() -> Vec<Value> {
    let mut out = Vec::new();
    for limit in [1, 2, 3] {
        for needle in [None, Some("needle"), Some("absent-zq9")] {
            let mut q = json!({"kind":"logs","from_ns":0,"to_ns":1u64<<40,"limit":limit});
            if let Some(n) = needle {
                q["contains"] = json!(n);
            }
            out.push(q);
        }
        out.push(json!({"kind":"spans","from_ns":0,"to_ns":1u64<<40,"limit":limit}));
        out.push(json!({"kind":"metrics","name":"system.network.receive.bytes","from_ns":0,"to_ns":1u64<<40,"limit":limit}));
    }
    out
}

fn verify(s: &Scratch, state: &Path, groups: &[Group], stage: &str) {
    for (plan, name) in [(Plan::Scan, "scan"), (Plan::Walk, "walk")] {
        let h = History::with_plan(state, plan);
        for q in queries() {
            let answer = pages(&h, q.clone(), groups.last().unwrap().group_sequence);
            grade(s, groups, &q, &answer, stage, name, true);
        }
    }
}

fn journal(state: &Path) -> FrameLog {
    fs::create_dir_all(state.join("journal")).unwrap();
    FrameLog::open(
        &state.join("journal"),
        1 << 28,
        segment::MAX_GROUP_PAYLOAD,
        |_, _| Ok(()),
    )
    .unwrap()
}

fn append(log: &mut FrameLog, groups: &[Group]) {
    for g in groups {
        log.append(&g.encode_to_vec()).unwrap();
    }
}

fn pass(state: &Path, age: u64) {
    let (intake, thread) = Store::open(state, 1 << 28, CommitMode::GROUPED)
        .unwrap()
        .spawn_joinable()
        .unwrap();
    let result = sealer::pass(
        state,
        &intake,
        Retention {
            max_age_s: age,
            max_bytes: 1 << 40,
        },
        1,
    );
    drop(intake);
    thread.join().unwrap();
    result.unwrap();
}

struct OwnedChild(std::process::Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        // SIGKILL also terminates a stopped child; cleanup applies during panic.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl OwnedChild {
    fn stopped(&mut self, trace: &Path, suffix: &str) {
        let begin = std::time::Instant::now();
        loop {
            assert!(
                self.0.try_wait().unwrap().is_none(),
                "child exited before cut"
            );
            let status = fs::read_to_string(format!("/proc/{}/status", self.0.id())).unwrap();
            if status
                .lines()
                .any(|s| s.starts_with("State:") && s.contains("T (stopped)"))
            {
                let receipt = fs::read_to_string(trace).unwrap();
                assert!(receipt.contains("FABRIC_INJECTION op=sync"), "{receipt}");
                assert!(receipt.contains(suffix), "wrong cut: {receipt}");
                return;
            }
            assert!(
                begin.elapsed() < std::time::Duration::from_secs(30),
                "cut not reached"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    fn finish(&mut self) -> i32 {
        assert!(
            Command::new("kill")
                .args(["-CONT", &self.0.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        let begin = std::time::Instant::now();
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "owned child exit {status}");
                return status.code().unwrap();
            }
            assert!(
                begin.elapsed() < std::time::Duration::from_secs(30),
                "child finish timeout"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

fn paused(
    s: &Scratch,
    state: &Path,
    library: &Path,
    action: &str,
    suffix: &str,
) -> (OwnedChild, PathBuf) {
    let trace = s.0.join(format!("{action}.stderr.txt"));
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "paused_catalog_child",
            "--nocapture",
        ])
        .env("FABRIC_CATALOG_CHILD_STATE", state)
        .env("FABRIC_CATALOG_CHILD_ACTION", action)
        .env("LD_PRELOAD", library)
        .env("FABRIC_FAULT_ROOT", state)
        .env("FABRIC_FAULT_OP", "sync")
        .env("FABRIC_FAULT_MATCH", suffix)
        .env("FABRIC_FAULT_PAUSE", "1")
        .stdout(fs::File::create(s.0.join(format!("{action}.stdout.txt"))).unwrap())
        .stderr(fs::File::create(&trace).unwrap())
        .spawn()
        .unwrap();
    (OwnedChild(child), trace)
}

#[test]
#[ignore = "owned helper invoked explicitly by parent fixture"]
fn paused_catalog_child() {
    let state = PathBuf::from(std::env::var("FABRIC_CATALOG_CHILD_STATE").unwrap());
    match std::env::var("FABRIC_CATALOG_CHILD_ACTION")
        .unwrap()
        .as_str()
    {
        "build" => {
            segment::build_sealed(
                &state,
                1,
                &state.join("journal/sealed-00000000000000000001.faj"),
            )
            .unwrap();
        }
        "checkpoint" => pass(&state, u64::MAX),
        other => panic!("unknown owned child action {other}"),
    }
}

fn reader_cuts(s: &Scratch, readers: &[(History, &str)], groups: &[Group], stage: &str) {
    for (h, name) in readers {
        for q in queries() {
            grade(s, groups, &q, &pages(h, q.clone(), 8), stage, name, true);
        }
    }
}

#[test]
fn paused_publication_and_checkpoint_preserve_shared_reader_coverage_and_gone() {
    let s = Scratch::new("O2/O3 selected stopped-process cuts and O4 shared History Gone");
    let state = s.0.join("state");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let mut groups = fixture(1_000_000);
    for g in &mut groups[4..] {
        for e in &mut g.entries {
            e.received_unix_nano = now + g.group_sequence;
        }
    }
    fs::write(s.0.join("expected-before-io.jsonl"), ledger(&groups)).unwrap();
    let mut log = journal(&state);
    append(&mut log, &groups[..4]);
    log.rotate(1).unwrap();
    append(&mut log, &groups[4..]);
    drop(log);
    let readers = [
        (History::with_plan(&state, Plan::Scan), "scan"),
        (History::with_plan(&state, Plan::Walk), "walk-clone"),
        (
            History::with_plan(&state, Plan::Walk).with_shared_catalog(),
            "walk-shared",
        ),
    ];
    readers[2].0.refresh_catalog(8).unwrap();
    verify(&s, &state, &groups, "pending-baseline-fresh");
    reader_cuts(&s, &readers, &groups, "pending-baseline-primed");
    let library = s.0.join("fault.so");
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/bench/labs/completion/io_fault.c");
    assert!(
        Command::new("gcc")
            .args(["-shared", "-fPIC", "-O2", "-o"])
            .arg(&library)
            .arg(source)
            .arg("-ldl")
            .status()
            .unwrap()
            .success()
    );
    let sealed = state.join("journal/sealed-00000000000000000001.faj");
    {
        let (mut child, trace) = paused(&s, &state, &library, "build", "manifest.json$");
        child.stopped(&trace, "manifest.json");
        assert!(segment::labels(&state).unwrap().is_empty());
        assert!(sealed.exists());
        reader_cuts(&s, &readers, &groups, "builder-paused-manifest-sync");
        fs::write(
            s.0.join("build.exit.json"),
            json!({"exit":child.finish()}).to_string(),
        )
        .unwrap();
    }
    assert_eq!(segment::labels(&state).unwrap(), vec![1]);
    assert!(sealed.exists());
    readers[2].0.refresh_catalog(8).unwrap();
    reader_cuts(&s, &readers, &groups, "published-before-checkpoint");
    {
        let (mut child, trace) = paused(&s, &state, &library, "checkpoint", "streams.json.tmp$");
        child.stopped(&trace, "streams.json.tmp");
        assert!(sealed.exists());
        assert!(state.join("streams.json.tmp").exists());
        reader_cuts(&s, &readers, &groups, "store-paused-checkpoint-sync");
        fs::write(
            s.0.join("checkpoint.exit.json"),
            json!({"exit":child.finish()}).to_string(),
        )
        .unwrap();
    }
    assert!(!sealed.exists());
    assert!(state.join("streams.json").exists());
    readers[2].0.refresh_catalog(8).unwrap();
    reader_cuts(&s, &readers, &groups, "after-owned-child-reclaim");
    let mut log = journal(&state);
    log.rotate(5).unwrap();
    drop(log);
    pass(&state, u64::MAX);
    reader_cuts(&s, &readers, &groups, "all-segments-before-retention");
    let q = json!({"kind":"logs","from_ns":0,"to_ns":1u64<<40,"limit":1});
    let tokens: Vec<_> = readers
        .iter()
        .map(|(h, _)| {
            h.run(&serde_json::from_value(q.clone()).unwrap(), 8)
                .unwrap()["next_page"]
                .clone()
        })
        .collect();
    pass(&state, 86400);
    assert_eq!(segment::labels(&state).unwrap(), vec![5]);
    for ((h, name), token) in readers.iter().zip(tokens) {
        let mut next = q.clone();
        next["page"] = token;
        assert!(
            matches!(
                h.run(&serde_json::from_value(next).unwrap(), 8),
                Err(QueryError::Gone)
            ),
            "{name} resurrected old token"
        );
    }
    readers[2].0.refresh_catalog(8).unwrap();
    reader_cuts(&s, &readers, &groups[4..], "retained-shared-reader-live");
}
