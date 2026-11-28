//! Non-blocking local Unix socket bridge (`ingest.sock`) and Spindle custody
//! poller mapping Fabric-local JSONL and versioned `fabric::observation` records into
//! bounded telemetry queues.

use crate::channel::BoundedTelemetryQueue;
use chrono::Utc;
use fabric::observation::{
    Data, Level, MetricType, Outcome, StoredObservation, VERSION_V5, decode_versioned,
};
use fabric::spindle::transport::{read_origin, read_raw_batch, read_raw_for_origin_batch};
use fabric::spindle::wire::MAX_READ_PAGE;
use fabric_core::{LogRecord, MetricPoint, SpindleProvenance, TraceSpanRecord};
use fabric_store::CatalogStore;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::sync::Semaphore;

const MAX_INGEST_LINE_BYTES: usize = 64 * 1024;
const MAX_CONCURRENT_INGEST_CONNS: usize = 12;
const INGEST_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Cancellation-safe accumulator state for [`read_bounded_line_cancel_safe`].
#[derive(Debug, Default)]
pub(crate) struct BoundedLineState {
    buf: Vec<u8>,
    overlong: bool,
    saw_any: bool,
}

/// Cancellation-safe bounded line reader whose partial frame buffer lives in `state`.
/// Returns `Ok(None)` on clean EOF with no buffered bytes, `Ok(Some(None))` when a line exceeded
/// `max_bytes` (drained to `\n`), or `Ok(Some(Some(line)))` for a valid bounded line.
pub(crate) async fn read_bounded_line_cancel_safe<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    state: &mut BoundedLineState,
    max_bytes: usize,
) -> std::io::Result<Option<Option<String>>> {
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            let finished = std::mem::take(state);
            if !finished.saw_any {
                return Ok(None);
            }
            if finished.overlong {
                return Ok(Some(None));
            }
            let line = String::from_utf8(finished.buf)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            return Ok(Some(Some(line)));
        }
        state.saw_any = true;
        if let Some(pos) = chunk.iter().position(|&b| b == b'\n') {
            if !state.overlong {
                if state.buf.len().saturating_add(pos) > max_bytes {
                    state.overlong = true;
                    state.buf.clear();
                } else {
                    state.buf.extend_from_slice(&chunk[..pos]);
                }
            }
            reader.consume(pos + 1);
            let mut finished = std::mem::take(state);
            if finished.overlong {
                return Ok(Some(None));
            }
            if finished.buf.last() == Some(&b'\r') {
                finished.buf.pop();
            }
            let line = String::from_utf8(finished.buf)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            return Ok(Some(Some(line)));
        }
        let len = chunk.len();
        if !state.overlong {
            if state.buf.len().saturating_add(len) > max_bytes {
                state.overlong = true;
                state.buf.clear();
            } else {
                state.buf.extend_from_slice(chunk);
            }
        }
        reader.consume(len);
    }
}

/// Reads one newline-delimited frame without allocating more than `max_bytes` before `\n`.
/// Returns `Ok(None)` on EOF, `Ok(Some(None))` when a line exceeded `max_bytes` (drained to `\n`),
/// or `Ok(Some(Some(line)))` for a valid bounded line.
pub(crate) async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> std::io::Result<Option<Option<String>>> {
    let mut state = BoundedLineState::default();
    read_bounded_line_cancel_safe(reader, &mut state, max_bytes).await
}

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
    let span_id = obs
        .span()
        .map(|span| span.span_id.clone())
        .unwrap_or_else(|| format!("{}:{}", id.generation, id.sequence));
    let context_run_id = obs.context().map(|ctx| ctx.run_id.clone());
    let with_run_context = |mut attributes: serde_json::Value| {
        if let Some(run_id) = context_run_id.as_deref() {
            attributes["context_run_id"] = serde_json::json!(run_id);
        }
        attributes.to_string()
    };

    match obs.data() {
        Data::ServiceRequest {
            service,
            operation,
            duration_ns,
            status,
            trace_id,
        } => IngestEnvelope::Trace(TraceSpanRecord {
            timestamp_ms,
            trace_id: obs
                .span()
                .map(|span| span.trace_id.clone())
                .or_else(|| trace_id.clone())
                .or_else(|| context_run_id.clone())
                .unwrap_or_else(|| span_id.clone()),
            span_id,
            parent_span_id: obs.span().and_then(|span| span.parent_span_id.clone()),
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
            trace_id: obs
                .span()
                .map(|span| span.trace_id.clone())
                .unwrap_or_else(|| run_id.clone()),
            span_id,
            parent_span_id: obs.span().and_then(|span| span.parent_span_id.clone()),
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
            trace_id: obs
                .span()
                .map(|span| span.trace_id.clone())
                .unwrap_or_else(|| run_id.clone()),
            span_id,
            parent_span_id: obs.span().and_then(|span| span.parent_span_id.clone()),
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
        } => {
            let mut metadata = serde_json::json!({
                "producer": id.producer,
                "generation": id.generation,
                "sequence": id.sequence,
                "metric_unit": unit,
                "metric_type": metric_type_str(*metric_type),
                "reset": reset,
                "version": obs.version(),
            });
            if let Some(observation) = obs.metric_v5() {
                metadata["labels"] = serde_json::json!(&observation.data.labels);
            }
            IngestEnvelope::Metric(MetricPoint {
                timestamp_ms,
                name: name.clone(),
                value: *value,
                host: default_host.to_owned(),
                slice: "spindle".to_owned(),
                unit: obs.resource().to_owned(),
                pid: None,
                labels_json: with_run_context(metadata),
            })
        }
        Data::Log { level, message } => IngestEnvelope::Log(LogRecord {
            timestamp_ms,
            level: level_str(*level).to_owned(),
            source: id.producer.clone(),
            unit: obs.resource().to_owned(),
            message: message.clone(),
            attributes_json: with_run_context(serde_json::json!({
                "kind": "log",
                "generation": id.generation,
                "sequence": id.sequence,
                "version": obs.version(),
            })),
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
            attributes_json: with_run_context(serde_json::json!({
                "kind": "event",
                "name": name,
                "generation": id.generation,
                "sequence": id.sequence,
                "version": obs.version(),
            })),
        }),
        Data::Gap { source, reason } => IngestEnvelope::Log(LogRecord {
            timestamp_ms,
            level: "warn".to_owned(),
            source: id.producer.clone(),
            unit: obs.resource().to_owned(),
            message: format!("gap({source}): {reason}"),
            attributes_json: with_run_context(serde_json::json!({
                "kind": "gap",
                "source": source,
                "generation": id.generation,
                "sequence": id.sequence,
                "version": obs.version(),
            })),
        }),
    }
}

// Inspect every decoded top-level version field. A duplicate cannot hide a
// custody claim by overwriting it in a JSON value map.
fn claims_v5_spindle_projection(labels: &str) -> bool {
    struct VersionProbe;
    impl<'de> serde::de::Visitor<'de> for VersionProbe {
        type Value = bool;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("metric metadata object")
        }

        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut fields: M,
        ) -> Result<Self::Value, M::Error> {
            let mut claims_v5 = false;
            while let Some(field) = fields.next_key::<String>()? {
                if field == "version" {
                    let value = fields.next_value::<serde_json::Value>()?;
                    claims_v5 |= value.as_u64() == Some(u64::from(VERSION_V5));
                } else {
                    fields.next_value::<serde::de::IgnoredAny>()?;
                }
            }
            Ok(claims_v5)
        }
    }
    serde::Deserializer::deserialize_map(
        &mut serde_json::Deserializer::from_str(labels),
        VersionProbe,
    )
    .unwrap_or(false)
}

/// Parses either a direct `IngestEnvelope` JSON line or a versioned `StoredObservation`.
pub fn parse_ingest_line(line: &str, default_host: &str) -> Result<IngestEnvelope, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err("empty ingest line".into());
    }
    // Probe only the small tag. Unknown values are skipped without building the
    // internally tagged enum's owned intermediate JSON content.
    #[derive(Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum DirectKind {
        Metric,
        Log,
        Trace,
    }
    #[derive(Deserialize)]
    struct DirectTag {
        kind: Option<DirectKind>,
    }
    let direct = match serde_json::from_str::<DirectTag>(trimmed) {
        Ok(DirectTag {
            kind: Some(DirectKind::Metric),
        }) => serde_json::from_str::<MetricPoint>(trimmed)
            .ok()
            .map(IngestEnvelope::Metric),
        Ok(DirectTag {
            kind: Some(DirectKind::Log),
        }) => serde_json::from_str::<LogRecord>(trimmed)
            .ok()
            .map(IngestEnvelope::Log),
        Ok(DirectTag {
            kind: Some(DirectKind::Trace),
        }) => serde_json::from_str::<TraceSpanRecord>(trimmed)
            .ok()
            .map(IngestEnvelope::Trace),
        _ => None,
    };
    if let Some(env) = direct {
        if let IngestEnvelope::Metric(point) = &env
            && point.slice == "spindle"
            && claims_v5_spindle_projection(&point.labels_json)
        {
            return Err("v5 Spindle metric projection requires custody".into());
        }
        return Ok(env);
    }
    let obs = decode_versioned(trimmed.as_bytes())?;
    if obs.metric_v5().is_some() {
        return Err("v5 Spindle metric projection requires custody".into());
    }
    Ok(observation_to_telemetry(&obs, default_host))
}

/// A contiguous verified prefix and an optional failure at its next record.
#[derive(Debug)]
pub struct SpindleProjectionPoll {
    pub records: Vec<(SpindleProvenance, IngestEnvelope)>,
    pub tail_error: Option<String>,
}

/// Polls a single origin and binds it under the caller's existing OLAP writer lock.
pub fn poll_spindle_projection_records(
    spindle_sock: &Path,
    catalog: &CatalogStore,
    after_id: u64,
    max_records: usize,
    default_host: &str,
) -> Result<SpindleProjectionPoll, String> {
    let deadline = Instant::now() + Duration::from_millis(250);
    let origin = read_origin(spindle_sock, deadline)?;
    let page_cap = max_records.min(MAX_READ_PAGE);
    poll_spindle_projection_records_with(
        &origin,
        after_id,
        max_records,
        default_host,
        deadline,
        |cursor, deadline| {
            read_raw_for_origin_batch(spindle_sock, &origin, cursor, page_cap, deadline)
        },
        || catalog.bind_spindle_projection(&origin).map(|_| ()),
    )
}

fn poll_spindle_projection_records_with(
    origin: &str,
    mut cursor: u64,
    max_records: usize,
    default_host: &str,
    deadline: Instant,
    mut read_batch: impl FnMut(u64, Instant) -> Result<Vec<(u64, Vec<u8>)>, String>,
    bind: impl FnOnce() -> Result<(), String>,
) -> Result<SpindleProjectionPoll, String> {
    let mut records = Vec::new();
    let mut bind = Some(bind);
    let mut tail_error = None;
    'outer: while records.len() < max_records {
        if Instant::now() >= deadline {
            if records.is_empty() {
                return Err(
                    "Spindle projection poll deadline expired before the first read".into(),
                );
            }
            break;
        }
        let page = match read_batch(cursor, deadline) {
            Ok(page) => page,
            Err(error) if records.is_empty() => return Err(error),
            Err(error) => {
                tail_error = Some(error);
                break;
            }
        };
        if page.len() > MAX_READ_PAGE {
            let err = format!(
                "Spindle returned {} records exceeding page limit {MAX_READ_PAGE}",
                page.len()
            );
            if records.is_empty() {
                return Err(err);
            }
            tail_error = Some(err);
            break;
        }
        if page.is_empty() {
            if let Some(bind) = bind.take() {
                bind()?;
            }
            break;
        }
        for (id, raw) in page {
            if cursor.checked_add(1) != Some(id) {
                let err = format!("non-contiguous Spindle record {id} after {cursor}");
                if records.is_empty() {
                    return Err(err);
                }
                tail_error = Some(err);
                break 'outer;
            }
            let observation = match decode_versioned(&raw) {
                Ok(obs) => obs,
                Err(error) => {
                    let err = format!("cannot decode Spindle record {id}: {error}");
                    if records.is_empty() {
                        return Err(err);
                    }
                    tail_error = Some(err);
                    break 'outer;
                }
            };
            if let Some(bind) = bind.take() {
                bind()?;
            }
            let identity = observation.identity();
            let key = SpindleProvenance {
                origin: origin.to_owned(),
                record_id: id,
                producer: identity.producer.clone(),
                generation: identity.generation.clone(),
                sequence: identity.sequence,
            };
            cursor = key.record_id;
            records.push((key, observation_to_telemetry(&observation, default_host)));
            if records.len() >= max_records {
                break 'outer;
            }
        }
    }
    Ok(SpindleProjectionPoll {
        records,
        tail_error,
    })
}

/// Incrementally polls committed custody `(id, envelope)` records from `spindle.sock`
/// strictly after `after_id` within an overall `250ms` poll budget.
pub fn poll_spindle_custody_records(
    spindle_sock: &Path,
    after_id: u64,
    max_records: usize,
    default_host: &str,
) -> Result<Vec<(u64, IngestEnvelope)>, String> {
    let overall_deadline = Instant::now() + Duration::from_millis(250);
    let page_cap = max_records.min(MAX_READ_PAGE);
    poll_spindle_custody_records_with(
        after_id,
        max_records,
        default_host,
        overall_deadline,
        &mut |cursor, deadline| read_raw_batch(spindle_sock, cursor, page_cap, deadline),
    )
}

fn poll_spindle_custody_records_with<F>(
    after_id: u64,
    max_records: usize,
    default_host: &str,
    overall_deadline: Instant,
    read_batch: &mut F,
) -> Result<Vec<(u64, IngestEnvelope)>, String>
where
    F: FnMut(u64, Instant) -> Result<Vec<(u64, Vec<u8>)>, String>,
{
    let mut cursor = after_id;
    let mut out = Vec::new();
    if max_records == 0 {
        return Ok(out);
    }

    while out.len() < max_records && Instant::now() < overall_deadline {
        let batch = match read_batch(cursor, overall_deadline) {
            Ok(batch) => batch,
            Err(err) => {
                if out.is_empty() {
                    return Err(err);
                }
                break;
            }
        };
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

/// Spawns the background local JSONL ingest listener using bounded `try_emit`.
pub async fn serve_ingest_socket(
    sock_path: &Path,
    metrics_q: BoundedTelemetryQueue<MetricPoint>,
    logs_q: BoundedTelemetryQueue<LogRecord>,
    traces_q: BoundedTelemetryQueue<TraceSpanRecord>,
    default_host: String,
) -> Result<(), String> {
    let (_tx, rx) = tokio::sync::watch::channel(false);
    serve_ingest_socket_until_shutdown(sock_path, metrics_q, logs_q, traces_q, default_host, rx)
        .await
}

/// Spawns the background local JSONL ingest listener and drains connection tasks on shutdown.
pub async fn serve_ingest_socket_until_shutdown(
    sock_path: &Path,
    metrics_q: BoundedTelemetryQueue<MetricPoint>,
    logs_q: BoundedTelemetryQueue<LogRecord>,
    traces_q: BoundedTelemetryQueue<TraceSpanRecord>,
    default_host: String,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<(), String> {
    let listener = crate::socket_security::bind_private_unix_listener(sock_path)?;
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_INGEST_CONNS));
    let mut conn_tasks = tokio::task::JoinSet::new();

    loop {
        while conn_tasks.try_join_next().is_some() {}
        if *shutdown_rx.borrow() {
            break;
        }
        let permit = tokio::select! {
            biased;
            _ = shutdown_rx.changed() => break,
            res = permits.clone().acquire_owned() => match res {
                Ok(p) => p,
                Err(_) => break,
            }
        };
        let stream = tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                drop(permit);
                break;
            }
            res = listener.accept() => match res {
                Ok((s, _)) => {
                    if !crate::socket_security::is_same_uid_peer(&s) {
                        drop(permit);
                        continue;
                    }
                    s
                }
                Err(_) => {
                    drop(permit);
                    continue;
                }
            }
        };
        let mq = metrics_q.clone();
        let lq = logs_q.clone();
        let tq = traces_q.clone();
        let host = default_host.clone();
        let mut conn_shutdown = shutdown_rx.clone();
        conn_tasks.spawn(async move {
            let _permit = permit;
            let mut reader = BufReader::with_capacity(8192, stream);
            let emit_line = |line: &str| {
                if line.trim().is_empty() {
                    return;
                }
                if let Ok(env) = parse_ingest_line(line, &host) {
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
            };
            let mut shutting_down = false;
            let mut line_state = BoundedLineState::default();
            loop {
                if *conn_shutdown.borrow() {
                    shutting_down = true;
                    break;
                }
                let maybe_line = tokio::select! {
                    _ = conn_shutdown.changed() => {
                        shutting_down = true;
                        break;
                    }
                    res = tokio::time::timeout(
                        INGEST_IDLE_TIMEOUT,
                        read_bounded_line_cancel_safe(
                            &mut reader,
                            &mut line_state,
                            MAX_INGEST_LINE_BYTES,
                        ),
                    ) => match res {
                        Ok(Ok(Some(line))) => line,
                        _ => break,
                    }
                };
                if let Some(line) = maybe_line {
                    emit_line(&line);
                }
            }
            if shutting_down {
                let drain_deadline =
                    tokio::time::Instant::now() + tokio::time::Duration::from_millis(150);
                let mut drained = 0usize;
                while drained < 1024 {
                    let remaining =
                        drain_deadline.saturating_duration_since(tokio::time::Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    let step_timeout = remaining.min(tokio::time::Duration::from_millis(20));
                    match tokio::time::timeout(
                        step_timeout,
                        read_bounded_line_cancel_safe(
                            &mut reader,
                            &mut line_state,
                            MAX_INGEST_LINE_BYTES,
                        ),
                    )
                    .await
                    {
                        Ok(Ok(Some(Some(line)))) => {
                            emit_line(&line);
                            drained += 1;
                        }
                        Ok(Ok(Some(None))) => {
                            drained += 1;
                        }
                        Ok(Ok(None)) | Ok(Err(_)) | Err(_) => break,
                    }
                }
            }
        });
    }
    drop(listener);
    let drain_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(250);
    while tokio::time::timeout_at(drain_deadline, conn_tasks.join_next())
        .await
        .ok()
        .flatten()
        .is_some()
    {}
    conn_tasks.abort_all();
    while conn_tasks.join_next().await.is_some() {}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric::observation::{Data, Identity, Observation, REGISTRY, VERSION};
    use std::fs;
    use tokio::io::AsyncWriteExt;

    fn sample_obs_bytes(seq: u64) -> Vec<u8> {
        let obs = Observation {
            version: VERSION,
            registry: REGISTRY.to_owned(),
            identity: Identity {
                producer: "test-producer".to_owned(),
                generation: "gen-1".to_owned(),
                sequence: seq,
            },
            timestamp_ns: 0,
            resource: "test.service".to_owned(),
            data: Data::Event {
                name: "ok".to_owned(),
                message: "hello".to_owned(),
            },
        };
        serde_json::to_vec(&obs).expect("serialize test observation")
    }

    #[test]
    fn poll_preserves_completed_prefix_when_later_read_times_out() {
        let mut calls = 0usize;
        let mut reader = |cursor: u64, _deadline: Instant| -> Result<Vec<(u64, Vec<u8>)>, String> {
            calls += 1;
            match calls {
                1 => {
                    assert_eq!(cursor, 4);
                    Ok(vec![(5, sample_obs_bytes(5))])
                }
                2 => {
                    assert_eq!(cursor, 5);
                    Err("read deadline expired".to_owned())
                }
                _ => Ok(Vec::new()),
            }
        };
        let recs = poll_spindle_custody_records_with(
            4,
            64,
            "workstation-01",
            Instant::now() + Duration::from_secs(1),
            &mut reader,
        )
        .expect("completed prefix must survive subsequent budget timeout");
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].0, 5);
        assert!(matches!(recs[0].1, IngestEnvelope::Log(_)));
    }

    #[test]
    fn poll_returns_error_when_first_read_fails_without_progress() {
        let mut reader =
            |_cursor: u64, _deadline: Instant| -> Result<Vec<(u64, Vec<u8>)>, String> {
                Err("spindle socket unreachable".to_owned())
            };
        let err = poll_spindle_custody_records_with(
            0,
            64,
            "workstation-01",
            Instant::now() + Duration::from_secs(1),
            &mut reader,
        )
        .expect_err("error without progress must propagate");
        assert!(err.contains("spindle socket unreachable"));
    }

    #[test]
    fn poll_zero_timestamp_and_decode_error_are_both_preserved() {
        let mut calls = 0usize;
        let mut reader = |cursor: u64, _deadline: Instant| -> Result<Vec<(u64, Vec<u8>)>, String> {
            calls += 1;
            match calls {
                1 => Ok(vec![(cursor + 1, sample_obs_bytes(1))]),
                2 => Ok(vec![(cursor + 1, b"not-valid-observation".to_vec())]),
                _ => Ok(Vec::new()),
            }
        };
        let recs = poll_spindle_custody_records_with(
            0,
            64,
            "workstation-01",
            Instant::now() + Duration::from_secs(1),
            &mut reader,
        )
        .expect("decode failure converts to explicit error log");
        assert_eq!(recs.len(), 2);
        match &recs[0].1 {
            IngestEnvelope::Log(l) => assert_eq!(l.timestamp_ms, 0),
            other => panic!("expected log record, got {other:?}"),
        }
        match &recs[1].1 {
            IngestEnvelope::Log(l) => {
                assert_eq!(l.level, "error");
                assert!(l.message.contains("failed to decode spindle record 2"));
            }
            other => panic!("expected decode error log, got {other:?}"),
        }
    }

    #[test]
    fn projection_poll_preserves_exact_prefix_and_reports_a_later_gap() {
        let mut bound = false;
        let result = poll_spindle_projection_records_with(
            "origin-a",
            4,
            64,
            "host",
            Instant::now() + Duration::from_secs(1),
            |cursor, _| match cursor {
                4 => Ok(vec![(5, sample_obs_bytes(11))]),
                5 => Ok(vec![(7, sample_obs_bytes(12))]),
                _ => panic!("polled beyond the contiguous prefix"),
            },
            || {
                bound = true;
                Ok(())
            },
        )
        .unwrap();
        assert!(bound);
        assert_eq!(result.records.len(), 1);
        assert_eq!(result.records[0].0.origin, "origin-a");
        assert_eq!(result.records[0].0.record_id, 5);
        assert_eq!(result.records[0].0.sequence, 11);
        assert_eq!(
            result.tail_error.as_deref(),
            Some("non-contiguous Spindle record 7 after 5")
        );
    }

    #[test]
    fn projection_poll_does_not_bind_after_failed_or_invalid_first_page() {
        let cases = [
            Err("Spindle origin mismatch".to_owned()),
            Ok(vec![(1, b"not a stored observation".to_vec())]),
            Ok(vec![(2, sample_obs_bytes(1))]),
        ];
        for page in cases {
            let mut page = Some(page);
            assert!(
                poll_spindle_projection_records_with(
                    "origin-a",
                    0,
                    64,
                    "host",
                    Instant::now() + Duration::from_secs(1),
                    |_, _| page.take().expect("exactly one read"),
                    || panic!("bound before the first verified response"),
                )
                .is_err()
            );
        }
    }

    #[test]
    fn projection_poll_binds_an_empty_store_after_verified_response() {
        let mut bound = false;
        let result = poll_spindle_projection_records_with(
            "origin-a",
            0,
            64,
            "host",
            Instant::now() + Duration::from_secs(1),
            |_, _| Ok(Vec::new()),
            || {
                bound = true;
                Ok(())
            },
        )
        .unwrap();
        assert!(bound);
        assert!(result.records.is_empty());
        assert!(result.tail_error.is_none());
    }

    #[tokio::test]
    async fn ingest_socket_drains_overlong_frame_and_accepts_subsequent_line() {
        use tokio::io::AsyncWriteExt;

        let base = std::env::var_os("TMPDIR")
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .filter(|p| !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
            .unwrap_or_else(|| std::path::PathBuf::from("/var/tmp"));
        fs::create_dir_all(&base).unwrap();
        let dir = tempfile::Builder::new()
            .prefix("spindle-bridge-")
            .tempdir_in(&base)
            .unwrap();
        let sock = dir.path().join("ingest.sock");

        let (mq, mut metrics_rx, _) = BoundedTelemetryQueue::new(16);
        let (lq, _, _) = BoundedTelemetryQueue::new(16);
        let (tq, _, _) = BoundedTelemetryQueue::new(16);

        let sock_srv = sock.clone();
        let server = tokio::spawn(async move {
            let _ = serve_ingest_socket(&sock_srv, mq, lq, tq, "workstation-01".into()).await;
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        while !sock.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let mut client = tokio::net::UnixStream::connect(&sock).await.unwrap();
        let mut overlong = vec![b'x'; 200 * 1024];
        overlong.push(b'\n');
        client.write_all(&overlong).await.unwrap();

        let valid = serde_json::to_vec(&IngestEnvelope::Metric(MetricPoint {
            timestamp_ms: 1_700_000_000_123,
            name: "post.overlong".into(),
            value: 42.0,
            host: "workstation-01".into(),
            slice: "spindle".into(),
            unit: "spindle.service".into(),
            pid: None,
            labels_json: "{}".into(),
        }))
        .unwrap();
        client.write_all(&valid).await.unwrap();
        client.write_all(b"\n").await.unwrap();
        client.flush().await.unwrap();

        let received = tokio::time::timeout(Duration::from_secs(3), metrics_rx.recv())
            .await
            .expect("valid metric after overlong frame must not time out")
            .expect("channel open");
        assert_eq!(received.name, "post.overlong");
        assert_eq!(received.value, 42.0);
        server.abort();
    }

    #[tokio::test]
    async fn serve_ingest_socket_until_shutdown_drains_accepted_connections_and_stops() {
        let base = std::env::var_os("TMPDIR")
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
            .filter(|p| !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
            .unwrap_or_else(|| {
                std::path::PathBuf::from(std::env::var("HOME").expect("HOME set"))
                    .join(".cache")
                    .join("fabrico11y-tmp")
            });
        fs::create_dir_all(&base).unwrap();
        let dir = tempfile::Builder::new()
            .prefix("spindle-bridge-shutdown-")
            .tempdir_in(&base)
            .unwrap();
        let sock = dir.path().join("ingest.sock");

        let (mq, mut metrics_rx, _) = BoundedTelemetryQueue::new(16);
        let (lq, _, _) = BoundedTelemetryQueue::new(16);
        let (tq, _, _) = BoundedTelemetryQueue::new(16);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        let sock_srv = sock.clone();
        let mut server = tokio::spawn(async move {
            serve_ingest_socket_until_shutdown(
                &sock_srv,
                mq,
                lq,
                tq,
                "workstation-01".into(),
                shutdown_rx,
            )
            .await
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        while !sock.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let mut client = tokio::net::UnixStream::connect(&sock).await.unwrap();
        let handshake = serde_json::to_vec(&IngestEnvelope::Metric(MetricPoint {
            timestamp_ms: 1_700_000_000_123,
            name: "shutdown.handshake".into(),
            value: 1.0,
            host: "workstation-01".into(),
            slice: "spindle".into(),
            unit: "spindle.service".into(),
            pid: None,
            labels_json: "{}".into(),
        }))
        .unwrap();
        // Write handshake + partial prefix of next frame (no newline yet) so the server
        // consumes the prefix into BoundedLineState and awaits more bytes when shutdown fires.
        let valid = serde_json::to_vec(&IngestEnvelope::Metric(MetricPoint {
            timestamp_ms: 1_700_000_000_456,
            name: "shutdown.drain".into(),
            value: 7.0,
            host: "workstation-01".into(),
            slice: "spindle".into(),
            unit: "spindle.service".into(),
            pid: None,
            labels_json: "{}".into(),
        }))
        .unwrap();
        let split = valid.len() / 2;
        client.write_all(&handshake).await.unwrap();
        client.write_all(b"\n").await.unwrap();
        client.write_all(&valid[..split]).await.unwrap();
        client.flush().await.unwrap();

        let ack = tokio::time::timeout(Duration::from_secs(2), metrics_rx.recv())
            .await
            .expect("handshake metric confirms connection accepted")
            .expect("channel open");
        assert_eq!(ack.name, "shutdown.handshake");

        // Let the connection task consume `valid[..split]` and park mid-frame before shutdown.
        tokio::time::sleep(Duration::from_millis(15)).await;
        shutdown_tx.send(true).unwrap();
        client.write_all(&valid[split..]).await.unwrap();
        client.write_all(b"\n").await.unwrap();
        client.flush().await.unwrap();

        let res = tokio::time::timeout(Duration::from_secs(2), &mut server)
            .await
            .expect("ingest server and active connection tasks must shut down promptly")
            .expect("task join");
        assert!(res.is_ok());
        let drained = metrics_rx
            .try_recv()
            .expect("split pre-shutdown frame must finish draining before abort");
        assert_eq!(drained.name, "shutdown.drain");
        drop(client);
    }

    #[test]
    fn v3_run_context_correlates_metrics_logs_events_and_gaps_with_trace() {
        use fabric::observation::{Identity, ObservationV3, REGISTRY_V3, VERSION_V3};
        use fabric::run::RunContext;

        let run_id = "11111111-2222-4333-8444-555555555555";
        let mk_v3 = |sequence: u64, data: Data| {
            StoredObservation::V3(ObservationV3 {
                version: VERSION_V3,
                registry: REGISTRY_V3.to_owned(),
                identity: Identity {
                    producer: "bench-worker".to_owned(),
                    generation: "gen-1".to_owned(),
                    sequence,
                },
                timestamp_ns: 1_700_000_000_000_000_000,
                resource: "worker.service".to_owned(),
                context: RunContext {
                    run_id: run_id.to_owned(),
                },
                data,
            })
        };

        let trace = observation_to_telemetry(
            &mk_v3(
                5,
                Data::ServiceRequest {
                    service: "bench-worker".to_owned(),
                    operation: "checkpoint".to_owned(),
                    duration_ns: 14_500_000,
                    status: Outcome::Ok,
                    trace_id: Some("request-trace".to_owned()),
                },
            ),
            "node-a",
        );
        let IngestEnvelope::Trace(trace) = trace else {
            panic!("expected Trace, got {trace:?}");
        };
        assert_eq!(trace.trace_id, "request-trace");
        let trace_attributes: serde_json::Value =
            serde_json::from_str(&trace.attributes_json).unwrap();
        let correlation = &trace_attributes["context_run_id"];
        assert_eq!(correlation, run_id);

        let metric = observation_to_telemetry(
            &mk_v3(
                1,
                Data::Metric {
                    name: "step.latency_ms".to_owned(),
                    unit: "ms".to_owned(),
                    value: 14.5,
                    metric_type: MetricType::Gauge,
                    reset: false,
                },
            ),
            "node-a",
        );
        match metric {
            IngestEnvelope::Metric(pt) => {
                let labels: serde_json::Value = serde_json::from_str(&pt.labels_json).unwrap();
                assert_eq!(&labels["context_run_id"], correlation);
                assert_eq!(labels["producer"], "bench-worker");
            }
            other => panic!("expected Metric, got {other:?}"),
        }

        for (seq, data, expected_kind) in [
            (
                2,
                Data::Log {
                    level: Level::Info,
                    message: "checkpoint saved".to_owned(),
                },
                "log",
            ),
            (
                3,
                Data::Event {
                    name: "stage.done".to_owned(),
                    message: "completed".to_owned(),
                },
                "event",
            ),
            (
                4,
                Data::Gap {
                    source: "collector".to_owned(),
                    reason: "ring backpressure".to_owned(),
                },
                "gap",
            ),
        ] {
            let env = observation_to_telemetry(&mk_v3(seq, data), "node-a");
            match env {
                IngestEnvelope::Log(log) => {
                    let attrs: serde_json::Value =
                        serde_json::from_str(&log.attributes_json).unwrap();
                    assert_eq!(attrs["kind"], expected_kind);
                    assert_eq!(&attrs["context_run_id"], correlation);
                }
                other => panic!("expected Log for {expected_kind}, got {other:?}"),
            }
        }
    }

    #[test]
    fn v2_log_preserves_attributes_without_run_context() {
        use fabric::observation::{Identity, Observation, REGISTRY, VERSION};

        let obs = StoredObservation::V2(Observation {
            version: VERSION,
            registry: REGISTRY.to_owned(),
            identity: Identity {
                producer: "bench-worker".to_owned(),
                generation: "gen-1".to_owned(),
                sequence: 1,
            },
            timestamp_ns: 1_700_000_000_000_000_000,
            resource: "worker.service".to_owned(),
            data: Data::Log {
                level: Level::Info,
                message: "checkpoint saved".to_owned(),
            },
        });
        match observation_to_telemetry(&obs, "node-a") {
            IngestEnvelope::Log(log) => {
                assert_eq!(
                    log.attributes_json,
                    serde_json::json!({
                        "kind": "log",
                        "generation": "gen-1",
                        "sequence": 1,
                        "version": VERSION,
                    })
                    .to_string()
                );
            }
            other => panic!("expected Log, got {other:?}"),
        }
    }

    #[test]
    fn v3_json_log_preserves_absent_and_caller_supplied_run_context_fields() {
        use fabric::observation::{Identity, ObservationV3, REGISTRY_V3, VERSION_V3};
        use fabric::run::RunContext;

        for submitted in [
            r#"{"format":"app.result.v1","message":"app.result.v1/1","status":"passed"}"#,
            r#"{"context_run_id":"caller-owned","format":"app.result.v1","message":"app.result.v1/1","status":"passed"}"#,
        ] {
            let fields = serde_json::from_str(submitted).unwrap();
            let expected_json = serde_json::to_string(&fields).unwrap();
            let obs = StoredObservation::V3(ObservationV3 {
                version: VERSION_V3,
                registry: REGISTRY_V3.to_owned(),
                identity: Identity {
                    producer: "bench-worker".to_owned(),
                    generation: "gen-1".to_owned(),
                    sequence: 9,
                },
                timestamp_ns: 1_700_000_000_000_000_000,
                resource: "worker.service".to_owned(),
                context: RunContext {
                    run_id: "11111111-2222-4333-8444-555555555555".to_owned(),
                },
                data: Data::JsonLog { fields },
            });
            match observation_to_telemetry(&obs, "node-a") {
                IngestEnvelope::Log(log) => assert_eq!(log.attributes_json, expected_json),
                other => panic!("expected Log, got {other:?}"),
            }
        }
    }
}

#[cfg(test)]
mod v4_bridge_tests {
    use super::*;
    use fabric::observation::{
        Identity, ObservationV4, Outcome, REGISTRY_V4, SpanContext, VERSION_V4, encode_v4,
    };
    use fabric::run::RunContext;
    #[test]
    fn direct_envelopes_skip_irrelevant_span_and_v4_observations_validate_raw() {
        let direct = serde_json::json!({"kind":"metric","timestamp_ms":1,"name":"name","value":1.0,"host":"host","slice":"slice","unit":"unit","pid":null,"labels_json":"{}"}).to_string();
        let IngestEnvelope::Metric(expected) =
            serde_json::from_str::<IngestEnvelope>(&direct).unwrap()
        else {
            panic!("metric");
        };
        let mut with_extra = direct.clone();
        with_extra.pop();
        with_extra.push_str(&format!(
            ",\"version\":4,\"span\":\"{}\"}}",
            "\\u0031".repeat(2000)
        ));
        let IngestEnvelope::Metric(actual) = parse_ingest_line(&with_extra, "host").unwrap() else {
            panic!("metric");
        };
        assert_eq!(actual, expected);
        let IngestEnvelope::Metric(actual) = parse_ingest_line(&direct, "host").unwrap() else {
            panic!("metric");
        };
        assert_eq!(actual, expected);
        let bad = r#"{"version":4,"registry":"fabrico11y/4","context":{"run_id":"00000000-0000-4000-8000-000000000001"},"identity":{"producer":"p","generation":"g","sequence":1},"timestamp_ns":1,"resource":"r","data":{"kind":"agent_run","agent":"a","run_id":"00000000-0000-4000-8000-000000000001","duration_ns":1,"status":"ok"},"span":{"trace_id":"\u00311111111111111111111111111111111","span_id":"2222222222222222"}}"#;
        assert!(parse_ingest_line(bad, "host").is_err());
        let oversized = bad.replace(
            "\\u00311111111111111111111111111111111",
            &"1".repeat(10_000),
        );
        assert!(parse_ingest_line(&oversized, "host").is_err());
    }

    #[test]
    fn bounded_v5_labels_project_with_custody_but_are_rejected_on_direct_ingest() {
        let raw = r#"{"version":5,"registry":"fabrico11y/5","context":{"run_id":"00000000-0000-4000-8000-000000000001"},"identity":{"producer":"probe","generation":"generation","sequence":7},"timestamp_ns":1000000,"resource":"gpu","data":{"kind":"metric","name":"stage.latency","unit":"ms","value":1.0,"metric_type":"gauge","reset":false,"labels":{"stage":"read","lane":"甲"}}}"#;
        let observation = decode_versioned(raw.as_bytes()).unwrap();
        let IngestEnvelope::Metric(projected) = observation_to_telemetry(&observation, "host")
        else {
            panic!("expected metric");
        };
        let fields: serde_json::Value = serde_json::from_str(&projected.labels_json).unwrap();
        assert_eq!(
            fields["labels"],
            serde_json::json!({"stage": "read", "lane": "甲"})
        );
        assert_eq!(fields["producer"], "probe");
        assert_eq!(fields["generation"], "generation");
        assert_eq!(fields["sequence"], 7);
        assert_eq!(fields["version"], 5);
        assert_eq!(
            fields["context_run_id"],
            "00000000-0000-4000-8000-000000000001"
        );
        assert_eq!(
            parse_ingest_line(raw, "host").unwrap_err(),
            "v5 Spindle metric projection requires custody"
        );
        for labels_json in [
            projected.labels_json.as_str(),
            r#"{"version":5,"version":3,"labels":{"lane":"甲"}}"#,
            r#"{"version":3,"ver\u0073ion":5,"labels":{"lane":"甲"}}"#,
            r#"{"ver\u0073ion":5,"version":3,"labels":{"lane":"甲"}}"#,
        ] {
            let mut direct_point = projected.clone();
            direct_point.labels_json = labels_json.to_owned();
            let direct = serde_json::to_string(&IngestEnvelope::Metric(direct_point)).unwrap();
            assert_eq!(
                parse_ingest_line(&direct, "host").unwrap_err(),
                "v5 Spindle metric projection requires custody"
            );
        }
    }

    #[test]
    fn explicit_trace_ids_and_parent_are_preserved_for_each_v4_kind() {
        let run = "00000000-0000-4000-8000-000000000001";
        let trace = "11111111111111111111111111111111";
        let span = "2222222222222222";
        let parent = "3333333333333333";
        for data in [
            Data::ServiceRequest {
                service: "svc".into(),
                operation: "operation".into(),
                duration_ns: 1,
                status: Outcome::Ok,
                trace_id: Some(trace.into()),
            },
            Data::AgentRun {
                agent: "agent".into(),
                run_id: run.into(),
                duration_ns: 1,
                status: Outcome::Ok,
            },
            Data::ToolExecution {
                tool: "tool".into(),
                run_id: run.into(),
                duration_ns: 1,
                status: Outcome::Ok,
            },
        ] {
            let observation = ObservationV4 {
                version: VERSION_V4,
                registry: REGISTRY_V4.into(),
                context: RunContext { run_id: run.into() },
                identity: Identity {
                    producer: "producer".into(),
                    generation: "generation".into(),
                    sequence: 1,
                },
                timestamp_ns: 1_000_000,
                resource: "resource".into(),
                data,
                span: SpanContext {
                    trace_id: trace.into(),
                    span_id: span.into(),
                    parent_span_id: Some(parent.into()),
                },
            };
            let decoded = decode_versioned(&encode_v4(&observation).unwrap()).unwrap();
            let IngestEnvelope::Trace(projected) = observation_to_telemetry(&decoded, "host")
            else {
                panic!("expected trace");
            };
            assert_eq!(projected.trace_id, trace);
            assert_eq!(projected.span_id, span);
            assert_eq!(projected.parent_span_id.as_deref(), Some(parent));
            let attrs: serde_json::Value =
                serde_json::from_str(&projected.attributes_json).unwrap();
            assert_eq!(attrs["version"], 4);
            assert_eq!(attrs["producer"], "producer");
            assert_eq!(attrs["generation"], "generation");
            assert_eq!(attrs["sequence"], 1);
        }
    }
}
