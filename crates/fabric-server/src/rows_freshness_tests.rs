//! Known-value controls for metadata-only decoding, independent of extract.
//! Root wires this module only when latest_observation_ns is promoted.
use super::latest_observation_ns;
use crate::store::Entry;
use fabric_frame::envelope::Batch;
use opentelemetry_proto::tonic::{
    collector::{
        logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
        trace::v1::ExportTraceServiceRequest,
    },
    common::v1::{AnyValue, any_value},
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
    metrics::v1::{
        Gauge, Histogram, HistogramDataPoint, Metric, NumberDataPoint, ResourceMetrics,
        ScopeMetrics, Sum, metric,
    },
    trace::v1::{ResourceSpans, ScopeSpans, Span},
};
use prost::Message;

fn envelope() -> Batch {
    Batch {
        version: 1,
        node_id: vec![7; 16],
        generation: 2,
        sequence: 3,
        ..Default::default()
    }
}
fn entry(batch: Batch) -> Entry {
    Entry {
        label: "known-node".into(),
        received_unix_nano: 999_999,
        batch: batch.encode_to_vec(),
    }
}
fn logs(records: Vec<LogRecord>) -> Vec<u8> {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: records,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec()
}
fn metrics(values: Vec<Metric>) -> Vec<u8> {
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            scope_metrics: vec![ScopeMetrics {
                metrics: values,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec()
}
fn traces(values: Vec<Span>) -> Vec<u8> {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: values,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec()
}

#[test]
fn known_values_keep_observation_point_and_start_semantics() {
    let mut batch = envelope();
    batch.logs = logs(vec![
        LogRecord {
            observed_time_unix_nano: 101,
            time_unix_nano: 50_000,
            body: Some(AnyValue {
                value: Some(any_value::Value::IntValue(9)),
            }),
            ..Default::default()
        },
        LogRecord {
            observed_time_unix_nano: 303,
            body: None,
            ..Default::default()
        },
    ]);
    batch.metrics = metrics(vec![
        Metric {
            data: Some(metric::Data::Gauge(Gauge {
                data_points: vec![NumberDataPoint {
                    time_unix_nano: 405,
                    start_time_unix_nano: 60_000,
                    value: None,
                    ..Default::default()
                }],
            })),
            ..Default::default()
        },
        Metric {
            data: Some(metric::Data::Sum(Sum {
                data_points: vec![NumberDataPoint {
                    time_unix_nano: 507,
                    value: None,
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        },
        Metric {
            data: Some(metric::Data::Histogram(Histogram {
                data_points: vec![HistogramDataPoint {
                    time_unix_nano: 70_000,
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        },
    ]);
    batch.traces = traces(vec![Span {
        start_time_unix_nano: 609,
        end_time_unix_nano: 80_000,
        ..Default::default()
    }]);
    assert_eq!(
        latest_observation_ns(&entry(batch.clone())).unwrap(),
        Some(609)
    );
    batch.traces.clear();
    assert_eq!(
        latest_observation_ns(&entry(batch.clone())).unwrap(),
        Some(507)
    );
    batch.metrics.clear();
    assert_eq!(latest_observation_ns(&entry(batch)).unwrap(), Some(303));
}

#[test]
fn zero_is_an_observation_but_gaps_and_unsupported_metrics_are_not() {
    for signal in ["logs", "metrics", "traces", "gap", "histogram", "empty"] {
        let mut batch = envelope();
        match signal {
            "logs" => batch.logs = logs(vec![LogRecord::default()]),
            "metrics" => {
                batch.metrics = metrics(vec![Metric {
                    data: Some(metric::Data::Gauge(Gauge {
                        data_points: vec![NumberDataPoint::default()],
                    })),
                    ..Default::default()
                }])
            }
            "traces" => batch.traces = traces(vec![Span::default()]),
            "gap" => batch.collection_gaps = vec!["unknown interval".into()],
            "histogram" => {
                batch.metrics = metrics(vec![Metric {
                    data: Some(metric::Data::Histogram(Histogram {
                        data_points: vec![HistogramDataPoint {
                            time_unix_nano: 90_000,
                            ..Default::default()
                        }],
                        ..Default::default()
                    })),
                    ..Default::default()
                }])
            }
            "empty" => {}
            _ => unreachable!(),
        }
        let expected = match signal {
            "logs" | "metrics" | "traces" => Some(0),
            _ => None,
        };
        assert_eq!(
            latest_observation_ns(&entry(batch)).unwrap(),
            expected,
            "{signal}"
        );
    }
}

// Tiny length-delimited fields below 128 bytes; hand-encoded invalid UTF-8
// cannot be constructed with the generated String API. No candidate encoder.
fn field(tag: u8, bytes: &[u8]) -> Vec<u8> {
    assert!(tag < 16 && bytes.len() < 128);
    [vec![(tag << 3) | 2, bytes.len() as u8], bytes.to_vec()].concat()
}

#[test]
fn malformed_envelope_identity_and_every_payload_are_rejected() {
    for defect in [
        "envelope",
        "identity",
        "logs",
        "metrics",
        "traces",
        "ignored_log_schema",
        "ignored_metric_description",
        "ignored_trace_schema",
    ] {
        let mut batch = envelope();
        match defect {
            "envelope" => {}
            "identity" => {
                batch.node_id = vec![7; 15];
                batch.logs = vec![0xff];
            }
            "logs" => batch.logs = vec![0xff],
            "metrics" => batch.metrics = vec![0xff],
            "traces" => batch.traces = vec![0xff],
            // Resource schema_url is irrelevant to freshness but still typed.
            "ignored_log_schema" => batch.logs = field(1, &field(3, &[0xff])),
            "ignored_trace_schema" => batch.traces = field(1, &field(3, &[0xff])),
            // Request.resource.scope.metric.description must decode even if
            // the Metric has no supported points and contributes no freshness.
            "ignored_metric_description" => {
                batch.metrics = field(1, &field(2, &field(2, &field(2, &[0xff]))))
            }
            _ => unreachable!(),
        }
        let mut record = entry(batch);
        if defect == "envelope" {
            record.batch = vec![0xff];
        }
        let error = latest_observation_ns(&record).expect_err(defect);
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{defect}");
        if defect == "identity" {
            assert_eq!(error.to_string(), "batch node_id is not 16 bytes");
        }
    }
}

#[test]
fn earlier_signal_decode_errors_take_precedence() {
    let bad_logs = vec![0xff];
    let bad_metrics = vec![0];
    let mut batch = envelope();
    batch.logs = bad_logs.clone();
    batch.metrics = bad_metrics.clone();
    batch.traces = vec![0x0a, 0x7f];
    let expected_logs = ExportLogsServiceRequest::decode(bad_logs.as_slice()).unwrap_err();
    assert_eq!(
        latest_observation_ns(&entry(batch.clone()))
            .unwrap_err()
            .to_string(),
        expected_logs.to_string()
    );
    batch.logs.clear();
    let expected_metrics = ExportMetricsServiceRequest::decode(bad_metrics.as_slice()).unwrap_err();
    assert_ne!(expected_logs.to_string(), expected_metrics.to_string());
    assert_eq!(
        latest_observation_ns(&entry(batch))
            .unwrap_err()
            .to_string(),
        expected_metrics.to_string()
    );
}
