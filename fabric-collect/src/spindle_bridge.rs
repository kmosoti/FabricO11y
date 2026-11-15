//! Non-blocking local Unix socket bridge (`ingest.sock`) and Spindle custody
//! poller mapping OTLP JSONL and versioned `fabric::observation` records into
//! bounded telemetry queues.

use crate::channel::BoundedTelemetryQueue;
use chrono::Utc;
use fabric::observation::{Data, Level, MetricType, Outcome, StoredObservation, decode_versioned};
use fabric::spindle::transport::read_raw;
use fabric_core::{LogRecord, MetricPoint, TraceSpanRecord};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixListener;

/// Envelope accepted on `ingest.sock` as newline-delimited JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IngestEnvelope {
    Metric(MetricPoint),
    Log(LogRecord),
    Trace(TraceSpanRecord),
}

fn outcome_str(status: Outcome) -> &'static str {
    match status {
        Outcome::Ok => "ok",
        Outcome::Error => "error",
        Outcome::Cancelled => "cancelled",
    }
}

fn level_str(level: Level) -> &'static str {
    match level {
        Level::Trace => "trace",
        Level::Debug => "debug",
        Level::Info => "info",
        Level::Warn => "warn",
        Level::Error => "error",
    }
}

fn metric_type_str(metric_type: MetricType) -> &'static str {
    match metric_type {
        MetricType::Counter => "counter",
        MetricType::Gauge => "gauge",
    }
}

/// Converts a versioned Spindle `StoredObservation` into unified `fabric-core` telemetry.
#[must_use]
pub fn observation_to_telemetry(obs: &StoredObservation, default_host: &str) -> IngestEnvelope {
    let timestamp_ms = i64::try_from(obs.timestamp_ns() / 1_000_000).unwrap_or(i64::MAX);
    let id = obs.identity();
    let span_id = format!("{}:{}", id.generation, id.sequence);
    let context_run_id = obs.context().map(|ctx| ctx.run_id.clone());

    match obs.data() {
        Data::ServiceRequest {
            service,
            operation,
            duration_ns,
            status,
            trace_id,
        } => IngestEnvelope::Trace(TraceSpanRecord {
            timestamp_ms,
            trace_id: trace_id
                .clone()
                .or_else(|| context_run_id.clone())
                .unwrap_or_else(|| span_id.clone()),
            span_id,
            parent_span_id: None,
            name: operation.clone(),
            service: service.clone(),
            duration_ms: (*duration_ns as f64) / 1_000_000.0,
            status: outcome_str(*status).to_owned(),
            attributes_json: serde_json::json!({
                "kind": "service_request",
                "producer": id.producer,
                "generation": id.generation,
                "sequence": id.sequence,
                "resource": obs.resource(),
                "context_run_id": context_run_id,
                "version": obs.version(),
            })
            .to_string(),
        }),
        Data::AgentRun {
            agent,
            run_id,
            duration_ns,
            status,
        } => IngestEnvelope::Trace(TraceSpanRecord {
            timestamp_ms,
            trace_id: run_id.clone(),
            span_id,
            parent_span_id: None,
            name: "agent_run".to_owned(),
            service: agent.clone(),
            duration_ms: (*duration_ns as f64) / 1_000_000.0,
            status: outcome_str(*status).to_owned(),
            attributes_json: serde_json::json!({
                "kind": "agent_run",
                "producer": id.producer,
                "generation": id.generation,
                "sequence": id.sequence,
                "resource": obs.resource(),
                "run_id": run_id,
                "context_run_id": context_run_id,
                "version": obs.version(),
            })
            .to_string(),
        }),
        Data::ToolExecution {
            tool,
            run_id,
            duration_ns,
            status,
        } => IngestEnvelope::Trace(TraceSpanRecord {
            timestamp_ms,
            trace_id: run_id.clone(),
            span_id,
            parent_span_id: None,
            name: tool.clone(),
            service: id.producer.clone(),
            duration_ms: (*duration_ns as f64) / 1_000_000.0,
            status: outcome_str(*status).to_owned(),
            attributes_json: serde_json::json!({
                "kind": "tool_execution",
                "producer": id.producer,
                "generation": id.generation,
                "sequence": id.sequence,
                "resource": obs.resource(),
                "run_id": run_id,
                "context_run_id": context_run_id,
                "version": obs.version(),
            })
            .to_string(),
        }),
        Data::Metric {
            name,
            unit,
            value,
            metric_type,
            reset,
        } => IngestEnvelope::Metric(MetricPoint {
            timestamp_ms,
            name: name.clone(),
            value: *value,
            host: default_host.to_owned(),
            slice: "spindle".to_owned(),
            unit: obs.resource().to_owned(),
            pid: None,
            labels_json: serde_json::json!({
                "producer": id.producer,
                "generation": id.generation,
                "sequence": id.sequence,
                "metric_unit": unit,
                "metric_type": metric_type_str(*metric_type),
                "reset": reset,
                "version": obs.version(),
            })
            .to_string(),
        }),
        Data::Log { level, message } => IngestEnvelope::Log(LogRecord {
            timestamp_ms,
            level: level_str(*level).to_owned(),
            source: id.producer.clone(),
            unit: obs.resource().to_owned(),
            message: message.clone(),
            attributes_json: serde_json::json!({
                "kind": "log",
                "generation": id.generation,
                "sequence": id.sequence,
                "version": obs.version(),
            })
            .to_string(),
        }),
        Data::JsonLog { fields } => {
            let level = fields
                .get("level")
                .and_then(|v| v.as_str())
                .unwrap_or("info")
                .to_owned();
            let message = fields
                .get("message")
                .or_else(|| fields.get("msg"))
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| serde_json::Value::Object(fields.clone()).to_string());
            let attributes_json = serde_json::to_string(fields).unwrap_or_else(|_| "{}".to_owned());
            IngestEnvelope::Log(LogRecord {
                timestamp_ms,
                level,
                source: id.producer.clone(),
                unit: obs.resource().to_owned(),
                message,
                attributes_json,
            })
        }
        Data::Event { name, message } => IngestEnvelope::Log(LogRecord {
            timestamp_ms,
            level: "info".to_owned(),
            source: id.producer.clone(),
            unit: obs.resource().to_owned(),
            message: format!("{name}: {message}"),
            attributes_json: serde_json::json!({
                "kind": "event",
                "name": name,
                "generation": id.generation,
                "sequence": id.sequence,
                "version": obs.version(),
            })
            .to_string(),
        }),
        Data::Gap { source, reason } => IngestEnvelope::Log(LogRecord {
            timestamp_ms,
            level: "warn".to_owned(),
            source: id.producer.clone(),
            unit: obs.resource().to_owned(),
            message: format!("gap({source}): {reason}"),
            attributes_json: serde_json::json!({
                "kind": "gap",
                "source": source,
                "generation": id.generation,
                "sequence": id.sequence,
                "version": obs.version(),
            })
            .to_string(),
        }),
    }
}

/// Parses either a direct `IngestEnvelope` JSON line or a versioned `StoredObservation`.
pub fn parse_ingest_line(line: &str, default_host: &str) -> Result<IngestEnvelope, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err("empty ingest line".into());
    }
    if let Ok(env) = serde_json::from_str::<IngestEnvelope>(trimmed) {
        return Ok(env);
    }
    let obs = decode_versioned(trimmed.as_bytes())?;
    Ok(observation_to_telemetry(&obs, default_host))
}

/// Incrementally polls committed custody `(id, envelope)` records from `spindle.sock`
/// strictly after `after_id` within an overall `250ms` poll budget.
pub fn poll_spindle_custody_records(
    spindle_sock: &Path,
    after_id: u64,
    max_records: usize,
    default_host: &str,
) -> Result<Vec<(u64, IngestEnvelope)>, String> {
    let mut cursor = after_id;
    let mut out = Vec::new();
    if max_records == 0 {
        return Ok(out);
    }

    let overall_deadline = Instant::now() + Duration::from_millis(250);
    while out.len() < max_records && Instant::now() < overall_deadline {
        let batch = read_raw(spindle_sock, cursor, overall_deadline)?;
        if batch.is_empty() {
            break;
        }
        let mut advanced = false;
        for (id, bytes) in batch {
            if id <= cursor {
                continue;
            }
            cursor = id;
            advanced = true;
            let envelope = match decode_versioned(&bytes) {
                Ok(obs) => observation_to_telemetry(&obs, default_host),
                Err(err) => IngestEnvelope::Log(LogRecord {
                    timestamp_ms: Utc::now().timestamp_millis(),
                    level: "error".to_owned(),
                    source: "spindle-bridge".to_owned(),
                    unit: "spindle.service".to_owned(),
                    message: format!("failed to decode spindle record {id}: {err}"),
                    attributes_json: serde_json::json!({
                        "kind": "decode_error",
                        "spindle_id": id,
                        "byte_len": bytes.len(),
                    })
                    .to_string(),
                }),
            };
            out.push((id, envelope));
            if out.len() >= max_records {
                break;
            }
        }
        if !advanced {
            break;
        }
    }

    Ok(out)
}

/// Incrementally polls committed custody records from `spindle.sock` strictly after `after_id`.
pub fn poll_spindle_custody(
    spindle_sock: &Path,
    after_id: u64,
    max_records: usize,
    default_host: &str,
) -> Result<(u64, Vec<IngestEnvelope>), String> {
    let records = poll_spindle_custody_records(spindle_sock, after_id, max_records, default_host)?;
    let next_cursor = records
        .last()
        .map(|(id, _)| (*id).max(after_id))
        .unwrap_or(after_id);
    let envelopes = records.into_iter().map(|(_, env)| env).collect();
    Ok((next_cursor, envelopes))
}

/// Spawns the background local OTLP/Spindle ingest listener using bounded `try_emit`.
pub async fn serve_ingest_socket(
    sock_path: &Path,
    metrics_q: BoundedTelemetryQueue<MetricPoint>,
    logs_q: BoundedTelemetryQueue<LogRecord>,
    traces_q: BoundedTelemetryQueue<TraceSpanRecord>,
    default_host: String,
) -> Result<(), String> {
    if let Some(parent) = sock_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if sock_path.exists() {
        let _ = fs::remove_file(sock_path);
    }
    let listener = UnixListener::bind(sock_path)
        .map_err(|e| format!("failed binding {}: {e}", sock_path.display()))?;

    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let mq = metrics_q.clone();
        let lq = logs_q.clone();
        let tq = traces_q.clone();
        let host = default_host.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stream).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if line.len() > 64 * 1024 || line.trim().is_empty() {
                    continue;
                }
                if let Ok(env) = parse_ingest_line(&line, &host) {
                    match env {
                        IngestEnvelope::Metric(mut pt) => {
                            if pt.timestamp_ms <= 0 {
                                pt.timestamp_ms = Utc::now().timestamp_millis();
                            }
                            let _ = mq.try_emit(pt);
                        }
                        IngestEnvelope::Log(mut log) => {
                            if log.timestamp_ms <= 0 {
                                log.timestamp_ms = Utc::now().timestamp_millis();
                            }
                            let _ = lq.try_emit(log);
                        }
                        IngestEnvelope::Trace(mut span) => {
                            if span.timestamp_ms <= 0 {
                                span.timestamp_ms = Utc::now().timestamp_millis();
                            }
                            let _ = tq.try_emit(span);
                        }
                    }
                }
            }
        });
    }
}
