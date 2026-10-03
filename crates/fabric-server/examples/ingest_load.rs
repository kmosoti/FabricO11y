//! Ingest load generator for throughput measurements on one machine (ADR-0025).
//!
//! `ingest_load enroll <STATE_DIR> <TOKEN_DIR> <NODES> [LOG_PATH]` enrolls NODES nodes
//! in a stopped server's control state, with LOG_PATH as their centrally configured
//! log if given, and writes one token file per node.
//! `ingest_load run <URL> <CA> <TOKEN_DIR> <NODES> <SECONDS> <CORPUS> <BODY_KIB>` then
//! drives one thread per node, each sending Batches with one in flight, as a Spindle
//! does, but without a Spool: log lines drawn in order from CORPUS up to BODY_KIB of
//! bodies per Batch, plus three spans. It prints one JSON line: Batches and payload
//! bytes acknowledged, elapsed seconds, and per-Batch latency percentiles.
use fabric_o11y::spindle::sender::{Delivery, Sender, ServerTarget};
use fabric_o11y::spindle::spool::Batch;
use fabric_server::control::{Control, DesiredConfig};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

fn batch(
    node: u64,
    sequence: u64,
    lines: &[String],
    cursor: &mut usize,
    body_bytes: usize,
) -> Vec<u8> {
    let t = now_ns();
    let mut records = Vec::new();
    let mut size = 0;
    while size < body_bytes {
        let line = &lines[*cursor % lines.len()];
        *cursor += 1;
        size += line.len();
        records.push(LogRecord {
            observed_time_unix_nano: t + records.len() as u64,
            body: Some(AnyValue {
                value: Some(any_value::Value::StringValue(line.clone())),
            }),
            ..Default::default()
        });
    }
    let logs = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: records,
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let mut trace_id = [0_u8; 16];
    trace_id[..8].copy_from_slice(&node.to_be_bytes());
    trace_id[8..].copy_from_slice(&sequence.to_be_bytes());
    let traces = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: (0..3_u64)
                    .map(|i| Span {
                        trace_id: trace_id.to_vec(),
                        span_id: (sequence * 4 + i + 1).to_be_bytes().to_vec(),
                        name: format!("op-{i}"),
                        kind: 2,
                        start_time_unix_nano: t + i,
                        end_time_unix_nano: t + i + 1000,
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    let mut node_id = [0_u8; 16];
    node_id[..8].copy_from_slice(&node.to_be_bytes());
    node_id[8] = 0xF0;
    Batch {
        version: 1,
        node_id: node_id.to_vec(),
        generation: 1,
        sequence,
        metrics: Vec::new(),
        logs: logs.encode_to_vec(),
        cursors: Vec::new(),
        collection_gaps: Vec::new(),
        traces: traces.encode_to_vec(),
    }
    .encode_to_vec()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("enroll") => {
            let (state, tokens, nodes) = (
                PathBuf::from(&args[1]),
                PathBuf::from(&args[2]),
                args[3].parse::<u64>().unwrap(),
            );
            // Optional: a log path the enrolled nodes' central configuration collects.
            let logs: Vec<String> = args.get(4).into_iter().cloned().collect();
            std::fs::create_dir_all(&tokens).unwrap();
            let mut control = Control::open(&state).unwrap();
            for n in 0..nodes {
                let (_, token) = control
                    .enroll(
                        &format!("load-{n:04}"),
                        DesiredConfig {
                            logs: logs.clone(),
                            metric_interval_s: 3600,
                        },
                    )
                    .unwrap();
                std::fs::write(tokens.join(format!("token-{n:04}")), format!("{token}\n")).unwrap();
            }
        }
        Some("run") => {
            let (url, ca, tokens) = (
                args[1].clone(),
                PathBuf::from(&args[2]),
                PathBuf::from(&args[3]),
            );
            let nodes: u64 = args[4].parse().unwrap();
            let seconds: u64 = args[5].parse().unwrap();
            let lines: Arc<Vec<String>> = Arc::new(
                std::fs::read_to_string(&args[6])
                    .unwrap()
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .collect(),
            );
            let body_bytes = args[7].parse::<usize>().unwrap() * 1024;
            let started = Instant::now();
            let until = started + Duration::from_secs(seconds);
            let handles: Vec<_> = (0..nodes)
                .map(|n| {
                    let (url, ca, tokens, lines) =
                        (url.clone(), ca.clone(), tokens.clone(), lines.clone());
                    std::thread::spawn(move || {
                        let sender = Sender::new(&ServerTarget {
                            url,
                            ca,
                            token_file: tokens.join(format!("token-{n:04}")),
                        })
                        .unwrap();
                        let mut cursor = (n as usize * 7919) % lines.len();
                        let (mut sent, mut bytes, mut latencies, mut sequence) =
                            (0_u64, 0_u64, Vec::new(), 1_u64);
                        let mut body = batch(n, sequence, &lines, &mut cursor, body_bytes);
                        while Instant::now() < until {
                            let t = Instant::now();
                            match sender.send(&body) {
                                Delivery::Ack(_) => {
                                    latencies.push(t.elapsed().as_secs_f64() * 1e3);
                                    sent += 1;
                                    bytes += body.len() as u64;
                                    sequence += 1;
                                    body = batch(n, sequence, &lines, &mut cursor, body_bytes);
                                }
                                other => {
                                    eprintln!("node {n}: {other:?}");
                                    std::thread::sleep(Duration::from_millis(100));
                                }
                            }
                        }
                        (sent, bytes, latencies)
                    })
                })
                .collect();
            let (mut sent, mut bytes, mut latencies) = (0, 0, Vec::new());
            for h in handles {
                let (s, b, l) = h.join().unwrap();
                sent += s;
                bytes += b;
                latencies.extend(l);
            }
            let elapsed = started.elapsed().as_secs_f64();
            latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let pct = |p: f64| {
                latencies
                    .get(((latencies.len() as f64 - 1.0) * p) as usize)
                    .copied()
                    .unwrap_or(0.0)
            };
            println!(
                "{}",
                serde_json::json!({
                    "nodes": nodes, "batches": sent, "bytes": bytes, "elapsed_s": elapsed,
                    "mb_per_s": bytes as f64 / elapsed / 1e6, "batch_kib": body_bytes / 1024,
                    "latency_ms": {"p50": pct(0.5), "p99": pct(0.99), "max": pct(1.0)},
                })
            );
        }
        _ => {
            eprintln!(
                "usage: ingest_load enroll <STATE> <TOKENS> <NODES> | run <URL> <CA> <TOKENS> <NODES> <SECONDS> <CORPUS> <BODY_KIB>"
            );
            std::process::exit(2);
        }
    }
}
