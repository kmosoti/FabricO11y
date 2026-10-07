//! Additional storage properties, with prebuild ledgers and owned fault state.
use fabric_frame::envelope::Batch;
use fabric_frame::frame::FrameLog;
use fabric_server::{
    query::{History, Plan, Query, QueryError},
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
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
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
        fs::write(path.join("origin.json"),json!({"fixture":origin,"seed":7,"contract":"HIST retention/completeness and bounded-sealer recovery"}).to_string()).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            if let Ok(root) = std::env::var("FABRIC_STORAGE_EVIDENCE") {
                let out = PathBuf::from(root).join(self.0.file_name().unwrap());
                fs::create_dir_all(&out).unwrap();
                for entry in fs::read_dir(&self.0).unwrap() {
                    let path = entry.unwrap().path();
                    if path.is_file() && path.extension().is_some_and(|e| e != "so") {
                        fs::copy(&path, out.join(path.file_name().unwrap())).unwrap();
                    }
                    if path.is_dir()
                        && path
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with("oracle-")
                    {
                        let destination = out.join(path.file_name().unwrap());
                        fs::create_dir(&destination).unwrap();
                        for artifact in fs::read_dir(&path).unwrap() {
                            let artifact = artifact.unwrap().path();
                            assert!(
                                artifact.is_file() && !artifact.is_symlink(),
                                "oracle artifact must be a regular file"
                            );
                            fs::copy(&artifact, destination.join(artifact.file_name().unwrap()))
                                .unwrap();
                        }
                    }
                }
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

fn fixture(first: u64, count: u64, received: u64) -> Vec<Group> {
    (first..first + count)
        .map(|sequence| {
            let mut batch = payload(
                1_000_000 + sequence * 100,
                6,
                sequence as i64 * 10,
                1_000_000,
            );
            batch.node_id = vec![7; 16];
            batch.generation = 1;
            batch.sequence = sequence;
            batch.validate().unwrap();
            Group {
                group_sequence: sequence,
                entries: vec![Entry {
                    label: "node-a".into(),
                    batch: batch.encode_to_vec(),
                    received_unix_nano: received + sequence,
                }],
            }
        })
        .collect()
}

fn ledger(groups: &[Group]) -> String {
    let mut text = String::new();
    for group in groups {
        for e in &group.entries {
            text.push_str(
                &json!({"label":e.label,"received_ns":e.received_unix_nano,"bytes":b64(&e.batch)})
                    .to_string(),
            );
            text.push('\n');
        }
    }
    text
}

fn grade(scratch: &Scratch, groups: &[Group], query: &Value, pages: &[Value], expected: bool) {
    grade_excluding(scratch, groups, query, pages, expected, None);
}

fn grade_excluding(
    scratch: &Scratch,
    groups: &[Group],
    query: &Value,
    pages: &[Value],
    expected: bool,
    unavailable: Option<Value>,
) {
    grade_with_model(
        scratch,
        groups,
        query,
        pages,
        expected,
        "query_oracle.py",
        unavailable.map(|value| ("--unavailable", value)),
    );
}

fn grade_projection(
    scratch: &Scratch,
    groups: &[Group],
    query: &Value,
    pages: &[Value],
    raw_available: bool,
    projection: Option<&str>,
) {
    let missing = projection
        .map(|name| json!([{"projection":name,"from_ns":2_000_001,"to_ns":2_000_009}]))
        .unwrap_or_else(|| json!([]));
    grade_with_model(
        scratch,
        groups,
        query,
        pages,
        true,
        "projection_availability_oracle.py",
        Some((
            "--availability",
            json!({"raw_available":raw_available,"missing_projections":missing}),
        )),
    );
}

fn grade_with_model(
    scratch: &Scratch,
    groups: &[Group],
    query: &Value,
    pages: &[Value],
    expected: bool,
    oracle: &str,
    declaration: Option<(&str, Value)>,
) {
    // Numbered calls retain every query/page/declaration/verdict across table cuts.
    let numbered = scratch.0.join(format!(
        "oracle-{:04}",
        fs::read_dir(&scratch.0)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("oracle-") && e.path().is_dir())
            .count()
    ));
    fs::create_dir(&numbered).unwrap();
    fs::write(numbered.join("ledger.jsonl"), ledger(groups)).unwrap();
    fs::write(numbered.join("query.json"), query.to_string()).unwrap();
    fs::write(numbered.join("pages.json"), json!(pages).to_string()).unwrap();
    let mut command = Command::new("python3");
    command
        .arg("-B")
        .arg(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../tools/qualification/{oracle}")),
        )
        .arg("--records")
        .arg(numbered.join("ledger.jsonl"))
        .arg("--query")
        .arg(numbered.join("query.json"))
        .arg("--answer")
        .arg(numbered.join("pages.json"));
    if let Some((flag, value)) = declaration {
        let path = numbered.join(format!("{}.json", flag.trim_start_matches("--")));
        fs::write(&path, value.to_string()).unwrap();
        command.arg(flag).arg(path);
    }
    let result = command.output().unwrap();
    fs::write(numbered.join("oracle.stdout.json"), &result.stdout).unwrap();
    fs::write(numbered.join("oracle.stderr.txt"), &result.stderr).unwrap();
    let verdict: Value = serde_json::from_slice(&result.stdout).expect("oracle JSON");
    {
        use std::io::Write;
        let mut receipts = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(scratch.0.join("oracle-receipts.jsonl"))
            .unwrap();
        writeln!(receipts, "{}", json!({"oracle":oracle,"query":query,"pages":pages.len(),"expected_pass":expected,"exit":result.status.code(),"verdict":verdict})).unwrap();
    }
    assert_eq!(
        result.status.code(),
        Some(if expected { 0 } else { 1 }),
        "{verdict}"
    );
    assert_eq!(verdict["passed"], expected, "{verdict}");
}

#[test]
fn missing_query_tables_are_incomplete_with_independently_declared_unavailability() {
    let s = Scratch::new("deleted logs metrics spans tables, query-specific unavailability");
    let state = s.0.join("state");
    empty_journal(&state);
    let groups = fixture(1, 8, 2_000_000);
    segment::build(&state, 1, &groups).unwrap();
    let dir = state.join("segments").join(segment::segment_name(1));
    segment::verify(&dir, &segment::read_manifest(&dir).unwrap()).unwrap();
    for (name, q) in [
        (
            "logs.parquet",
            json!({"kind":"logs","from_ns":0,"to_ns":u64::MAX/2,"limit":3}),
        ),
        (
            "metrics.parquet",
            json!({"kind":"metrics","name":"system.network.receive.bytes","from_ns":0,"to_ns":u64::MAX/2,"limit":3}),
        ),
        (
            segment::SPANS,
            json!({"kind":"spans","from_ns":0,"to_ns":u64::MAX/2,"limit":3}),
        ),
    ] {
        let original = fs::read(dir.join(name)).unwrap();
        fs::remove_file(dir.join(name)).unwrap();
        for plan in [Plan::Scan, Plan::Walk] {
            let chain = pages(&History::with_plan(&state, plan), q.clone(), 8);
            assert_eq!(chain[0]["complete"], false);
            let projection = q["kind"].as_str().unwrap();
            grade_projection(&s, &groups, &q, &chain, true, Some(projection));
        }
        fs::write(dir.join(name), original).unwrap();
    }
}

#[test]
fn missing_raw_batches_preserve_projection_rows_but_mark_incomplete() {
    let s =
        Scratch::new("raw batches table deleted; verified projections metadata and gaps survive");
    let state = s.0.join("state");
    empty_journal(&state);
    let groups = fixture(1, 8, 2_000_000);
    segment::build(&state, 1, &groups).unwrap();
    let dir = state.join("segments").join(segment::segment_name(1));
    // Verify intact pre-cut custody; expected rows/envelope come from producer bytes.
    segment::verify(&dir, &segment::read_manifest(&dir).unwrap()).unwrap();
    fs::remove_file(dir.join("batches.parquet")).unwrap();
    for plan in [Plan::Scan, Plan::Walk] {
        for q in [
            json!({"kind":"logs","from_ns":0,"to_ns":u64::MAX/2,"limit":3}),
            json!({"kind":"metrics","name":"system.network.receive.bytes","from_ns":0,"to_ns":u64::MAX/2,"limit":3}),
            json!({"kind":"spans","from_ns":0,"to_ns":u64::MAX/2,"limit":3}),
        ] {
            let chain = pages(&History::with_plan(&state, plan), q.clone(), 32);
            grade_projection(&s, &groups, &q, &chain, false, None);
        }
    }
}

#[test]
fn raw_batch_table_integrity_failures_are_incomplete() {
    for defect in ["truncated", "footer", "schema", "manifest_rows"] {
        let s = Scratch::new(&format!(
            "raw integrity {defect}; intact projection metadata and gaps"
        ));
        let state = s.0.join("state");
        empty_journal(&state);
        let groups = fixture(1, 8, 2_000_000);
        segment::build(&state, 1, &groups).unwrap();
        let dir = state.join("segments").join(segment::segment_name(1));
        let mut manifest = segment::read_manifest(&dir).unwrap();
        segment::verify(&dir, &manifest).unwrap();
        let raw = dir.join("batches.parquet");
        let original = fs::read(&raw).unwrap();
        match defect {
            "truncated" => {
                fs::write(&raw, &original[..original.len() - 1]).unwrap();
            }
            "footer" => {
                let mut broken = original.clone();
                let len = broken.len();
                broken[len - 4..].copy_from_slice(b"FAIL");
                fs::write(&raw, &broken).unwrap();
                assert_eq!(
                    fs::metadata(&raw).unwrap().len(),
                    manifest.files["batches.parquet"].bytes
                );
            }
            "schema" => {
                fs::copy(dir.join("metrics.parquet"), &raw).unwrap();
                // Make size/hash/rows describe the actual replacement: schema alone is wrong.
                let replacement = manifest.files["metrics.parquet"].clone();
                manifest.files.insert("batches.parquet".into(), replacement);
                fs::write(
                    dir.join("manifest.json"),
                    serde_json::to_vec(&manifest).unwrap(),
                )
                .unwrap();
            }
            "manifest_rows" => {
                manifest.files.get_mut("batches.parquet").unwrap().rows += 1;
                fs::write(
                    dir.join("manifest.json"),
                    serde_json::to_vec(&manifest).unwrap(),
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        fs::write(s.0.join("raw-integrity-cut.json"), json!({"defect":defect,"raw_before_bytes":original.len(),"raw_after_bytes":fs::metadata(&raw).unwrap().len()}).to_string()).unwrap();
        for plan in [Plan::Scan, Plan::Walk] {
            for q in [
                json!({"kind":"logs","from_ns":0,"to_ns":u64::MAX/2,"limit":3}),
                json!({"kind":"metrics","name":"system.network.receive.bytes","from_ns":0,"to_ns":u64::MAX/2,"limit":3}),
                json!({"kind":"spans","from_ns":0,"to_ns":u64::MAX/2,"limit":3}),
            ] {
                let chain = pages(&History::with_plan(&state, plan), q.clone(), 32);
                grade_projection(&s, &groups, &q, &chain, false, None);
            }
        }
    }
}

#[test]
fn missing_gap_table_is_incomplete_and_missing_manifest_is_source_error() {
    for defect in ["gaps", "manifest"] {
        let s = Scratch::new(&format!(
            "missing {defect}; unambiguous source availability control"
        ));
        let state = s.0.join("state");
        empty_journal(&state);
        let groups = fixture(1, 8, 2_000_000);
        segment::build(&state, 1, &groups).unwrap();
        let dir = state.join("segments").join(segment::segment_name(1));
        segment::verify(&dir, &segment::read_manifest(&dir).unwrap()).unwrap();
        fs::remove_file(dir.join(if defect == "gaps" {
            "gaps.parquet"
        } else {
            "manifest.json"
        }))
        .unwrap();
        let mut observations = Vec::new();
        for plan in [Plan::Scan, Plan::Walk] {
            let q = json!({"kind":"logs","from_ns":0,"to_ns":u64::MAX/2,"limit":3});
            let history = History::with_plan(&state, plan);
            if defect == "gaps" {
                let chain = pages(&history, q, 32);
                for page in &chain {
                    assert_eq!(page["complete"], false);
                    assert!(!page["unavailable"].as_array().unwrap().is_empty());
                }
                observations.push(json!({"plan":format!("{plan:?}"),"pages":chain}));
                // No companion grade: gaps evidence is outside its intact-gaps scope.
            } else {
                let parsed: Query = serde_json::from_value(q).unwrap();
                match history.run(&parsed, 32) {
                    Err(QueryError::Io(error)) => {
                        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
                        observations.push(json!({"plan":format!("{plan:?}"),"error_kind":"Interrupted","error":error.to_string()}));
                    }
                    other => panic!("missing Manifest must remain a source error, got {other:?}"),
                }
            }
        }
        fs::write(
            s.0.join("source-error-controls.json"),
            json!({"defect":defect,"observations":observations}).to_string(),
        )
        .unwrap();
    }
}

#[test]
fn span_table_and_filter_write_and_sync_faults_keep_pending_custody() {
    let s = Scratch::new("owned span table/filter ENOSPC writes and sync");
    let lib = s.0.join("fault.so");
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/bench/labs/completion/io_fault.c");
    assert!(
        Command::new("gcc")
            .args(["-shared", "-fPIC", "-O2", "-o"])
            .arg(&lib)
            .arg(source)
            .arg("-ldl")
            .status()
            .unwrap()
            .success()
    );
    for (index, (file, op)) in [
        (segment::SPANS, "write"),
        (segment::SPANS, "sync"),
        (segment::SPANS_FILTER, "write"),
        (segment::SPANS_FILTER, "sync"),
    ]
    .into_iter()
    .enumerate()
    {
        let state = s.0.join(format!("state-{index}"));
        let groups = fixture(1, 8, 2_000_000);
        let input = input_journal(&state, &groups);
        let original = fs::read(&input).unwrap();
        let err = s.0.join(format!("child-{index}.err"));
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "completion_storage_child",
                "--nocapture",
            ])
            .env("FABRIC_STORAGE_CHILD_STATE", &state)
            .env("LD_PRELOAD", &lib)
            .env("FABRIC_FAULT_ROOT", &state)
            .env("FABRIC_FAULT_MATCH", file)
            .env("FABRIC_FAULT_OP", op)
            .stdout(fs::File::create(s.0.join(format!("child-{index}.out"))).unwrap())
            .stderr(fs::File::create(&err).unwrap())
            .spawn()
            .unwrap();
        let began = Instant::now();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(!status.success());
                break;
            }
            if began.elapsed() > Duration::from_secs(30) {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("fault was not reached");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            fs::read_to_string(err)
                .unwrap()
                .contains("FABRIC_INJECTION")
        );
        assert_eq!(fs::read(&input).unwrap(), original);
        assert!(segment::labels(&state).unwrap().is_empty());
        assert!(
            !state
                .join("segments/.building-00000000000000000001")
                .exists()
        );
        verify(&s, &state, &groups);
        segment::cleanup(&state).unwrap();
        segment::build_sealed(&state, 1, &input).unwrap();
        verify(&s, &state, &groups);
    }
}

fn pages(history: &History, mut query: Value, newest: u64) -> Vec<Value> {
    let mut chain = Vec::new();
    for _ in 0..100 {
        let parsed: Query = serde_json::from_value(query.clone()).unwrap();
        let answer = history.run(&parsed, newest).unwrap();
        let next = answer["next_page"].clone();
        chain.push(answer);
        if next.is_null() {
            return chain;
        }
        query["page"] = next;
    }
    panic!("pagination did not terminate");
}

fn queries() -> Vec<Value> {
    vec![
        json!({"kind":"logs","from_ns":0,"to_ns":u64::MAX/2,"limit":3}),
        json!({"kind":"logs","contains":"needle","from_ns":0,"to_ns":u64::MAX/2,"limit":3}),
        json!({"kind":"spans","trace_id":"00112233445566778899aabbccddeeff","from_ns":0,"to_ns":u64::MAX/2,"limit":2}),
        json!({"kind":"spans","from_ns":0,"to_ns":u64::MAX/2,"limit":2}),
        json!({"kind":"rate","name":"system.network.receive.bytes","from_ns":0,"to_ns":u64::MAX/2}),
    ]
}

#[test]
fn full_chains_stay_exact_during_a_builder_paused_before_publication() {
    let s = Scratch::new("Q1 actual in-progress builder with manifest-sync barrier");
    let state = s.0.join("state");
    let groups = fixture(1, 8, 2_000_000);
    input_journal(&state, &groups);
    fs::write(s.0.join("expected-before-build.jsonl"), ledger(&groups)).unwrap();
    let lib = s.0.join("fault.so");
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/bench/labs/completion/io_fault.c");
    assert!(
        Command::new("gcc")
            .args(["-shared", "-fPIC", "-O2", "-o"])
            .arg(&lib)
            .arg(source)
            .arg("-ldl")
            .status()
            .unwrap()
            .success()
    );
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "completion_storage_child",
            "--nocapture",
        ])
        .env("FABRIC_STORAGE_CHILD_STATE", &state)
        .env("LD_PRELOAD", &lib)
        .env("FABRIC_FAULT_ROOT", &state)
        .env("FABRIC_FAULT_OP", "sync")
        .env("FABRIC_FAULT_MATCH", "manifest.json")
        .env("FABRIC_FAULT_PAUSE", "1")
        .stdout(fs::File::create(s.0.join("child.out")).unwrap())
        .stderr(fs::File::create(s.0.join("child.err")).unwrap())
        .spawn()
        .unwrap();
    let began = Instant::now();
    loop {
        let status = fs::read_to_string(format!("/proc/{}/status", child.id())).unwrap();
        if status
            .lines()
            .any(|line| line.starts_with("State:") && line.contains("T (stopped)"))
        {
            break;
        }
        if child.try_wait().unwrap().is_some() || began.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("in-progress build cut not reached");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        fs::read_to_string(s.0.join("child.err"))
            .unwrap()
            .contains("FABRIC_INJECTION")
    );
    assert!(segment::labels(&state).unwrap().is_empty());
    verify(&s, &state, &groups);
    assert!(
        Command::new("kill")
            .args(["-CONT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    while !state.join("published-ready").exists() {
        if began.elapsed() > Duration::from_secs(60) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("publication timeout");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    verify(&s, &state, &groups);
}

fn verify(scratch: &Scratch, state: &Path, groups: &[Group]) {
    let newest = groups.iter().map(|g| g.group_sequence).max().unwrap();
    for plan in [Plan::Scan, Plan::Walk] {
        let history = History::with_plan(state, plan);
        for q in queries() {
            grade(
                scratch,
                groups,
                &q,
                &pages(&history, q.clone(), newest),
                true,
            );
        }
    }
}

fn empty_journal(state: &Path) {
    fs::create_dir_all(state.join("journal")).unwrap();
    drop(
        FrameLog::open(
            &state.join("journal"),
            1 << 28,
            segment::MAX_GROUP_PAYLOAD,
            |_, _| Ok(()),
        )
        .unwrap(),
    );
}

fn retain(state: &Path, age: u64) {
    let (intake, thread) = Store::open(state, 1 << 28, CommitMode::GROUPED)
        .unwrap()
        .spawn_joinable()
        .unwrap();
    sealer::pass(
        state,
        &intake,
        Retention {
            max_age_s: age,
            max_bytes: 1 << 40,
        },
        2,
    )
    .unwrap();
    drop(intake);
    thread.join().unwrap();
}

#[test]
fn age_eviction_grades_the_predeclared_retained_ledger_and_expires_pages() {
    let s = Scratch::new("age retention + snapshot floor");
    let state = s.0.join("state");
    empty_journal(&state);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let old = fixture(1, 4, 1_000_000);
    let retained = fixture(5, 4, now);
    segment::build(&state, 1, &old).unwrap();
    segment::build(&state, 5, &retained).unwrap();
    let all: [Vec<Group>; 2] = [old, retained.clone()];
    let before: Vec<_> = all.into_iter().flatten().collect();
    let q = json!({"kind":"logs","from_ns":0,"to_ns":u64::MAX/2,"limit":2});
    let mut saved = Vec::new();
    for plan in [Plan::Scan, Plan::Walk] {
        let h = History::with_plan(&state, plan);
        let chain = pages(&h, q.clone(), 8);
        grade(&s, &before, &q, &chain, true);
        assert!(chain.len() > 1);
        saved.push((h, chain[0]["next_page"].clone()));
    }
    retain(&state, 86400);
    assert_eq!(segment::labels(&state).unwrap(), vec![5]);
    for (h, token) in saved {
        let mut next = q.clone();
        next["page"] = token;
        assert!(matches!(
            h.run(&serde_json::from_value(next).unwrap(), 8),
            Err(QueryError::Gone)
        ));
        let chain = pages(&h, q.clone(), 8);
        grade(&s, &retained, &q, &chain, true);
    }
    verify(&s, &state, &retained);
    let mut bad = pages(&History::new(&state), q.clone(), 8);
    bad[0]["rows"].as_array_mut().unwrap().pop();
    grade(&s, &retained, &q, &bad, false);
}

#[test]
fn absent_and_unauthentic_optional_log_and_span_filters_preserve_exact_answers() {
    let s = Scratch::new("optional filters fallback");
    let state = s.0.join("state");
    empty_journal(&state);
    let groups = fixture(1, 8, 2_000_000);
    segment::build(&state, 1, &groups).unwrap();
    let dir = state.join("segments").join(segment::segment_name(1));
    for name in [fabric_server::text_filter::FILE, segment::SPANS_FILTER] {
        let path = dir.join(name);
        let original = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        verify(&s, &state, &groups);
        fs::write(&path, b"unauthentic optional filter").unwrap();
        verify(&s, &state, &groups);
        fs::write(&path, original).unwrap();
    }
}

#[test]
fn missing_raw_batch_table_reports_replay_failure_without_fabricating_projection_loss() {
    let s = Scratch::new("missing raw custody table");
    let state = s.0.join("state");
    empty_journal(&state);
    let groups = fixture(1, 8, 2_000_000);
    segment::build(&state, 1, &groups).unwrap();
    fs::remove_file(
        state
            .join("segments")
            .join(segment::segment_name(1))
            .join("batches.parquet"),
    )
    .unwrap();
    assert!(
        Store::replay(&state, 1 << 28, |_| Ok(())).is_err(),
        "raw custody replay must surface loss"
    );
    // Raw custody is unavailable while the original projection query set survives.
    // Use the already registered companion; retain all five queries in both plans.
    for plan in [Plan::Scan, Plan::Walk] {
        let history = History::with_plan(&state, plan);
        for query in queries() {
            let chain = pages(&history, query.clone(), 8);
            grade_projection(&s, &groups, &query, &chain, false, None);
        }
    }
}

fn input_journal(state: &Path, groups: &[Group]) -> PathBuf {
    fs::create_dir_all(state.join("journal")).unwrap();
    let journal = state.join("journal");
    let mut log =
        FrameLog::open(&journal, 1 << 28, segment::MAX_GROUP_PAYLOAD, |_, _| Ok(())).unwrap();
    for group in groups {
        log.append(&group.encode_to_vec()).unwrap();
    }
    log.rotate(1).unwrap();
    drop(log);
    journal.join("sealed-00000000000000000001.faj")
}

#[test]
fn scoped_input_and_output_path_failures_keep_custody_and_retry_exactly() {
    let s = Scratch::new("missing read input + blocked output path");
    let state = s.0.join("state");
    let groups = fixture(1, 8, 2_000_000);
    let input = input_journal(&state, &groups);
    let original = fs::read(&input).unwrap();
    assert!(segment::build_sealed(&state, 1, &s.0.join("absent-input")).is_err());
    assert!(
        !state
            .join("segments/.building-00000000000000000001")
            .exists(),
        "failed read leaked build state"
    );
    let blocked = s.0.join("blocked-state");
    fs::write(&blocked, b"owned output obstruction").unwrap();
    assert!(segment::build_sealed(&blocked, 1, &input).is_err());
    assert_eq!(fs::read(&input).unwrap(), original);
    assert!(segment::labels(&state).unwrap().is_empty());
    fs::remove_file(&blocked).unwrap();
    segment::build_sealed(&state, 1, &input).unwrap();
    assert!(
        !state
            .join("segments/.building-00000000000000000001")
            .exists()
    );
    verify(&s, &state, &groups);
}

#[test]
#[ignore = "owned subprocess helper, invoked only by kill fixture"]
fn completion_storage_child() {
    let state = PathBuf::from(
        std::env::var("FABRIC_STORAGE_CHILD_STATE").expect("owned helper environment required"),
    );
    segment::build_sealed(
        &state,
        1,
        &state.join("journal/sealed-00000000000000000001.faj"),
    )
    .unwrap();
    fs::write(
        state.join("published-ready"),
        b"bounded builder returned after publication",
    )
    .unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[test]
fn an_actual_owned_child_killed_after_publication_reclaims_and_replays_exactly() {
    let s = Scratch::new("owned process kill after publication before reclaim");
    let state = s.0.join("state");
    let groups = fixture(1, 8, 2_000_000);
    let input = input_journal(&state, &groups);
    fs::write(s.0.join("prekill-ledger.jsonl"), ledger(&groups)).unwrap();
    let out = fs::File::create(s.0.join("child.out")).unwrap();
    let err = fs::File::create(s.0.join("child.err")).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "completion_storage_child",
            "--nocapture",
        ])
        .env("FABRIC_STORAGE_CHILD_STATE", &state)
        .stdout(out)
        .stderr(err)
        .spawn()
        .unwrap();
    let began = Instant::now();
    while !state.join("published-ready").exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("builder child exited before cut: {status}");
        }
        if began.elapsed() > Duration::from_secs(30) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("publication cut not reached");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(input.exists(), "publication barrier precedes reclaim");
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    verify(&s, &state, &groups);
    retain(&state, u64::MAX);
    assert!(
        !input.exists(),
        "restart pass must reclaim published journal"
    );
    let mut recovered = String::new();
    Store::replay(&state,1<<28,|entry| {
  recovered.push_str(&json!({"label":entry.label,"received_ns":entry.received_unix_nano,"bytes":b64(&entry.batch)}).to_string());recovered.push('\n');Ok(())
 }).unwrap();
    assert_eq!(
        recovered,
        ledger(&groups),
        "custody identities/bytes exactly survive actual child kill"
    );
    verify(&s, &state, &groups);
}
