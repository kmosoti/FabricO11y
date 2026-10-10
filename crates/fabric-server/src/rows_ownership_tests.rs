//! Frozen all-signal differential and logs ownership controls.
use super::*;

// Frozen pre-ownership implementation. Do not route its log projection through
// the candidate helper: whole-extract timings include the same decode/error order.
fn legacy_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn legacy_strings(attributes: &[KeyValue]) -> Attributes {
    attributes
        .iter()
        .filter_map(|kv| match kv.value.as_ref()?.value.as_ref()? {
            any_value::Value::StringValue(s) => Some((kv.key.clone(), s.clone())),
            _ => None,
        })
        .collect()
}

fn legacy_extract(group: u64, entry: &Entry, out: &mut Rows) -> io::Result<()> {
    #[cfg(feature = "phase-probe")]
    let _phase = fabric_frame::probe::span("otlp_decode_project");

    let batch = {
        #[cfg(feature = "phase-probe")]
        let _phase = fabric_frame::probe::span("batch_envelope_decode");
        Batch::decode(entry.batch.as_slice()).map_err(|e| invalid(e.to_string()))?
    };
    let node_id: [u8; 16] = batch
        .node_id
        .as_slice()
        .try_into()
        .map_err(|_| invalid("batch node_id is not 16 bytes"))?;
    let node = entry.label.clone();
    if !batch.logs.is_empty() {
        let request = {
            #[cfg(feature = "phase-probe")]
            let _phase = fabric_frame::probe::span("otlp_logs_decode");
            ExportLogsServiceRequest::decode(batch.logs.as_slice())
                .map_err(|e| invalid(e.to_string()))?
        };
        #[cfg(feature = "phase-probe")]
        let _projection = fabric_frame::probe::span("otlp_logs_projection");
        let mut index = 0_u32;
        for resource in &request.resource_logs {
            for scope in &resource.scope_logs {
                for record in &scope.log_records {
                    let body = match record.body.as_ref().and_then(|b| b.value.as_ref()) {
                        Some(any_value::Value::StringValue(s)) => s.clone(),
                        _ => String::new(),
                    };
                    out.logs.push(LogRow {
                        group,
                        node: node.clone(),
                        node_id,
                        sequence: batch.sequence,
                        index,
                        observed_ns: record.observed_time_unix_nano,
                        body,
                        attributes: legacy_strings(&record.attributes),
                    });
                    index += 1;
                }
            }
        }
    }
    if !batch.metrics.is_empty() {
        let request = {
            #[cfg(feature = "phase-probe")]
            let _phase = fabric_frame::probe::span("otlp_metrics_decode");
            ExportMetricsServiceRequest::decode(batch.metrics.as_slice())
                .map_err(|e| invalid(e.to_string()))?
        };
        #[cfg(feature = "phase-probe")]
        let _projection = fabric_frame::probe::span("otlp_metrics_projection");
        let mut index = 0_u32;
        for resource in &request.resource_metrics {
            for scope in &resource.scope_metrics {
                for metric in &scope.metrics {
                    let (points, sum, monotonic) = match &metric.data {
                        Some(metric::Data::Gauge(g)) => (&g.data_points, false, false),
                        Some(metric::Data::Sum(s)) => (&s.data_points, true, s.is_monotonic),
                        _ => continue,
                    };
                    for point in points {
                        let value = match point.value {
                            Some(number_data_point::Value::AsInt(v)) => Number::Int(v),
                            Some(number_data_point::Value::AsDouble(v)) => Number::Double(v),
                            // One row per data point; an absent value reads as 0.
                            None => Number::Int(0),
                        };
                        out.metrics.push(MetricRow {
                            group,
                            node: node.clone(),
                            node_id,
                            sequence: batch.sequence,
                            index,
                            name: metric.name.clone(),
                            unit: metric.unit.clone(),
                            sum,
                            monotonic,
                            time_ns: point.time_unix_nano,
                            start_ns: if sum { point.start_time_unix_nano } else { 0 },
                            value,
                            attributes: legacy_strings(&point.attributes),
                        });
                        index += 1;
                    }
                }
            }
        }
    }
    if !batch.traces.is_empty() {
        let request = {
            #[cfg(feature = "phase-probe")]
            let _phase = fabric_frame::probe::span("otlp_traces_decode");
            ExportTraceServiceRequest::decode(batch.traces.as_slice())
                .map_err(|e| invalid(e.to_string()))?
        };
        #[cfg(feature = "phase-probe")]
        let _projection = fabric_frame::probe::span("otlp_traces_projection");
        let mut index = 0_u32;
        for resource in &request.resource_spans {
            for scope in &resource.scope_spans {
                for span in &scope.spans {
                    out.spans.push(SpanRow {
                        group,
                        node: node.clone(),
                        node_id,
                        sequence: batch.sequence,
                        index,
                        trace_id: legacy_hex(&span.trace_id),
                        span_id: legacy_hex(&span.span_id),
                        parent_span_id: legacy_hex(&span.parent_span_id),
                        name: span.name.clone(),
                        kind: span.kind,
                        status: span.status.as_ref().map_or(0, |s| s.code),
                        start_ns: span.start_time_unix_nano,
                        end_ns: span.end_time_unix_nano,
                        attributes: legacy_strings(&span.attributes),
                    });
                    index += 1;
                }
            }
        }
    }
    for text in &batch.collection_gaps {
        out.gaps.push(GapRow {
            group,
            node: node.clone(),
            node_id,
            sequence: batch.sequence,
            received_ns: entry.received_unix_nano,
            text: text.clone(),
        });
    }
    Ok(())
}

use opentelemetry_proto::tonic::common::v1::{AnyValue, ArrayValue, KeyValueList};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, Sum,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, Status};
use std::hint::black_box;
use std::time::Instant;

fn value(value: any_value::Value) -> AnyValue {
    AnyValue { value: Some(value) }
}
fn kv(key: &str, content: Option<any_value::Value>) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: content.map(value),
        ..Default::default()
    }
}
fn string(content: &str) -> any_value::Value {
    any_value::Value::StringValue(content.into())
}
fn request(records: Vec<LogRecord>) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![kv("ignored-resource", Some(string("ignored")))],
                ..Default::default()
            }),
            scope_logs: vec![ScopeLogs {
                log_records: records,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}
fn entry(batch: &Batch) -> Entry {
    Entry {
        label: "ownership-node".into(),
        received_unix_nano: 123456,
        batch: batch.encode_to_vec(),
    }
}
fn mixed_batch() -> Batch {
    let bodies = vec![
        Some(string("héllo🦀")),
        Some(any_value::Value::BoolValue(true)),
        Some(any_value::Value::IntValue(i64::MIN)),
        Some(any_value::Value::DoubleValue(-0.0)),
        Some(any_value::Value::ArrayValue(ArrayValue {
            values: vec![value(string("nested"))],
        })),
        Some(any_value::Value::KvlistValue(KeyValueList {
            values: vec![kv("nested", Some(string("value")))],
        })),
        Some(any_value::Value::BytesValue(vec![0, 255])),
        None,
    ];
    let attributes = vec![
        kv("repeat", Some(string("first"))),
        kv("repeat", Some(any_value::Value::IntValue(1))),
        kv("repeat", Some(string("last"))),
        kv("repeat", None),
        kv("repeat", Some(any_value::Value::BoolValue(false))),
        kv("nonstring", Some(any_value::Value::BytesValue(vec![255]))),
        kv("", Some(string("empty-key"))),
        kv("unicode", Some(string("λ🦀"))),
        KeyValue {
            key: "empty-any".into(),
            value: Some(AnyValue { value: None }),
            ..Default::default()
        },
    ];
    let mut logs = request(
        bodies
            .into_iter()
            .enumerate()
            .map(|(index, body)| LogRecord {
                observed_time_unix_nano: 77 + index as u64,
                body: body.map(value),
                attributes: attributes.clone(),
                ..Default::default()
            })
            .collect(),
    );
    logs.resource_logs[0].scope_logs.push(ScopeLogs {
        log_records: vec![LogRecord {
            body: Some(AnyValue { value: None }),
            observed_time_unix_nano: 100,
            ..Default::default()
        }],
        ..Default::default()
    });
    logs.resource_logs.push(ResourceLogs {
        scope_logs: vec![ScopeLogs {
            log_records: vec![LogRecord {
                body: Some(value(string("last-resource"))),
                observed_time_unix_nano: 101,
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    });
    let numbers = vec![
        Some(number_data_point::Value::AsInt(i64::MIN)),
        Some(number_data_point::Value::AsInt(i64::MAX)),
        Some(number_data_point::Value::AsInt((1_i64 << 53) + 1)),
        Some(number_data_point::Value::AsDouble(-0.0)),
        Some(number_data_point::Value::AsDouble(f64::from_bits(
            0x7ff8_0000_0000_0042,
        ))),
        Some(number_data_point::Value::AsDouble(f64::INFINITY)),
        None,
    ];
    let points = numbers
        .into_iter()
        .enumerate()
        .map(|(index, value)| NumberDataPoint {
            value,
            time_unix_nano: 200 + index as u64,
            start_time_unix_nano: 50,
            attributes: attributes.clone(),
            ..Default::default()
        })
        .collect();
    let metrics = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![
                    Metric {
                        name: "sum".into(),
                        unit: "By".into(),
                        data: Some(metric::Data::Sum(Sum {
                            data_points: points,
                            is_monotonic: true,
                            ..Default::default()
                        })),
                        ..Default::default()
                    },
                    Metric {
                        name: "unsupported".into(),
                        data: None,
                        ..Default::default()
                    },
                    Metric {
                        name: "gauge".into(),
                        unit: "1".into(),
                        data: Some(metric::Data::Gauge(Gauge {
                            data_points: vec![NumberDataPoint {
                                value: Some(number_data_point::Value::AsDouble(-0.0)),
                                time_unix_nano: 300,
                                start_time_unix_nano: 999,
                                ..Default::default()
                            }],
                        })),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let traces = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: vec![
                    Span {
                        trace_id: vec![0, 255, 16],
                        span_id: vec![1, 0],
                        parent_span_id: vec![],
                        name: "span-λ".into(),
                        kind: 99,
                        status: Some(Status {
                            code: 42,
                            ..Default::default()
                        }),
                        start_time_unix_nano: 400,
                        end_time_unix_nano: 401,
                        attributes,
                        ..Default::default()
                    },
                    Span {
                        name: "absent-status".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    Batch {
        version: 1,
        node_id: vec![9; 16],
        generation: 1,
        sequence: 7,
        logs: logs.encode_to_vec(),
        metrics: metrics.encode_to_vec(),
        traces: traces.encode_to_vec(),
        collection_gaps: vec!["gap-first".into(), "gap-last".into()],
        ..Default::default()
    }
}

fn metric_bits(rows: &[MetricRow]) -> Vec<serde_json::Value> {
    rows.iter()
        .map(|row| serde_json::to_value(row).unwrap())
        .collect()
}
fn same_rows(actual: &Rows, expected: &Rows) -> bool {
    actual.logs==expected.logs && actual.spans==expected.spans && actual.gaps==expected.gaps
        // Number's serializer stores Double.to_bits(); ordinary PartialEq
        // would reject identical NaNs and equate +0 with -0.
        && metric_bits(&actual.metrics)==metric_bits(&expected.metrics)
}
fn prepopulated() -> Rows {
    let mut out = Rows::default();
    legacy_extract(90, &entry(&mixed_batch()), &mut out).unwrap();
    out
}

#[test]
fn owned_logs_match_frozen_all_signal_projection_and_literal_semantics() {
    let fixture = entry(&mixed_batch());
    let mut candidate = Rows::default();
    let mut legacy = Rows::default();
    extract(4, &fixture, &mut candidate).unwrap();
    legacy_extract(4, &fixture, &mut legacy).unwrap();
    assert!(same_rows(&candidate, &legacy));
    assert_eq!(candidate.logs.len(), 10);
    assert_eq!(candidate.logs[0].body, "héllo🦀");
    assert!(candidate.logs[1..9].iter().all(|row| row.body.is_empty()));
    assert_eq!(candidate.logs[9].body, "last-resource");
    for (index, row) in candidate.logs.iter().enumerate() {
        assert_eq!(row.index, index as u32);
        assert_eq!(row.sequence, 7);
        assert_eq!(row.group, 4);
        assert!(!row.attributes.contains_key("ignored-resource"));
    }
    assert_eq!(
        candidate.logs[0].attributes,
        BTreeMap::from([
            ("repeat".into(), "last".into()),
            ("".into(), "empty-key".into()),
            ("unicode".into(), "λ🦀".into())
        ])
    );
    assert_eq!(candidate.metrics[3].value, Number::Double(-0.0));
    assert_eq!(candidate.metrics[0].value, Number::Int(i64::MIN));
    assert_eq!(candidate.metrics[1].value, Number::Int(i64::MAX));
    assert_eq!(candidate.metrics[2].value, Number::Int((1_i64 << 53) + 1));
    match candidate.metrics[4].value {
        Number::Double(number) => assert_eq!(number.to_bits(), 0x7ff8_0000_0000_0042),
        _ => panic!("wrong number variant"),
    }
    assert_eq!(candidate.metrics[6].value, Number::Int(0));
    assert_eq!(candidate.metrics[7].start_ns, 0);
    assert_eq!(candidate.spans[0].trace_id, "00ff10");
    assert_eq!(candidate.spans[1].status, 0);
    let mut mutation = Rows::default();
    legacy_extract(4, &fixture, &mut mutation).unwrap();
    mutation.logs[0]
        .attributes
        .insert("repeat".into(), "first".into());
    assert!(
        !same_rows(&mutation, &candidate),
        "first-duplicate-wins mutation must fail"
    );
    mutation.logs = candidate.logs.clone();
    mutation.logs[0].body = "non-string-converted".into();
    assert!(!same_rows(&mutation, &candidate), "altered body must fail");
    mutation.logs = candidate.logs.clone();
    mutation.metrics[3].value = Number::Double(0.0);
    assert!(
        !same_rows(&mutation, &candidate),
        "signed-zero bit mutation must fail"
    );
}

#[test]
fn owned_logs_preserve_legacy_string_reference_handling() {
    // Review found the pinned OTLP type also carries dictionary references.
    // Projection has no dictionary resolver: preserve the legacy literal-key,
    // StringValue-only rule rather than interpreting indices as text.
    let logs = request(vec![LogRecord {
        body: Some(value(any_value::Value::StringValueStrindex(7))),
        attributes: vec![
            KeyValue {
                key: "literal-key".into(),
                key_strindex: 9,
                value: Some(value(string("literal-value"))),
            },
            KeyValue {
                key: String::new(),
                key_strindex: 10,
                value: Some(value(string("empty-literal-key"))),
            },
            kv(
                "unresolved-value",
                Some(any_value::Value::StringValueStrindex(8)),
            ),
        ],
        ..Default::default()
    }]);
    let fixture = entry(&Batch {
        logs: logs.encode_to_vec(),
        metrics: Vec::new(),
        traces: Vec::new(),
        collection_gaps: Vec::new(),
        ..mixed_batch()
    });
    let mut candidate = Rows::default();
    let mut legacy = Rows::default();
    extract(4, &fixture, &mut candidate).unwrap();
    legacy_extract(4, &fixture, &mut legacy).unwrap();
    assert!(same_rows(&candidate, &legacy));
    assert_eq!(candidate.logs.len(), 1);
    assert!(candidate.logs[0].body.is_empty());
    assert_eq!(
        candidate.logs[0].attributes,
        BTreeMap::from([
            ("literal-key".into(), "literal-value".into()),
            (String::new(), "empty-literal-key".into()),
        ])
    );
}

#[test]
fn owned_logs_preserve_prepopulated_output_and_partial_decode_failures() {
    for cut in ["valid", "envelope", "node-id", "logs", "metrics", "traces"] {
        let mut batch = mixed_batch();
        match cut {
            "node-id" => batch.node_id = vec![1; 15],
            "logs" => batch.logs = vec![255],
            "metrics" => batch.metrics = vec![255],
            "traces" => batch.traces = vec![255],
            _ => {}
        }
        let mut fixture = entry(&batch);
        if cut == "envelope" {
            fixture.batch = vec![255];
        }
        let mut candidate = prepopulated();
        let mut legacy = prepopulated();
        let before = (
            candidate.logs.len(),
            candidate.metrics.len(),
            candidate.spans.len(),
            candidate.gaps.len(),
        );
        let actual = extract(4, &fixture, &mut candidate);
        let expected = legacy_extract(4, &fixture, &mut legacy);
        match (actual, expected) {
            (Ok(()), Ok(())) => assert_eq!(cut, "valid"),
            (Err(actual), Err(expected)) => {
                assert_eq!(actual.kind(), expected.kind());
                assert_eq!(actual.to_string(), expected.to_string());
            }
            _ => panic!("error mismatch at {cut}"),
        }
        assert!(
            same_rows(&candidate, &legacy),
            "partial rows differ at {cut}"
        );
        let delta = (
            candidate.logs.len() - before.0,
            candidate.metrics.len() - before.1,
            candidate.spans.len() - before.2,
            candidate.gaps.len() - before.3,
        );
        assert_eq!(
            delta,
            match cut {
                "valid" => (10, 8, 2, 2),
                "metrics" => (10, 0, 0, 0),
                "traces" => (10, 8, 0, 0),
                _ => (0, 0, 0, 0),
            }
        );
    }
}

#[test]
fn owned_log_projection_transfers_allocations_and_rejects_clone_negative_control() {
    let fixture = request(vec![LogRecord {
        body: Some(value(string("allocation-body"))),
        attributes: vec![kv("allocation-key", Some(string("allocation-value")))],
        ..Default::default()
    }])
    .encode_to_vec();
    let decoded = ExportLogsServiceRequest::decode(fixture.as_slice()).unwrap();
    let record = &decoded.resource_logs[0].scope_logs[0].log_records[0];
    let body = match record.body.as_ref().unwrap().value.as_ref().unwrap() {
        any_value::Value::StringValue(body) => body.as_ptr() as usize,
        _ => unreachable!(),
    };
    let key = record.attributes[0].key.as_ptr() as usize;
    let content = match record.attributes[0]
        .value
        .as_ref()
        .unwrap()
        .value
        .as_ref()
        .unwrap()
    {
        any_value::Value::StringValue(content) => content.as_ptr() as usize,
        _ => unreachable!(),
    };
    let preserved = |rows: &Rows| {
        let (actual_key, actual_value) = rows.logs[0].attributes.first_key_value().unwrap();
        rows.logs[0].body.as_ptr() as usize == body
            && actual_key.as_ptr() as usize == key
            && actual_value.as_ptr() as usize == content
    };
    // Keep the original allocations alive while testing a deliberately cloned
    // tree: equal row contents must not hide a copy regression.
    let mut cloned = Rows::default();
    project_logs(1, "pointer-node", [1; 16], 1, decoded.clone(), &mut cloned);
    assert!(
        !preserved(&cloned),
        "cloning negative control must fail allocation identity"
    );
    let mut moved = Rows::default();
    project_logs(1, "pointer-node", [1; 16], 1, decoded, &mut moved);
    assert!(preserved(&moved));
    assert_eq!(moved.logs, cloned.logs);
}

fn benchmark_entries(body_bytes: usize, attributes: usize) -> Vec<Entry> {
    let mut seed = 42_u64;
    (0_usize..8)
        .map(|batch_index| {
            let logs = request(
                (0..128)
                    .map(|record_index| {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        let id = batch_index * 128 + record_index;
                        let prefix = format!("{id:04}:");
                        let body = prefix
                            + &char::from(b'A' + (seed % 26) as u8)
                                .to_string()
                                .repeat(body_bytes - 5);
                        LogRecord {
                            body: Some(value(string(&body))),
                            observed_time_unix_nano: 1000 + id as u64,
                            attributes: (0..attributes)
                                .map(|index| {
                                    kv(
                                        &format!("key-{index}"),
                                        Some(string(&format!("value-{index}-{}", "R".repeat(32)))),
                                    )
                                })
                                .collect(),
                            ..Default::default()
                        }
                    })
                    .collect(),
            );
            let batch = Batch {
                version: 1,
                node_id: vec![42; 16],
                generation: 1,
                sequence: (batch_index + 1) as u64,
                logs: logs.encode_to_vec(),
                ..Default::default()
            };
            batch.validate().unwrap();
            let encoded = entry(&batch);
            assert!(encoded.batch.len() <= fabric_frame::envelope::MAX_BATCH);
            encoded
        })
        .collect()
}
fn measured_extract(entries: &[Entry], owned: bool) -> (u128, usize) {
    let expected = {
        let mut rows = Rows::default();
        for entry in entries {
            legacy_extract(1, entry, &mut rows).unwrap();
        }
        rows
    };
    let mut outputs = Vec::with_capacity(20);
    let start = Instant::now();
    for _ in 0..20 {
        let mut rows = Rows::default();
        for entry in entries {
            if owned {
                extract(black_box(1), black_box(entry), &mut rows).unwrap();
            } else {
                legacy_extract(black_box(1), black_box(entry), &mut rows).unwrap();
            }
        }
        outputs.push(black_box(rows));
    }
    let elapsed = start.elapsed().as_nanos();
    for rows in &outputs {
        assert!(same_rows(rows, &expected));
        assert_eq!(rows.logs.len(), 1024);
    }
    (elapsed, outputs.iter().map(|rows| rows.logs.len()).sum())
}

#[test]
#[ignore = "registered root-only logs ownership extraction timing"]
fn logs_ownership_timing_probe() {
    for body_bytes in [16, 1024] {
        for attributes in [0, 8] {
            let entries = benchmark_entries(body_bytes, attributes);
            for pair in 1..=3 {
                for owned in if pair % 2 == 1 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    let (elapsed, rows) = measured_extract(&entries, owned);
                    println!(
                        "{}",
                        serde_json::json!({"probe":"logs_ownership","seed":42,"logs":1024,"batches":8,"logs_per_batch":128,
                        "body_bytes":body_bytes,"string_attributes_per_log":attributes,"pair":pair,"owned":owned,"repetitions":20,
                        "whole_extract_wall_ns":elapsed,"checked_output_rows":rows,"exact_rows":true,
                        "encoded_batch_bytes":entries.iter().map(|entry|entry.batch.len()).sum::<usize>(),
                        "fixture_encoding_and_comparison_outside_timing":true,"outputs_retained_until_after_timing":true,
                        "scope":"whole extraction including decode; no allocator or HTTP attribution"})
                    );
                }
            }
        }
    }
}
