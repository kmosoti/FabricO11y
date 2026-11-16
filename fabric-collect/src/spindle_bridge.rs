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
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::net::UnixListener;
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
    let overall_deadline = Instant::now() + Duration::from_millis(250);
    poll_spindle_custody_records_with(
        after_id,
        max_records,
        default_host,
        overall_deadline,
        &mut |cursor, deadline| read_raw(spindle_sock, cursor, deadline),
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

/// Spawns the background local OTLP/Spindle ingest listener using bounded `try_emit`.
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

/// Spawns the background local OTLP/Spindle ingest listener and drains connection tasks on shutdown.
pub async fn serve_ingest_socket_until_shutdown(
    sock_path: &Path,
    metrics_q: BoundedTelemetryQueue<MetricPoint>,
    logs_q: BoundedTelemetryQueue<LogRecord>,
    traces_q: BoundedTelemetryQueue<TraceSpanRecord>,
    default_host: String,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<(), String> {
    if let Some(parent) = sock_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if sock_path.exists() {
        let _ = fs::remove_file(sock_path);
    }
    let listener = UnixListener::bind(sock_path)
        .map_err(|e| format!("failed binding {}: {e}", sock_path.display()))?;
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
                Ok((s, _)) => s,
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
            "fedora-blade",
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
            "fedora-blade",
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
            "fedora-blade",
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
            let _ = serve_ingest_socket(&sock_srv, mq, lq, tq, "fedora-blade".into()).await;
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
            host: "fedora-blade".into(),
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
                "fedora-blade".into(),
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
            host: "fedora-blade".into(),
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
            host: "fedora-blade".into(),
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
}
