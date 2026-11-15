//! Non-blocking local Unix socket bridge (`ingest.sock`) accepting OTLP/Spindle
//! JSONL metrics, logs, and trace spans into bounded telemetry queues.

use crate::channel::BoundedTelemetryQueue;
use chrono::Utc;
use fabric_core::{LogRecord, MetricPoint, TraceSpanRecord};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
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

/// Spawns the background local OTLP/Spindle ingest listener using bounded `try_emit`.
pub async fn serve_ingest_socket(
    sock_path: &Path,
    metrics_q: BoundedTelemetryQueue<MetricPoint>,
    logs_q: BoundedTelemetryQueue<LogRecord>,
    traces_q: BoundedTelemetryQueue<TraceSpanRecord>,
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
        tokio::spawn(async move {
            let mut reader = BufReader::new(stream).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if line.len() > 64 * 1024 || line.trim().is_empty() {
                    continue;
                }
                if let Ok(env) = serde_json::from_str::<IngestEnvelope>(&line) {
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
