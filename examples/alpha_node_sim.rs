//! Simulated node identities for the fleet tiers (plan step 3.4).
//!
//! One process speaks the real protocol for N enrolled identities: each has
//! its own random node identity, stream generation 1, batch sequences, bearer
//! token and TLS connection. It offers the registered open-loop workload
//! (per identity per second: two 512-byte log bodies, alternating repetitive
//! and seeded high entropy; 32 metric points every 15 s), builds real `Batch`
//! envelopes around OTLP export requests, sends each identity's oldest
//! unacknowledged batch, and polls configuration every 5 s.
//!
//! Unlike `fabric-node`, unacknowledged batches are retained in memory, not
//! in a synced spool. The fleet tiers measure the server; node-side process
//! crashes are exercised with real node processes instead. This process does
//! not represent 1,000 deployed hosts or networks.
//!
//! Output, under `--out`: `events.jsonl` (created, attempt and apply events
//! with Unix nanosecond times), `transcript.jsonl` for the delivery oracle
//! with bytes projected to their SHA-256, and `sim-summary.json`.

use fabric_o11y::alpha::journal::Batch;
use fabric_o11y::alpha::sender::{Delivery, Sender, ServerTarget};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, metric, number_data_point,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CONFIG_POLL: Duration = Duration::from_secs(5);
const SEEDS: [u64; 3] = [0xA11FA001, 0xA11FA002, 0xA11FA003];

fn unix_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The phase-0 high-entropy body: base85 of 13 chained SHA-256 digests.
fn entropy_body(seed: u64, node: usize, tick: u64) -> String {
    let source = format!("fabric-alpha-v1:{seed}:{node}:{tick}");
    let mut raw = Vec::with_capacity(13 * 32);
    for i in 0_u16..13 {
        let mut h = Sha256::new();
        h.update(source.as_bytes());
        h.update(b":");
        h.update(i.to_be_bytes());
        raw.extend_from_slice(&h.finalize());
    }
    base85(&raw)[..512].to_owned()
}

/// Python's `base64.b85encode` alphabet and padding.
fn base85(data: &[u8]) -> String {
    const A: &[u8; 85] =
        b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~";
    let mut out = String::new();
    for chunk in data.chunks(4) {
        let mut word = [0_u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        let mut n = u32::from_be_bytes(word);
        let mut digits = [0_u8; 5];
        for d in digits.iter_mut().rev() {
            *d = A[(n % 85) as usize];
            n /= 85;
        }
        let keep = if chunk.len() == 4 { 5 } else { chunk.len() + 1 };
        out.push_str(std::str::from_utf8(&digits[..keep]).unwrap());
    }
    out
}

fn attr(key: &str, value: String) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value)),
        }),
        ..Default::default()
    }
}

fn resource(index: usize) -> Resource {
    Resource {
        attributes: vec![
            attr("host.name", format!("sim-{index:04}")),
            attr("service.name", "fabric-node-sim".into()),
        ],
        ..Default::default()
    }
}

/// The registered offer for one identity in one second.
fn offer(seed: u64, index: usize, second: u64, now: u64) -> (Vec<u8>, Vec<u8>) {
    let records = (0..2)
        .map(|half| {
            let tick = second * 2 + half;
            let body = if tick.is_multiple_of(2) {
                "R".repeat(512)
            } else {
                entropy_body(seed, index, tick)
            };
            LogRecord {
                observed_time_unix_nano: now,
                body: Some(AnyValue {
                    value: Some(any_value::Value::StringValue(body)),
                }),
                ..Default::default()
            }
        })
        .collect();
    let logs = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(resource(index)),
            scope_logs: vec![ScopeLogs {
                log_records: records,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    let metrics = if second.is_multiple_of(15) {
        let points = (0..32)
            .map(|point| {
                let digest = Sha256::digest(format!("metric:{seed}:{index}:{second}:{point}"));
                let value = u64::from_be_bytes(digest[..8].try_into().unwrap()) >> 1;
                Metric {
                    name: format!("sim.metric.{point}"),
                    unit: "1".into(),
                    data: Some(metric::Data::Gauge(Gauge {
                        data_points: vec![NumberDataPoint {
                            time_unix_nano: now,
                            value: Some(number_data_point::Value::AsInt(value as i64)),
                            ..Default::default()
                        }],
                    })),
                    ..Default::default()
                }
            })
            .collect();
        ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Some(resource(index)),
                scope_metrics: vec![ScopeMetrics {
                    metrics: points,
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
        .encode_to_vec()
    } else {
        Vec::new()
    };
    (logs, metrics)
}

struct Identity {
    index: usize,
    node_id: [u8; 16],
    sender: Sender,
    next_sequence: u64,
    acked: u64,
    /// Unacknowledged batches in sequence order: (sequence, created ns, bytes).
    pending: VecDeque<(u64, u64, Vec<u8>)>,
    applied_revision: u64,
    next_poll: Instant,
}

struct Shared {
    events: Mutex<BufWriter<File>>,
    transcript: Mutex<BufWriter<File>>,
}

fn log_event(shared: &Shared, line: String) {
    let mut out = shared.events.lock().unwrap();
    let _ = writeln!(out, "{line}");
}

fn log_transcript(shared: &Shared, line: String) {
    let mut out = shared.transcript.lock().unwrap();
    let _ = writeln!(out, "{line}");
}

fn digest_b64(bytes: &[u8]) -> String {
    b64(&Sha256::digest(bytes))
}

struct Args {
    url: String,
    ca: PathBuf,
    tokens: PathBuf,
    seed: u64,
    seconds: u64,
    workers: usize,
    out: PathBuf,
}

fn parse() -> Result<Args, String> {
    let mut map = std::collections::HashMap::new();
    let raw: Vec<String> = std::env::args().skip(1).collect();
    for pair in raw.chunks(2) {
        let [k, v] = pair else {
            return Err("arguments are --key value pairs".into());
        };
        map.insert(k.trim_start_matches("--").to_owned(), v.clone());
    }
    let get = |k: &str| map.get(k).cloned().ok_or(format!("missing --{k}"));
    let seed = u64::from_str_radix(get("seed")?.trim_start_matches("0x"), 16)
        .map_err(|_| "seed must be hex".to_string())?;
    if !SEEDS.contains(&seed) {
        return Err("unregistered seed".into());
    }
    Ok(Args {
        url: get("server-url")?,
        ca: PathBuf::from(get("ca")?),
        tokens: PathBuf::from(get("tokens")?),
        seed,
        seconds: get("seconds")?.parse().map_err(|_| "bad --seconds")?,
        workers: get("workers")?.parse().map_err(|_| "bad --workers")?,
        out: PathBuf::from(get("out")?),
    })
}

fn main() -> ExitCode {
    let args = match parse() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("alpha_node_sim: {error}");
            return ExitCode::from(2);
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("alpha_node_sim: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> std::io::Result<()> {
    std::fs::create_dir_all(&args.out)?;
    let mut text = String::new();
    File::open(&args.tokens)?.read_to_string(&mut text)?;
    let mut identities = Vec::new();
    for (index, line) in text.lines().filter(|l| !l.trim().is_empty()).enumerate() {
        let token_file = args.out.join(format!("token-{index}"));
        std::fs::write(&token_file, format!("{}\n", line.trim()))?;
        let target = ServerTarget {
            url: args.url.clone(),
            ca: args.ca.clone(),
            token_file,
        };
        let mut node_id = [0_u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut node_id)?;
        identities.push(Identity {
            index,
            node_id,
            sender: Sender::new(&target)?,
            next_sequence: 1,
            acked: 0,
            pending: VecDeque::new(),
            applied_revision: 0,
            next_poll: Instant::now(),
        });
    }
    let count = identities.len();
    let shared = Arc::new(Shared {
        events: Mutex::new(BufWriter::new(File::create(args.out.join("events.jsonl"))?)),
        transcript: Mutex::new(BufWriter::new(File::create(
            args.out.join("transcript.jsonl"),
        )?)),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let began = Instant::now();
    let began_ns = unix_ns();
    let per_worker = count.div_ceil(args.workers.max(1));
    let mut slices: Vec<Vec<Identity>> = Vec::new();
    let mut rest = identities;
    while !rest.is_empty() {
        let tail = rest.split_off(per_worker.min(rest.len()));
        slices.push(rest);
        rest = tail;
    }
    let mut handles = Vec::new();
    for mut slice in slices {
        let shared = Arc::clone(&shared);
        let stop = Arc::clone(&stop);
        let (seed, seconds) = (args.seed, args.seconds);
        handles.push(std::thread::spawn(move || {
            let mut next_second = 0_u64;
            loop {
                let elapsed = began.elapsed();
                // Open loop: offer each due second regardless of delivery.
                while next_second < seconds && elapsed >= Duration::from_secs(next_second) {
                    let now = unix_ns();
                    for id in &mut slice {
                        let (logs, metrics) = offer(seed, id.index, next_second, now);
                        let batch = Batch {
                            version: 1,
                            node_id: id.node_id.to_vec(),
                            generation: 1,
                            sequence: id.next_sequence,
                            metrics,
                            logs,
                            cursors: vec![],
                            collection_gaps: vec![],
                        };
                        let bytes = batch.encode_to_vec();
                        log_transcript(
                            &shared,
                            format!(
                                "{{\"type\":\"source\",\"node_id\":\"{}\",\"generation\":1,\"sequence\":{},\"bytes\":\"{}\"}}",
                                hex(&id.node_id),
                                id.next_sequence,
                                digest_b64(&bytes)
                            ),
                        );
                        log_event(
                            &shared,
                            format!(
                                "{{\"e\":\"created\",\"id\":{},\"seq\":{},\"t\":{now}}}",
                                id.index, id.next_sequence
                            ),
                        );
                        id.pending.push_back((id.next_sequence, now, bytes));
                        id.next_sequence += 1;
                    }
                    next_second += 1;
                }
                let done_offering = next_second >= seconds;
                let mut progressed = false;
                for id in &mut slice {
                    if Instant::now() >= id.next_poll {
                        id.next_poll = Instant::now() + CONFIG_POLL;
                        if let Ok(Some(view)) = id.sender.fetch_config(id.applied_revision, None) {
                            id.applied_revision = view.revision;
                            // Confirm at once, as fabric-node does.
                            let _ = id.sender.fetch_config(id.applied_revision, None);
                            log_event(
                                &shared,
                                format!(
                                    "{{\"e\":\"applied\",\"id\":{},\"rev\":{},\"t\":{}}}",
                                    id.index,
                                    view.revision,
                                    unix_ns()
                                ),
                            );
                        }
                    }
                    let Some((seq, created, bytes)) = id.pending.front().cloned() else {
                        continue;
                    };
                    let started = unix_ns();
                    let outcome = id.sender.send(&bytes);
                    let finished = unix_ns();
                    let ident = format!(
                        "\"node_id\":\"{}\",\"generation\":1,\"sequence\":{seq}",
                        hex(&id.node_id)
                    );
                    log_transcript(
                        &shared,
                        format!(
                            "{{\"type\":\"attempt\",{ident},\"bytes\":\"{}\",\"injected_conflict\":false}}",
                            digest_b64(&bytes)
                        ),
                    );
                    let (kind, through) = match &outcome {
                        Delivery::Ack(t) => ("ack", Some(*t)),
                        Delivery::Conflict(_) => ("conflict", None),
                        Delivery::Gap(_) => ("gap", None),
                        Delivery::Rejected(w) if w.starts_with("HTTP 413") => ("too_large", None),
                        Delivery::Rejected(w) if w.starts_with("HTTP 400") => ("bad_request", None),
                        Delivery::Rejected(_) => ("unauthorized", None),
                        Delivery::Retry(w) if w.starts_with("HTTP 503") => ("unavailable", None),
                        Delivery::Retry(_) => ("no_response", None),
                    };
                    let through_field = through.map_or(String::new(), |t| format!(",\"committed_through\":{t}"));
                    log_transcript(
                        &shared,
                        format!("{{\"type\":\"response\",{ident},\"kind\":\"{kind}\"{through_field}}}"),
                    );
                    log_event(
                        &shared,
                        format!(
                            "{{\"e\":\"attempt\",\"id\":{},\"seq\":{seq},\"created\":{created},\"start\":{started},\"end\":{finished},\"kind\":\"{kind}\"}}",
                            id.index
                        ),
                    );
                    if let Delivery::Ack(t) = outcome
                        && t >= seq
                    {
                        id.acked = t;
                        while id.pending.front().is_some_and(|(s, _, _)| *s <= t) {
                            id.pending.pop_front();
                        }
                        progressed = true;
                    }
                }
                if done_offering && slice.iter().all(|id| id.pending.is_empty()) {
                    break;
                }
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                if !progressed {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            for id in &slice {
                let retained: Vec<String> = id.pending.iter().map(|(s, _, _)| s.to_string()).collect();
                log_transcript(
                    &shared,
                    format!(
                        "{{\"type\":\"node_state\",\"node_id\":\"{}\",\"generation\":1,\"ack_cursor\":{},\"retained_sequences\":[{}]}}",
                        hex(&id.node_id),
                        id.acked,
                        retained.join(",")
                    ),
                );
            }
            slice.iter().map(|id| id.pending.len()).sum::<usize>()
        }));
    }
    // Stop anything still undelivered 60 s after the offer ends.
    let limit = Duration::from_secs(args.seconds + 60);
    let watchdog_stop = Arc::clone(&stop);
    std::thread::spawn(move || {
        std::thread::sleep(limit);
        watchdog_stop.store(true, Ordering::SeqCst);
    });
    let undelivered: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
    shared.events.lock().unwrap().flush()?;
    shared.transcript.lock().unwrap().flush()?;
    std::fs::write(
        args.out.join("sim-summary.json"),
        format!(
            "{{\"identities\":{count},\"seconds\":{},\"seed\":{},\"began_unix_ns\":{began_ns},\"undelivered\":{undelivered}}}\n",
            args.seconds, args.seed
        ),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entropy_body_matches_the_phase0_python_generator() {
        let mut joined = String::new();
        for node in [0, 7] {
            for tick in [1, 3, 241] {
                let body = entropy_body(0xA11FA001, node, tick);
                assert_eq!(body.len(), 512);
                joined.push_str(&body);
            }
        }
        // sha256 of the same six bodies from tools/alpha/workload.py.
        assert_eq!(
            hex(&Sha256::digest(joined.as_bytes())),
            "5c1ecbdbd540dc9cea7b9f63227d423df0143972e2dc54d928c717378e5469aa"
        );
    }
}
