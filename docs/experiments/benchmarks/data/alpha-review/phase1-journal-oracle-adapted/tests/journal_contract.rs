use fabric_o11y::alpha::journal::{Batch, Cursor, Journal};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, metric, number_data_point,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("data")
            .join(format!("{label}-{}-{id}", std::process::id()));
        fs::create_dir(&path).expect("fresh scratch directory");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove owned scratch directory");
    }
}

fn string_attr(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.into())),
        }),
        ..Default::default()
    }
}

fn metric_request() -> Vec<u8> {
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource {
                attributes: vec![string_attr("service.name", "alpha-oracle")],
                ..Default::default()
            }),
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![Metric {
                    name: "sample.count".into(),
                    unit: "1".into(),
                    data: Some(metric::Data::Gauge(Gauge {
                        data_points: vec![NumberDataPoint {
                            start_time_unix_nano: 1_000,
                            time_unix_nano: 2_000,
                            value: Some(number_data_point::Value::AsInt(7)),
                            ..Default::default()
                        }],
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec()
}

fn log_request(body: String) -> Vec<u8> {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![string_attr("service.name", "alpha-oracle")],
                ..Default::default()
            }),
            scope_logs: vec![ScopeLogs {
                log_records: vec![LogRecord {
                    time_unix_nano: 3_000,
                    observed_time_unix_nano: 4_000,
                    body: Some(AnyValue {
                        value: Some(any_value::Value::StringValue(body)),
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec()
}

fn batch(body: &str) -> Batch {
    Batch {
        version: 1,
        node_id: vec![0xAB; 16],
        generation: 777,
        sequence: 900,
        metrics: metric_request(),
        logs: log_request(body.into()),
        cursors: vec![Cursor {
            path: "/var/log/example.log".into(),
            device: 12,
            inode: 34,
            offset: 56,
            skipping_oversize: false,
            prefix_len: 0,
            prefix_crc: 0,
        }],
        collection_gaps: vec!["source unavailable before offset 56".into()],
    }
}

#[test]
fn identity_and_sequence_survive_restart_and_are_scoped_to_directory() {
    let a = Scratch::new("identity-a");
    let b = Scratch::new("identity-b");
    let mut journal = Journal::open(&a.0, 64 * 1024).unwrap();
    let identity = journal.identity();
    assert_ne!(identity.0, [0; 16], "node identity must be nonzero");
    assert_ne!(identity.1, 0, "generation must be nonzero");
    let first_sequence = journal.next_sequence();
    let input = batch("first");
    let committed = journal.append(&input).unwrap();
    assert_eq!(committed.node_id, identity.0.to_vec());
    assert_eq!(committed.generation, identity.1);
    assert_eq!(committed.sequence, first_sequence);
    assert_eq!(committed.version, input.version);
    assert_eq!(committed.metrics, input.metrics);
    assert_eq!(committed.logs, input.logs);
    assert_eq!(committed.cursors, input.cursors);
    assert_eq!(committed.collection_gaps, input.collection_gaps);
    committed.validate().unwrap();
    assert_eq!(journal.next_sequence(), first_sequence + 1);
    drop(journal);

    let mut reopened = Journal::open(&a.0, 64 * 1024).unwrap();
    assert_eq!(reopened.identity(), identity);
    assert_eq!(reopened.next_sequence(), first_sequence + 1);
    let second = reopened.append(&batch("second")).unwrap();
    assert_eq!(second.sequence, first_sequence + 1);
    drop(reopened);
    let reopened = Journal::open(&a.0, 64 * 1024).unwrap();
    assert_eq!(reopened.next_sequence(), first_sequence + 2);
    let independent = Journal::open(&b.0, 64 * 1024).unwrap();
    assert_ne!(independent.identity().0, identity.0);
}

#[test]
fn complete_otlp_payload_wire_bytes_are_unchanged_in_committed_batch() {
    let dir = Scratch::new("wire-bytes");
    let mut journal = Journal::open(&dir.0, 64 * 1024).unwrap();
    let mut input = batch("unicode: café; observed after event");
    // Unknown protobuf field 127, varint value 1: valid wire data that a decode/re-encode loses.
    input.metrics.extend_from_slice(&[0xF8, 0x07, 0x01]);
    input.logs.extend_from_slice(&[0xF8, 0x07, 0x01]);
    ExportMetricsServiceRequest::decode(input.metrics.as_slice()).unwrap();
    ExportLogsServiceRequest::decode(input.logs.as_slice()).unwrap();
    let committed = journal.append(&input).unwrap();
    assert_eq!(committed.metrics, input.metrics);
    assert_eq!(committed.logs, input.logs);
    assert_eq!(committed.cursors, input.cursors);
    assert_eq!(committed.collection_gaps, input.collection_gaps);
}

#[test]
fn malformed_otlp_rejection_keeps_sequence_and_used_bytes() {
    let dir = Scratch::new("bad-otlp");
    let mut journal = Journal::open(&dir.0, 64 * 1024).unwrap();
    let before_sequence = journal.next_sequence();
    let before_bytes = journal.used_bytes();
    for corrupt_metrics in [true, false] {
        let mut input = batch("valid companion signal");
        if corrupt_metrics {
            input.metrics = vec![0x80]; // truncated protobuf varint
        } else {
            input.logs = vec![0x80];
        }
        assert!(input.validate().is_err());
        assert!(journal.append(&input).is_err());
        assert_eq!(journal.next_sequence(), before_sequence);
        assert_eq!(journal.used_bytes(), before_bytes);
    }
    let committed = journal.append(&batch("after invalid input")).unwrap();
    assert_eq!(committed.sequence, before_sequence);
}

#[test]
fn encoded_batch_larger_than_one_mebibyte_is_rejected_without_commit() {
    let dir = Scratch::new("oversize");
    let mut journal = Journal::open(&dir.0, 2 * 1024 * 1024).unwrap();
    let before_sequence = journal.next_sequence();
    let before_bytes = journal.used_bytes();
    let mut input = batch("small");
    input.logs = log_request("X".repeat(1_048_576));
    assert!(input.encoded_len() > 1_048_576);
    assert!(journal.append(&input).is_err());
    assert_eq!(journal.next_sequence(), before_sequence);
    assert_eq!(journal.used_bytes(), before_bytes);
    drop(journal);
    let mut reopened = Journal::open(&dir.0, 2 * 1024 * 1024).unwrap();
    assert_eq!(reopened.next_sequence(), before_sequence);
    assert_eq!(reopened.append(&batch("next accepted")).unwrap().sequence, before_sequence);
}

#[test]
fn full_journal_is_bounded_and_does_not_commit_rejected_cursor() {
    let dir = Scratch::new("full");
    let limit = 8 * 1024;
    let mut journal = Journal::open(&dir.0, limit).unwrap();
    let mut accepted = 0;
    let mut rejected = false;
    for n in 0..16_u64 {
        let mut input = batch(&"Q".repeat(1_500));
        input.cursors[0].offset = n + 1;
        let before_sequence = journal.next_sequence();
        let before_bytes = journal.used_bytes();
        match journal.append(&input) {
            Ok(committed) => {
                accepted += 1;
                assert_eq!(committed.sequence, before_sequence);
                assert_eq!(committed.cursors[0].offset, n + 1);
                assert!(journal.used_bytes() <= limit);
            }
            Err(_) => {
                rejected = true;
                assert_eq!(journal.next_sequence(), before_sequence);
                assert_eq!(journal.used_bytes(), before_bytes);
                break;
            }
        }
    }
    assert!(accepted > 0, "small valid batch should fit");
    assert!(rejected, "finite byte limit must eventually reject batches");
    let next = journal.next_sequence();
    let used = journal.used_bytes();
    drop(journal);
    let reopened = Journal::open(&dir.0, limit).unwrap();
    assert_eq!(reopened.next_sequence(), next);
    assert_eq!(reopened.used_bytes(), used);
}

#[test]
fn only_one_live_writer_can_open_a_directory() {
    let dir = Scratch::new("writer-lock");
    let first = Journal::open(&dir.0, 64 * 1024).unwrap();
    assert!(Journal::open(&dir.0, 64 * 1024).is_err());
    drop(first);
    assert!(Journal::open(&dir.0, 64 * 1024).is_ok());
}
