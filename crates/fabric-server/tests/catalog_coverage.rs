//! Catalog coverage cuts with independent prebuild ledgers; seed 0xCA7A10A1.
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

#[test]
fn active_pending_published_and_reclaimed_sources_cover_late_ties_exactly() {
    let s = Scratch::new("O1/O2/O3 real journal publication and reclaim cuts");
    let state = s.0.join("state");
    let groups = fixture(2_000_000);
    fs::write(s.0.join("expected-before-io.jsonl"), ledger(&groups)).unwrap();
    let mut log = journal(&state);
    append(&mut log, &groups[..4]);
    verify(&s, &state, &groups[..4], "active-first-four");
    let q = json!({"kind":"logs","from_ns":0,"to_ns":1u64<<40,"limit":2});
    let snapshots: Vec<_> = [Plan::Scan, Plan::Walk]
        .into_iter()
        .map(|plan| {
            let h = History::with_plan(&state, plan);
            let first = h
                .run(&serde_json::from_value(q.clone()).unwrap(), 4)
                .unwrap();
            (h, first)
        })
        .collect();
    append(&mut log, &groups[4..]);
    for (index, (h, first)) in snapshots.into_iter().enumerate() {
        let mut continuation = q.clone();
        continuation["page"] = first["next_page"].clone();
        let mut chain = vec![first];
        chain.extend(pages(&h, continuation, 8));
        grade(
            &s,
            &groups[..4],
            &q,
            &chain,
            "old-snapshot-after-append",
            if index == 0 { "scan" } else { "walk" },
            true,
        );
    }
    verify(&s, &state, &groups, "active-all-eight");
    let reused = [
        (History::with_plan(&state, Plan::Scan), "scan"),
        (History::with_plan(&state, Plan::Walk), "walk"),
    ];
    for (h, name) in &reused {
        for q in queries() {
            grade(
                &s,
                &groups,
                &q,
                &pages(h, q.clone(), 8),
                "active-before-rotation-reused",
                name,
                true,
            );
        }
    }
    log.rotate(1).unwrap();
    drop(log);
    let sealed = state.join("journal/sealed-00000000000000000001.faj");
    assert!(sealed.exists());
    verify(&s, &state, &groups, "pending-unpublished");
    for (h, name) in &reused {
        for q in queries() {
            grade(
                &s,
                &groups,
                &q,
                &pages(h, q.clone(), 8),
                "pending-unpublished-reused",
                name,
                true,
            );
        }
    }
    segment::build_sealed(&state, 1, &sealed).unwrap();
    assert!(sealed.exists(), "builder publication must precede reclaim");
    verify(&s, &state, &groups, "published-unreclaimed");
    for (h, name) in &reused {
        for q in queries() {
            grade(
                &s,
                &groups,
                &q,
                &pages(h, q.clone(), 8),
                "published-unreclaimed-reused",
                name,
                true,
            );
        }
    }
    pass(&state, u64::MAX);
    assert!(!sealed.exists(), "actual sealer pass reclaims the journal");
    verify(&s, &state, &groups, "reclaimed-reopened");
    for (h, name) in &reused {
        for q in queries() {
            grade(
                &s,
                &groups,
                &q,
                &pages(h, q.clone(), 8),
                "reclaimed-reopened-reused",
                name,
                true,
            );
        }
    }
    let mut replay = Vec::new();
    Store::replay(&state, 1 << 28, |e| {
        replay.push(
            json!({"label":e.label,"received_ns":e.received_unix_nano,"bytes":b64(&e.batch)})
                .to_string(),
        );
        Ok(())
    })
    .unwrap();
    let expected: Vec<_> = ledger(&groups).lines().map(str::to_owned).collect();
    replay.sort();
    let mut expected = expected;
    expected.sort();
    assert_eq!(
        replay, expected,
        "exact raw custody across coverage replacement"
    );
    let h = History::new(&state);
    let clean = pages(&h, q.clone(), 8);
    let mut missing = clean.clone();
    missing[0]["rows"].as_array_mut().unwrap().pop();
    grade(
        &s,
        &groups,
        &q,
        &missing,
        "negative-dropped-row",
        "scan",
        false,
    );
    let mut duplicate = clean;
    let row = duplicate[0]["rows"][0].clone();
    duplicate[0]["rows"].as_array_mut().unwrap().push(row);
    grade(
        &s,
        &groups,
        &q,
        &duplicate,
        "negative-duplicate-row",
        "scan",
        false,
    );
}

#[test]
fn retained_descriptors_do_not_resurrect_expired_page_snapshots() {
    let s = Scratch::new("O4 logical retention distinct from History lifetime");
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
    log.rotate(5).unwrap();
    drop(log);
    pass(&state, u64::MAX);
    verify(&s, &state, &groups, "before-retention");
    let q = json!({"kind":"logs","from_ns":0,"to_ns":1u64<<40,"limit":1});
    let saved: Vec<_> = [Plan::Scan, Plan::Walk]
        .into_iter()
        .map(|p| {
            let h = History::with_plan(&state, p);
            let first = h
                .run(&serde_json::from_value(q.clone()).unwrap(), 8)
                .unwrap();
            (h, first["next_page"].clone())
        })
        .collect();
    pass(&state, 86400);
    assert_eq!(segment::labels(&state).unwrap(), vec![5]);
    for (h, token) in saved {
        let mut next = q.clone();
        next["page"] = token;
        assert!(matches!(
            h.run(&serde_json::from_value(next).unwrap(), 8),
            Err(QueryError::Gone)
        ));
    }
    verify(&s, &state, &groups[4..], "after-retention");
}

#[test]
fn optional_filter_fallback_is_exact_and_authenticated_lies_are_detected() {
    use sha2::{Digest, Sha256};
    let s = Scratch::new("O5 absent/corrupt filters and forged false-negative control");
    let state = s.0.join("state");
    let groups = fixture(2_000_000);
    fs::write(s.0.join("expected-before-io.jsonl"), ledger(&groups)).unwrap();
    let mut log = journal(&state);
    append(&mut log, &groups);
    log.rotate(1).unwrap();
    drop(log);
    pass(&state, u64::MAX);
    let dir = state.join("segments").join(segment::segment_name(1));
    let manifest = segment::read_manifest(&dir).unwrap();
    let path = dir.join(fabric_server::text_filter::FILE);
    let original = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    verify(&s, &state, &groups, "absent-filter");
    fs::write(&path, b"malformed optional filter").unwrap();
    verify(&s, &state, &groups, "malformed-filter");
    let count = fabric_server::text_filter::decode(&original).unwrap().len();
    let mut zeroed = original;
    zeroed[8 + 4 * count..].fill(0);
    fs::write(&path, &zeroed).unwrap();
    assert!(segment::read_text_filter(&dir, &manifest).is_none());
    verify(&s, &state, &groups, "unauthenticated-false-negative");
    let mut lying = manifest;
    lying
        .files
        .get_mut(fabric_server::text_filter::FILE)
        .unwrap()
        .sha256 = Sha256::digest(&zeroed)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec_pretty(&lying).unwrap(),
    )
    .unwrap();
    assert!(segment::read_text_filter(&dir, &lying).is_some());
    let q = json!({"kind":"logs","contains":"needle","from_ns":0,"to_ns":1u64<<40,"limit":3});
    let answer = pages(&History::with_plan(&state, Plan::Walk), q.clone(), 8);
    grade(
        &s,
        &groups,
        &q,
        &answer,
        "negative-authenticated-filter-lie",
        "walk",
        false,
    );
}

#[test]
fn simultaneous_active_pending_and_published_sources_cover_every_identity() {
    let s = Scratch::new("O1 simultaneous active pending published sources");
    let state = s.0.join("state");
    let groups = fixture(2_000_000);
    fs::write(s.0.join("expected-before-io.jsonl"), ledger(&groups)).unwrap();
    let mut log = journal(&state);
    append(&mut log, &groups[..2]);
    log.rotate(1).unwrap();
    let first = state.join("journal/sealed-00000000000000000001.faj");
    segment::build_sealed(&state, 1, &first).unwrap();
    append(&mut log, &groups[2..4]);
    log.rotate(3).unwrap();
    append(&mut log, &groups[4..]);
    assert!(first.exists());
    assert!(
        state
            .join("journal/sealed-00000000000000000003.faj")
            .exists()
    );
    assert_eq!(segment::labels(&state).unwrap(), vec![1]);
    verify(&s, &state, &groups, "simultaneous-three-kinds");
    let reused = [
        (History::with_plan(&state, Plan::Scan), "scan"),
        (History::with_plan(&state, Plan::Walk), "walk"),
    ];
    for (h, name) in &reused {
        for q in queries() {
            grade(
                &s,
                &groups,
                &q,
                &pages(h, q.clone(), 8),
                "simultaneous-reused-before-reclaim",
                name,
                true,
            );
        }
    }
    drop(log);
    pass(&state, u64::MAX);
    assert!(!first.exists());
    assert!(
        !state
            .join("journal/sealed-00000000000000000003.faj")
            .exists()
    );
    let mut labels = segment::labels(&state).unwrap();
    labels.sort_unstable();
    assert_eq!(labels, vec![1, 3]);
    verify(&s, &state, &groups, "published-and-active-after-reclaim");
    for (h, name) in &reused {
        for q in queries() {
            grade(
                &s,
                &groups,
                &q,
                &pages(h, q.clone(), 8),
                "simultaneous-reused-after-reclaim",
                name,
                true,
            );
        }
    }
    let mut replay = Vec::new();
    Store::replay(&state, 1 << 28, |e| {
        replay.push(
            json!({"label":e.label,"received_ns":e.received_unix_nano,"bytes":b64(&e.batch)})
                .to_string(),
        );
        Ok(())
    })
    .unwrap();
    let mut expected: Vec<_> = ledger(&groups).lines().map(str::to_owned).collect();
    replay.sort();
    expected.sort();
    let replay_bytes = format!("{}\n", replay.join("\n"));
    fs::write(s.0.join("raw-replay-sorted.jsonl"), &replay_bytes).unwrap();
    {
        use sha2::{Digest, Sha256};
        let digest: String = Sha256::digest(replay_bytes.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        fs::write(s.0.join("raw-replay-sha256.txt"), format!("{digest}\n")).unwrap();
    }
    assert_eq!(replay, expected);
}

fn rewrite_log_time_statistics(path: &Path, original: &[u8], stats: Option<(i64, i64)>) {
    use parquet::{
        arrow::arrow_reader::ParquetRecordBatchReaderBuilder,
        file::{metadata::ParquetMetaDataWriter, statistics::Statistics, writer::TrackedWrite},
    };
    use std::io::Write;
    // Restore before parsing so variants never inherit each other's metadata.
    fs::write(path, original).unwrap();
    let builder = ParquetRecordBatchReaderBuilder::try_new(fs::File::open(path).unwrap()).unwrap();
    let meta = builder.metadata().as_ref().clone();
    let row_groups = meta
        .row_groups()
        .iter()
        .cloned()
        .map(|rg| {
            let mut columns = rg.columns().to_vec();
            let column = columns[5].clone().into_builder();
            columns[5] = match stats {
                Some((min, max)) => column.set_statistics(Statistics::new::<i64>(
                    Some(min),
                    Some(max),
                    None,
                    Some(0),
                    false,
                )),
                None => column.clear_statistics(),
            }
            .build()
            .unwrap();
            rg.into_builder()
                .set_column_metadata(columns)
                .build()
                .unwrap()
        })
        .collect();
    let changed = meta.into_builder().set_row_groups(row_groups).build();
    let len = original.len();
    let footer = u32::from_le_bytes(original[len - 8..len - 4].try_into().unwrap()) as usize;
    let mut bytes = Vec::new();
    {
        let mut tracked = TrackedWrite::new(&mut bytes);
        tracked.write_all(&original[..len - footer - 8]).unwrap();
        ParquetMetaDataWriter::new_with_tracked(tracked, &changed)
            .finish()
            .unwrap();
    }
    fs::write(path, bytes).unwrap();
}

#[test]
fn absent_numeric_bounds_fall_back_and_lying_footer_bounds_are_rejected() {
    use sha2::{Digest, Sha256};
    let s = Scratch::new("O5 real Parquet footer statistics omission and numeric lies");
    let state = s.0.join("state");
    let groups = fixture(2_000_000);
    fs::write(s.0.join("expected-before-io.jsonl"), ledger(&groups)).unwrap();
    let mut log = journal(&state);
    append(&mut log, &groups);
    log.rotate(1).unwrap();
    drop(log);
    pass(&state, u64::MAX);
    let dir = state.join("segments").join(segment::segment_name(1));
    let original_manifest = segment::read_manifest(&dir).unwrap();
    let path = dir.join("logs.parquet");
    let original = fs::read(&path).unwrap();
    let q = json!({"kind":"logs","from_ns":0,"to_ns":1u64<<40,"limit":3});
    for (stage, stats, expected_pass) in [
        ("absent-numeric-bounds", None, true),
        (
            "negative-reversed-numeric-bounds",
            Some((3_000_000_000_000, 0)),
            false,
        ),
        (
            "negative-plausible-false-negative-bounds",
            Some((3_000_000_000_000, 3_000_000_000_100)),
            false,
        ),
    ] {
        rewrite_log_time_statistics(&path, &original, stats);
        // Authenticate the rewritten footer and retain correct size/row count.
        // The control then tests bound truth, not a file-size/digest failure.
        let raw = fs::read(&path).unwrap();
        let mut manifest = original_manifest.clone();
        let file = manifest.files.get_mut("logs.parquet").unwrap();
        file.bytes = raw.len() as u64;
        file.sha256 = Sha256::digest(&raw)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let bounds = segment::row_group_bounds(&dir, &manifest, segment::Table::Logs).unwrap();
        if stats.is_none() {
            assert!(
                bounds
                    .iter()
                    .all(|(_, min, max)| *min == 0 && *max == u64::MAX)
            );
        }
        for (plan, name) in [(Plan::Scan, "scan"), (Plan::Walk, "walk")] {
            let answers = pages(&History::with_plan(&state, plan), q.clone(), 8);
            grade(&s, &groups, &q, &answers, stage, name, expected_pass);
        }
    }
}
