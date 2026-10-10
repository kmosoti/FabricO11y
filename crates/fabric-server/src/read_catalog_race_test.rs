//! Deterministic discovery gap reproduction using real journal and Segment IO.
include!("coupled_catalog_tests.rs");
use crate::{
    query::{History, Plan},
    sealer::{self, Retention},
    segment,
    store::{CommitMode, Entry, Group, Store},
};
use fabric_frame::envelope::Batch;
use fabric_frame::frame::FrameLog;
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
};
static CALL: AtomicU64 = AtomicU64::new(0);
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new(origin: &str) -> Self {
        let base = PathBuf::from(
            std::env::var("FABRIC_SCRATCH_ROOT").expect("resource launcher scratch required"),
        );
        // Each module has its own counter, but both execute in one test process.
        // Distinct namespaces preserve parallel fixture isolation (catalog-parallel-fixture-01).
        let path = base.join(format!(
            "catalog-walk-cut-{}-{}",
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

use super::{CatalogRequest, FilterKind, ReadCatalog, Sources};
use fabric_core::query::Window;

fn custody(view: &Sources) -> Vec<String> {
    let mut rows = Vec::new();
    let mut push = |e: &Entry| {
        rows.push(
            json!({"label":e.label,"received_ns":e.received_unix_nano,"bytes":b64(&e.batch)})
                .to_string(),
        )
    };
    for (dir, manifest) in &view.segments {
        segment::scan_batches(dir, manifest, |_, e| push(&e)).unwrap();
    }
    for group in &view.journal {
        for entry in &group.entries {
            push(entry);
        }
    }
    assert!(
        view.blocks.is_empty(),
        "small fixture must not silently omit block custody"
    );
    let mut reader = crate::tail::TailReader::new(&view.tail_paths);
    for entry in &view.tail {
        let group = reader.group(entry.file_first, entry.offset).unwrap();
        push(&group.entries[entry.index as usize]);
    }
    rows.sort();
    rows
}

#[test]
fn publication_and_reclaim_between_discovery_steps_preserve_coverage() {
    let s = Scratch::new("discovery gap real publication checkpoint reclaim; no retention");
    let state = s.0.join("state");
    let groups = fixture(2_000_000);
    fs::write(s.0.join("expected-before-io.jsonl"), ledger(&groups)).unwrap();
    let mut expected: Vec<_> = ledger(&groups).lines().map(str::to_owned).collect();
    expected.sort();
    // Explicit producer-side log row ledger, before any journal/Segment IO.
    let mut log_rows = Vec::new();
    for sequence in 1u64..=8 {
        for node in 1u8..=3 {
            for index in 0u64..6 {
                let time = 1_000_000 + (0xCA7A10A1u64 % 17) * 100 + (8 - sequence) * 100;
                log_rows.push(
                    json!({"node":format!("node-{node}"),"node_id":format!("{node:02x}").repeat(16),
            "sequence":sequence,"index":index,"observed_ns":time+index,
            "body":format!("line {time}+{index} {}",if index%3==0{"needle"}else{"hay"}),
            "attributes":{"log.file.path":"/var/log/app.log"}}),
                );
            }
        }
    }
    fs::write(
        s.0.join("expected-log-rows-before-io.json"),
        json!(log_rows).to_string(),
    )
    .unwrap();
    let mut log = journal(&state);
    append(&mut log, &groups);
    log.rotate(1).unwrap();
    drop(log);
    verify(&s, &state, &groups, "discovery-gap-valid-query-control");
    let catalog = ReadCatalog::new(&state);
    let window = Window::new(0, 1u64 << 40).unwrap();
    let node = None;
    let request = || CatalogRequest {
        oldest: None,
        newest: 8,
        table: segment::Table::Logs,
        window: &window,
        node: &node,
        authorized_nodes: None,
        filter: FilterKind::None,
    };
    let baseline = catalog.view(request()).unwrap();
    assert_eq!(custody(&baseline), expected, "valid no-cut control");
    let sealed = state.join("journal/sealed-00000000000000000001.faj");
    let view=catalog.view_at_discovery_cut(request(),||{
        assert!(segment::labels(&state).unwrap().is_empty());
        assert!(sealed.exists());
        segment::build_sealed(&state,1,&sealed).unwrap();
        assert!(sealed.exists(),"publication precedes reclaim");
        pass(&state,u64::MAX);
        assert_eq!(segment::labels(&state).unwrap(),vec![1]);
        assert!(!sealed.exists(),"actual Store startup checkpoint/reclaim completed");
        fs::write(s.0.join("cut.json"),json!({"published":[1],"reclaimed_journal":1,"retention":"unlimited","callback":"after_segments_before_extend"}).to_string()).unwrap();
    });
    match view {
        Ok(view) => {
            let actual = custody(&view);
            fs::write(
                s.0.join("selected-custody.jsonl"),
                format!("{}\n", actual.join("\n")),
            )
            .unwrap();
            fs::write(s.0.join("selected-source-summary.json"),json!({"segments":view.segments.iter().map(|(_,m)|m.journal_label).collect::<Vec<_>>(),"oldest_group":view.oldest_group,"tail_paths":view.tail_paths,"tail_entries":view.tail.len(),"eager_groups":view.journal.len(),"blocks":view.blocks.len(),"expected_batches":expected.len(),"selected_batches":actual.len()}).to_string()).unwrap();
            assert_eq!(
                actual, expected,
                "catalog acquisition omitted committed custody after publication/reclaim discovery gap"
            );
        }
        Err(error) => {
            fs::write(
                s.0.join("selected-source-error.json"),
                json!({"kind":format!("{:?}",error.kind()),"error":error.to_string()}).to_string(),
            )
            .unwrap();
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::Interrupted,
                "unexpected acquisition error: {error}"
            );
            let retried = catalog
                .view(request())
                .expect("fresh discovery after explicit movement");
            let actual = custody(&retried);
            fs::write(
                s.0.join("selected-custody-after-retry.jsonl"),
                format!("{}\n", actual.join("\n")),
            )
            .unwrap();
            fs::write(s.0.join("selected-source-after-retry.json"),json!({"movement":"Interrupted","segments":retried.segments.iter().map(|(_,m)|m.journal_label).collect::<Vec<_>>(),"oldest_group":retried.oldest_group,"expected_batches":expected.len(),"selected_batches":actual.len()}).to_string()).unwrap();
            assert_eq!(
                actual, expected,
                "explicit movement retry must recover exact committed coverage"
            );
        }
    }
}

#[test]
fn cold_and_shared_discovery_cuts_preserve_minimal_custody() {
    for (name, shared, primed) in [
        ("cold-clone", false, false),
        ("primed-clone", false, true),
        ("cold-shared", true, false),
        ("primed-shared", true, true),
    ] {
        let s = Scratch::new(&format!(
            "minimal discovery gap {name}; one Batch no retention"
        ));
        let state = s.0.join("state");
        let mut groups = fixture(2_000_000);
        groups.truncate(1);
        groups[0].entries.truncate(1);
        fs::write(s.0.join("expected-before-io.jsonl"), ledger(&groups)).unwrap();
        fs::write(
            s.0.join("variant.json"),
            json!({"variant":name,"shared":shared,"primed":primed,"expected_batches":1})
                .to_string(),
        )
        .unwrap();
        let mut expected: Vec<_> = ledger(&groups).lines().map(str::to_owned).collect();
        expected.sort();
        let mut log = journal(&state);
        append(&mut log, &groups);
        log.rotate(1).unwrap();
        drop(log);
        verify(&s, &state, &groups, "minimal-discovery-valid-query-control");
        let window = Window::new(0, 1u64 << 40).unwrap();
        let node = None;
        let request = || CatalogRequest {
            oldest: None,
            newest: 1,
            table: segment::Table::Logs,
            window: &window,
            node: &node,
            authorized_nodes: None,
            filter: FilterKind::None,
        };
        // Independent no-cut control does not populate the target cold catalog.
        let control = ReadCatalog::new(&state);
        assert_eq!(
            custody(&control.view(request()).unwrap()),
            expected,
            "separate valid control {name}"
        );
        let mut catalog = ReadCatalog::new(&state);
        if shared {
            catalog.use_shared_metadata();
        }
        if primed {
            assert_eq!(
                custody(&catalog.view(request()).unwrap()),
                expected,
                "primed {name}"
            );
        }
        let sealed = state.join("journal/sealed-00000000000000000001.faj");
        let result=catalog.view_at_discovery_cut(request(),||{
            assert!(segment::labels(&state).unwrap().is_empty());assert!(sealed.exists());
            segment::build_sealed(&state,1,&sealed).unwrap();pass(&state,u64::MAX);
            assert_eq!(segment::labels(&state).unwrap(),vec![1]);assert!(!sealed.exists());
            fs::write(s.0.join("cut.json"),json!({"variant":name,"callback":"after_segments_before_extend","published":[1],"reclaimed_journal":1}).to_string()).unwrap();
        });
        let actual = match result {
            Ok(view) => {
                fs::write(
                    s.0.join("movement.json"),
                    json!({"variant":name,"outcome":"Ok","oldest_group":view.oldest_group})
                        .to_string(),
                )
                .unwrap();
                custody(&view)
            }
            Err(error) => {
                fs::write(s.0.join("movement.json"),json!({"variant":name,"outcome":format!("{:?}",error.kind()),"error":error.to_string()}).to_string()).unwrap();
                assert_eq!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted,
                    "{name}: {error}"
                );
                custody(
                    &catalog
                        .view(request())
                        .expect("exact fresh view after movement"),
                )
            }
        };
        fs::write(
            s.0.join("selected-custody.jsonl"),
            format!("{}\n", actual.join("\n")),
        )
        .unwrap();
        assert_eq!(
            actual, expected,
            "minimal discovery omitted custody in {name}"
        );
    }
}
