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
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;

pub type Attributes = BTreeMap<String, String>;

// Conservative retained-payload admission, not allocator/RSS accounting.
// A generous per-entry allowance includes BTree node structure; the cgroup
// remains the independent whole-process enforcement boundary.
fn attribute_bytes(a: &Attributes) -> usize {
    a.iter().fold(0usize, |n, (k, v)| {
        n.saturating_add(256)
            .saturating_add(k.capacity())
            .saturating_add(v.capacity())
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
impl LogRow {
    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.node.capacity())
            .saturating_add(self.body.capacity())
            .saturating_add(attribute_bytes(&self.attributes))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Number {
    Int(i64),
    Double(#[serde(with = "float_bits")] f64),
}

// Private spill rows must preserve every represented number, including NaN
// payloads and signed zero. JSON numbers cannot represent all these values.
mod float_bits {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(value.to_bits())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
        u64::deserialize(deserializer).map(f64::from_bits)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
impl MetricRow {
    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.node.capacity())
            .saturating_add(self.name.capacity())
            .saturating_add(self.unit.capacity())
            .saturating_add(attribute_bytes(&self.attributes))
    }
}

/// One OTLP span (ADR-0025). Identities are lowercase hex of their bytes,
/// empty when absent; `kind` and `status` are the OTLP integers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
impl SpanRow {
    pub(crate) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.node.capacity())
            .saturating_add(self.name.capacity())
            .saturating_add(self.trace_id.capacity())
            .saturating_add(self.span_id.capacity())
            .saturating_add(self.parent_span_id.capacity())
            .saturating_add(attribute_bytes(&self.attributes))
    }
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

// Consuming only the decoded logs tree transfers its String allocations into
// rows. Collect retains the legacy last-string-value-wins duplicate semantics;
// missing/non-string values do not erase earlier string-valued attributes.
fn project_logs(
    group: u64,
    node: &str,
    node_id: [u8; 16],
    sequence: u64,
    request: ExportLogsServiceRequest,
    out: &mut Rows,
) {
    let mut index = 0_u32;
    for resource in request.resource_logs {
        for scope in resource.scope_logs {
            for record in scope.log_records {
                let body = match record.body.and_then(|body| body.value) {
                    Some(any_value::Value::StringValue(body)) => body,
                    _ => String::new(),
                };
                let attributes = record
                    .attributes
                    .into_iter()
                    .filter_map(|kv| match kv.value?.value? {
                        any_value::Value::StringValue(value) => Some((kv.key, value)),
                        _ => None,
                    })
                    .collect();
                out.logs.push(LogRow {
                    group,
                    node: node.to_owned(),
                    node_id,
                    sequence,
                    index,
                    observed_ns: record.observed_time_unix_nano,
                    body,
                    attributes,
                });
                index += 1;
            }
        }
    }
}

/// Newest supported observation in one raw Entry without constructing query rows.
/// Performs the same full typed envelope/node-ID/logs/metrics/traces validation,
/// in that order, as `extract`. Counts each log's observed time, Gauge/Sum point
/// time, and span start; ignores unsupported metric types. Returns `None` when
/// there are no supported signal rows (including a gaps-only Entry).
pub fn latest_observation_ns(entry: &Entry) -> io::Result<Option<u64>> {
    latest_observation_bytes(&entry.batch)
}

/// The same validated metadata projection over borrowed raw Batch bytes.
/// Avoids requiring an owned Entry at the storage visitor boundary.
pub fn latest_observation_bytes(bytes: &[u8]) -> io::Result<Option<u64>> {
    latest_selected_observation_bytes(bytes, true, true, true)
}

/// Validate the entire record but derive freshness from authorized signals only.
pub(crate) fn latest_selected_observation_bytes(
    bytes: &[u8],
    logs: bool,
    metrics: bool,
    traces: bool,
) -> io::Result<Option<u64>> {
    let times = latest_signal_observation_bytes(bytes)?;
    Ok(times
        .into_iter()
        .zip([logs, metrics, traces])
        .filter_map(|(time, selected)| selected.then_some(time).flatten())
        .max())
}

/// Validate once and retain separate maxima for logs, metric points and spans.
/// The three slots never imply authorization; callers select an allowed signal.
pub(crate) fn latest_signal_observation_bytes(bytes: &[u8]) -> io::Result<[Option<u64>; 3]> {
    let batch = Batch::decode(bytes).map_err(|error| invalid(error.to_string()))?;
    let _: [u8; 16] = batch
        .node_id
        .as_slice()
        .try_into()
        .map_err(|_| invalid("batch node_id is not 16 bytes"))?;
    let mut newest = [None; 3];
    let mut add = |signal: usize, time: u64| {
        newest[signal] = Some(newest[signal].map_or(time, |old: u64| old.max(time)));
    };
    if !batch.logs.is_empty() {
        let request = ExportLogsServiceRequest::decode(batch.logs.as_slice())
            .map_err(|error| invalid(error.to_string()))?;
        for resource in request.resource_logs {
            for scope in resource.scope_logs {
                for record in scope.log_records {
                    add(0, record.observed_time_unix_nano);
                }
            }
        }
    }
    if !batch.metrics.is_empty() {
        let request = ExportMetricsServiceRequest::decode(batch.metrics.as_slice())
            .map_err(|error| invalid(error.to_string()))?;
        for resource in request.resource_metrics {
            for scope in resource.scope_metrics {
                for metric in scope.metrics {
                    let points = match metric.data {
                        Some(metric::Data::Gauge(g)) => g.data_points,
                        Some(metric::Data::Sum(s)) => s.data_points,
                        _ => continue,
                    };
                    for point in points {
                        add(1, point.time_unix_nano);
                    }
                }
            }
        }
    }
    if !batch.traces.is_empty() {
        let request = ExportTraceServiceRequest::decode(batch.traces.as_slice())
            .map_err(|error| invalid(error.to_string()))?;
        for resource in request.resource_spans {
            for scope in resource.scope_spans {
                for span in scope.spans {
                    add(2, span.start_time_unix_nano);
                }
            }
        }
    }
    Ok(newest)
}

/// Decode one record into its rows, appending to `out`.
pub fn extract(group: u64, entry: &Entry, out: &mut Rows) -> io::Result<()> {
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
        project_logs(group, &node, node_id, batch.sequence, request, out);
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
                            attributes: strings(&point.attributes),
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

#[cfg(test)]
#[path = "rows_ownership_tests.rs"]
mod ownership_tests;

#[cfg(test)]
#[path = "rows_freshness_tests.rs"]
mod freshness_tests;
