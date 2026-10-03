//! Query rows derived from one committed record (retained-history contract).
//!
//! A record is one journal entry: the submitting label, the server receive
//! time and the node's exact batch bytes. Log rows and metric points are
//! numbered by their position in the batch's request, across all resource
//! and scope groups, so every row has a total order key.

use crate::store::Entry;
use fabric_frame::envelope::Batch;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{KeyValue, any_value};
use opentelemetry_proto::tonic::metrics::v1::{metric, number_data_point};
use prost::Message;
use std::collections::BTreeMap;
use std::io;

pub type Attributes = BTreeMap<String, String>;

#[derive(Clone, Debug, PartialEq)]
pub struct LogRow {
    pub group: u64,
    pub node: String,
    pub node_id: [u8; 16],
    pub sequence: u64,
    pub index: u32,
    pub observed_ns: u64,
    pub body: String,
    pub attributes: Attributes,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Number {
    Int(i64),
    Double(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MetricRow {
    pub group: u64,
    pub node: String,
    pub node_id: [u8; 16],
    pub sequence: u64,
    pub index: u32,
    pub name: String,
    pub unit: String,
    /// `false` for a gauge.
    pub sum: bool,
    pub monotonic: bool,
    pub time_ns: u64,
    pub start_ns: u64,
    pub value: Number,
    pub attributes: Attributes,
}

/// One OTLP span (ADR-0025). Identities are lowercase hex of their bytes,
/// empty when absent; `kind` and `status` are the OTLP integers.
#[derive(Clone, Debug, PartialEq)]
pub struct SpanRow {
    pub group: u64,
    pub node: String,
    pub node_id: [u8; 16],
    pub sequence: u64,
    pub index: u32,
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: String,
    pub name: String,
    pub kind: i32,
    pub status: i32,
    pub start_ns: u64,
    pub end_ns: u64,
    pub attributes: Attributes,
}

/// Lowercase hex of bytes.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct GapRow {
    pub group: u64,
    pub node: String,
    pub node_id: [u8; 16],
    pub sequence: u64,
    pub received_ns: u64,
    pub text: String,
}

#[derive(Default)]
pub struct Rows {
    pub logs: Vec<LogRow>,
    pub metrics: Vec<MetricRow>,
    pub gaps: Vec<GapRow>,
    pub spans: Vec<SpanRow>,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// String-valued attributes only; other value types are not queryable.
fn strings(attributes: &[KeyValue]) -> Attributes {
    attributes
        .iter()
        .filter_map(|kv| match kv.value.as_ref()?.value.as_ref()? {
            any_value::Value::StringValue(s) => Some((kv.key.clone(), s.clone())),
            _ => None,
        })
        .collect()
}

/// Decode one record into its rows, appending to `out`.
pub fn extract(group: u64, entry: &Entry, out: &mut Rows) -> io::Result<()> {
    let batch = Batch::decode(entry.batch.as_slice()).map_err(|e| invalid(e.to_string()))?;
    let node_id: [u8; 16] = batch
        .node_id
        .as_slice()
        .try_into()
        .map_err(|_| invalid("batch node_id is not 16 bytes"))?;
    let node = entry.label.clone();
    if !batch.logs.is_empty() {
        let request = ExportLogsServiceRequest::decode(batch.logs.as_slice())
            .map_err(|e| invalid(e.to_string()))?;
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
                        attributes: strings(&record.attributes),
                    });
                    index += 1;
                }
            }
        }
    }
    if !batch.metrics.is_empty() {
        let request = ExportMetricsServiceRequest::decode(batch.metrics.as_slice())
            .map_err(|e| invalid(e.to_string()))?;
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
                            attributes: strings(&point.attributes),
                        });
                        index += 1;
                    }
                }
            }
        }
    }
    if !batch.traces.is_empty() {
        let request = ExportTraceServiceRequest::decode(batch.traces.as_slice())
            .map_err(|e| invalid(e.to_string()))?;
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
                        trace_id: hex(&span.trace_id),
                        span_id: hex(&span.span_id),
                        parent_span_id: hex(&span.parent_span_id),
                        name: span.name.clone(),
                        kind: span.kind,
                        status: span.status.as_ref().map_or(0, |s| s.code),
                        start_ns: span.start_time_unix_nano,
                        end_ns: span.end_time_unix_nano,
                        attributes: strings(&span.attributes),
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
