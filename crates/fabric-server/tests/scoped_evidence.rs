//! Origin: production fixed-query gate exposed repeated scoped raw-Batch scans.
//! Literal metadata/row expectations protect cache reuse, scope changes, snapshot
//! publication and unavailable storage. These tests do not claim HTTP coverage.
use fabric_frame::{envelope::Batch, frame::FrameLog};
use fabric_server::{
    query::{History, Plan, Query, QueryError},
    sealer::{self, Retention},
    segment,
    store::{CommitMode, Entry, Group, Store},
};
use opentelemetry_proto::tonic::{
    collector::{
        logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
        trace::v1::ExportTraceServiceRequest,
    },
    common::v1::{AnyValue, any_value},
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
    metrics::v1::{
        Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum, metric, number_data_point,
    },
    trace::v1::{ResourceSpans, ScopeSpans, Span},
};
use prost::Message;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const BYTES: u64 = 2 * 1024 * 1024;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = PathBuf::from(
            std::env::var("FABRIC_SCRATCH_ROOT").expect("contained mounted scratch required"),
        )
        .join(format!(
            "scoped-evidence-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn save(&self, name: &str, value: &Value) {
        fs::write(self.0.join(format!("{name}.json")), value.to_string()).unwrap();
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("scoped evidence failure retained at {}", self.0.display());
        } else {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

fn group(
    sequences: (u64, u64),
    node: &str,
    id: u8,
    log: u64,
    metric_time: u64,
    span_time: u64,
    receive: u64,
) -> Group {
    let (group_sequence, sequence) = sequences;
    let logs = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: vec![LogRecord {
                    observed_time_unix_nano: log,
                    body: Some(AnyValue {
                        value: Some(any_value::Value::StringValue(format!(
                            "literal-{node}-{sequence}"
                        ))),
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    let metrics = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![Metric {
                    name: "literal.counter".into(),
                    data: Some(metric::Data::Sum(Sum {
                        data_points: vec![NumberDataPoint {
                            time_unix_nano: metric_time,
                            value: Some(number_data_point::Value::AsInt(sequence as i64)),
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
    }
    .encode_to_vec();
    let traces = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: vec![Span {
                    trace_id: vec![id; 16],
                    span_id: sequence.to_be_bytes().to_vec(),
                    name: "literal.span".into(),
                    start_time_unix_nano: span_time,
                    end_time_unix_nano: span_time + 1,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    Group {
        group_sequence,
        entries: vec![Entry {
            label: node.into(),
            received_unix_nano: receive,
            batch: Batch {
                version: 1,
                node_id: vec![id; 16],
                generation: 1,
                sequence,
                logs,
                metrics,
                traces,
                ..Default::default()
            }
            .encode_to_vec(),
        }],
    }
}
fn groups() -> [Group; 3] {
    [
        group((1, 1), "alpha", 1, 100, 400, 700, 11),
        group((2, 2), "alpha", 1, 200, 500, 800, 22),
        group((3, 1), "beta", 2, 300, 600, 900, 33),
    ]
}
fn scope(nodes: &[&str]) -> HashSet<String> {
    nodes.iter().map(|node| (*node).to_owned()).collect()
}
fn request(kind: &str, limit: u32, node: Option<&str>) -> Value {
    let mut value = json!({"kind":kind,"from_ns":0,"to_ns":1000,"limit":limit,"node":node});
    if kind == "metrics" {
        value["name"] = json!("literal.counter");
    }
    value
}
fn query(value: Value) -> Query {
    serde_json::from_value(value).unwrap()
}
fn metadata(answer: &Value, freshness: Value, from: u64, to: u64) {
    assert_eq!(answer["complete"], true, "{answer}");
    assert_eq!(answer["unavailable"], json!([]));
    assert_eq!(answer["freshness"], freshness);
    assert_eq!(answer["retained_from_ns"], from);
    assert_eq!(answer["retained_to_ns"], to);
    assert_eq!(answer["gaps"], json!([]));
}
fn times(answer: &Value, field: &str) -> Vec<u64> {
    answer["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[field].as_u64().unwrap())
        .collect()
}

fn sealed_fixture(state: &Path) {
    fs::create_dir(state.join("journal")).unwrap();
    drop(
        FrameLog::open(
            &state.join("journal"),
            BYTES,
            segment::MAX_GROUP_PAYLOAD,
            |_, _| Ok(()),
        )
        .unwrap(),
    );
    segment::build(state, 1, &groups()).unwrap();
}

#[test]
fn cold_and_warm_full_scoped_evidence_keeps_literal_signal_and_node_boundaries() {
    let scratch = Scratch::new();
    sealed_fixture(&scratch.0);
    let history = History::with_plan(&scratch.0, Plan::Walk);
    let both = scope(&["alpha", "beta"]);
    for (kind, field, expected, freshness) in [
        (
            "logs",
            "observed_ns",
            vec![100, 200, 300],
            json!({"alpha":200,"beta":300}),
        ),
        (
            "metrics",
            "time_ns",
            vec![400, 500, 600],
            json!({"alpha":500,"beta":600}),
        ),
        (
            "spans",
            "start_ns",
            vec![700, 800, 900],
            json!({"alpha":800,"beta":900}),
        ),
    ] {
        let q = query(request(kind, 10, None));
        for attempt in 0..2 {
            let answer = history.run_scoped(&q, 3, &both).unwrap();
            scratch.save(&format!("{kind}-{attempt}"), &answer);
            metadata(&answer, freshness.clone(), 11, 33);
            assert_eq!(times(&answer, field), expected);
        }
        let alpha = history.run_scoped(&q, 3, &scope(&["alpha"])).unwrap();
        metadata(&alpha, json!({"alpha":freshness["alpha"]}), 11, 22);
        assert_eq!(times(&alpha, field), expected[..2]);
        let wider = history.run_scoped(&q, 3, &both).unwrap();
        metadata(&wider, freshness.clone(), 11, 33);
        assert_eq!(times(&wider, field), expected);
        let beta = history
            .run_scoped(&query(request(kind, 10, Some("beta"))), 3, &both)
            .unwrap();
        metadata(&beta, json!({"beta":freshness["beta"]}), 33, 33);
        assert_eq!(times(&beta, field), expected[2..]);
        let empty = history.run_scoped(&q, 3, &scope(&[])).unwrap();
        metadata(&empty, json!({}), 0, 0);
        assert_eq!(empty["rows"], json!([]));
    }
}

fn publish(state: &Path) {
    let (intake, thread) = Store::open(state, BYTES, CommitMode::INDIVIDUAL)
        .unwrap()
        .spawn_joinable()
        .unwrap();
    let result = sealer::pass(
        state,
        &intake,
        Retention {
            max_age_s: u64::MAX,
            max_bytes: u64::MAX,
        },
        1,
    );
    drop(intake);
    thread.join().unwrap();
    result.unwrap();
}

#[test]
fn fixed_partial_snapshot_pages_remain_exact_after_a_straddling_publication() {
    let scratch = Scratch::new();
    fs::create_dir(scratch.0.join("journal")).unwrap();
    let mut log = FrameLog::open(
        &scratch.0.join("journal"),
        BYTES,
        segment::MAX_GROUP_PAYLOAD,
        |_, _| Ok(()),
    )
    .unwrap();
    let groups = groups();
    for group in &groups[..2] {
        log.append(&group.encode_to_vec()).unwrap();
    }
    let history = History::with_plan(&scratch.0, Plan::Walk);
    let allowed = scope(&["alpha", "beta"]);
    let first = history
        .run_scoped(&query(request("logs", 1, None)), 2, &allowed)
        .unwrap();
    metadata(&first, json!({"alpha":200}), 11, 22);
    assert_eq!(times(&first, "observed_ns"), vec![100]);
    assert!(first["next_page"].is_string());
    log.append(&groups[2].encode_to_vec()).unwrap();
    log.rotate(1).unwrap();
    drop(log);
    publish(&scratch.0);
    let manifests = segment::list(&scratch.0).unwrap();
    assert_eq!(manifests.len(), 1);
    assert_eq!(
        (manifests[0].1.first_group, manifests[0].1.last_group),
        (1, 3)
    );
    // Populate a full-Segment cache first, then resume the older partial snapshot.
    let fresh = history
        .run_scoped(&query(request("logs", 10, None)), 3, &allowed)
        .unwrap();
    metadata(&fresh, json!({"alpha":200,"beta":300}), 11, 33);
    let mut next = request("logs", 1, None);
    next["page"] = first["next_page"].clone();
    let second = history.run_scoped(&query(next), 3, &allowed).unwrap();
    scratch.save("partial-after-publication", &second);
    metadata(&second, json!({"alpha":200}), 11, 22);
    assert_eq!(times(&second, "observed_ns"), vec![200]);
    assert!(second["next_page"].is_null());
}

#[test]
fn warm_evidence_does_not_hide_corrupt_or_missing_raw_storage() {
    for remove in [false, true] {
        let scratch = Scratch::new();
        sealed_fixture(&scratch.0);
        let history = History::with_plan(&scratch.0, Plan::Walk);
        let allowed = scope(&["alpha", "beta"]);
        let full = query(request("logs", 10, None));
        let warm = history.run_scoped(&full, 3, &allowed).unwrap();
        metadata(&warm, json!({"alpha":200,"beta":300}), 11, 33);
        let first = history
            .run_scoped(&query(request("logs", 1, None)), 3, &allowed)
            .unwrap();
        let directory = scratch.0.join("segments").join(segment::segment_name(1));
        let raw = directory.join("batches.parquet");
        if remove {
            fs::remove_file(&raw).unwrap();
        } else {
            fs::write(&raw, b"deterministic corrupt raw parquet").unwrap();
        }
        let answer = history.run_scoped(&full, 3, &allowed).unwrap();
        scratch.save("unavailable-after-warm-cache", &answer);
        assert_eq!(answer["complete"], false, "{answer}");
        assert!(!answer["unavailable"].as_array().unwrap().is_empty());
        fs::remove_dir_all(directory).unwrap();
        let mut continuation = request("logs", 1, None);
        continuation["page"] = first["next_page"].clone();
        assert!(
            matches!(
                history.run_scoped(&query(continuation), 3, &allowed),
                Err(QueryError::Gone)
            ),
            "removed page snapshot must be Gone"
        );
    }
}
