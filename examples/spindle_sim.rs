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

use fabric_o11y::spindle::sender::{Delivery, Sender, ServerTarget};
use fabric_o11y::spindle::spool::Batch;
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
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CONFIG_POLL: Duration = Duration::from_secs(5);
const SEEDS: [u64; 3] = [0xA11FA001, 0xA11FA002, 0xA11FA003];

fn witness_require(ok: bool, message: &str) -> std::io::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(std::io::Error::other(message))
    }
}

fn monotonic_ns() -> std::io::Result<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // The pointer names a live timespec; CLOCK_MONOTONIC never changes host time.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    witness_require(
        time.tv_sec >= 0 && (0..1_000_000_000).contains(&time.tv_nsec),
        "invalid monotonic clock",
    )?;
    (time.tv_sec as u64)
        .checked_mul(1_000_000_000)
        .and_then(|n| n.checked_add(time.tv_nsec as u64))
        .ok_or_else(|| std::io::Error::other("monotonic clock overflow"))
}

fn clock_bracket(lo: u64, hi: u64) -> std::io::Result<Duration> {
    witness_require(
        hi >= lo && hi - lo <= 1_000_000,
        "begin clock bracket exceeds bound",
    )?;
    Ok(Duration::from_nanos(hi - lo))
}

fn witness_partition(count: usize, workers: usize, seconds: u64) -> std::io::Result<usize> {
    witness_require(
        (1..=100).contains(&count) && (1..=100).contains(&workers) && (1..=7200).contains(&seconds),
        "native witness requires bounded positive parameters",
    )?;
    let per_worker = count.div_ceil(workers);
    witness_require(
        count.div_ceil(per_worker) == workers,
        "native witness requires exact worker partition",
    )?;
    Ok(per_worker)
}

struct BeginWitness {
    path: PathBuf,
    file: File,
    raw: Vec<u8>,
    value: serde_json::Value,
}

impl BeginWitness {
    fn publish(
        path: PathBuf,
        value: serde_json::Value,
        mut step: impl FnMut(&str) -> std::io::Result<()>,
    ) -> std::io::Result<Self> {
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("witness parent absent"))?;
        let meta = std::fs::symlink_metadata(parent)?;
        // geteuid has no arguments or pointer preconditions.
        witness_require(
            path.is_absolute()
                && parent.canonicalize()? == parent
                && meta.is_dir()
                && meta.uid() == unsafe { libc::geteuid() }
                && meta.mode() & 0o7777 == 0o700,
            "witness parent must be canonical, owned and private",
        )?;
        let mounts = std::fs::read_to_string("/proc/self/mountinfo")?;
        witness_require(
            !mounts
                .lines()
                .any(|line| line.split_whitespace().nth(4) == parent.to_str()),
            "witness parent is a mount",
        )?;
        let raw = serde_json::to_vec(&value)?;
        witness_require(raw.len() <= 131072, "bounded witness")?;
        let stage = parent.join(format!(".begin-witness-{}.stage", std::process::id()));
        let file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&stage)?;
        // Failed stages are retained. Only this successfully-created name may be unlinked.
        let mut witness = Self {
            path,
            file,
            raw,
            value,
        };
        step("write")?;
        witness.file.write_all(&witness.raw)?;
        witness
            .file
            .set_permissions(std::fs::Permissions::from_mode(0o400))?;
        step("sync")?;
        witness.file.sync_all()?;
        step("link")?;
        std::fs::hard_link(&stage, &witness.path)?;
        step("unlink")?;
        std::fs::remove_file(&stage)?;
        File::open(witness.path.parent().unwrap())?.sync_all()?;
        witness.verify()?;
        Ok(witness)
    }

    fn verify(&mut self) -> std::io::Result<()> {
        let held = self.file.metadata()?;
        let named = std::fs::symlink_metadata(&self.path)?;
        witness_require(
            named.is_file()
                && held.is_file()
                && named.dev() == held.dev()
                && named.ino() == held.ino()
                && held.nlink() == 1
                && held.mode() & 0o7777 == 0o400
                && held.len() == self.raw.len() as u64,
            "witness identity or metadata changed",
        )?;
        self.file.seek(SeekFrom::Start(0))?;
        let mut raw = Vec::new();
        Read::by_ref(&mut self.file)
            .take(131073)
            .read_to_end(&mut raw)?;
        let final_held = self.file.metadata()?;
        let final_named = std::fs::symlink_metadata(&self.path)?;
        let identity = |m: &std::fs::Metadata| {
            (
                m.dev(),
                m.ino(),
                m.mode(),
                m.uid(),
                m.gid(),
                m.nlink(),
                m.len(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
            )
        };
        witness_require(
            raw == self.raw
                && identity(&held) == identity(&final_held)
                && identity(&held) == identity(&final_named),
            "held witness bytes changed",
        )
    }
}

fn begin_value(
    args: &Args,
    count: usize,
    lo: u64,
    hi: u64,
    wall: u64,
) -> std::io::Result<serde_json::Value> {
    clock_bracket(lo, hi)?;
    let process = std::fs::read_to_string("/proc/self/stat")?;
    let ticks: u64 = process
        .rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| std::io::Error::other("process start identity"))?;
    let mut executable = File::open("/proc/self/exe")?;
    let metadata = executable.metadata()?;
    witness_require(metadata.len() <= 268435456, "bounded executable identity")?;
    let mut hash = Sha256::new();
    let mut block = [0; 65536];
    loop {
        let size = executable.read(&mut block)?;
        if size == 0 {
            break;
        }
        hash.update(&block[..size]);
    }
    Ok(serde_json::json!({
        "mono_before_ns": lo, "mono_after_ns": hi, "began_unix_ns": wall,
        "binary_sha256": hex(&hash.finalize()),
        "parameters": {"seconds": args.seconds, "identities": count, "workers": args.workers, "seed": args.seed},
        "process": {"pid": std::process::id(), "start_ticks": ticks,
            "boot_id": std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim(),
            "exe_device": metadata.dev(), "exe_inode": metadata.ino()}
    }))
}

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
    entropy_body_sized(seed, node, tick, 512)
}

fn entropy_body_sized(seed: u64, node: usize, tick: u64, body_bytes: usize) -> String {
    let source = format!("fabric-alpha-v1:{seed}:{node}:{tick}");
    let mut raw = Vec::with_capacity(13 * 32);
    for i in 0_u16..body_bytes.div_ceil(40) as u16 {
        let mut h = Sha256::new();
        h.update(source.as_bytes());
        h.update(b":");
        h.update(i.to_be_bytes());
        raw.extend_from_slice(&h.finalize());
    }
    base85(&raw)[..body_bytes].to_owned()
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

/// The registered offer for one identity in one second; `factor` > 1 is a
/// burst of that many times the registered log rate.
fn offer(seed: u64, index: usize, second: u64, now: u64, factor: u64) -> (Vec<u8>, Vec<u8>) {
    offer_sized(seed, index, second, now, factor, 512)
}

fn offer_sized(
    seed: u64,
    index: usize,
    second: u64,
    now: u64,
    factor: u64,
    body_bytes: usize,
) -> (Vec<u8>, Vec<u8>) {
    let records = (0..2 * factor)
        .map(|half| {
            // Registered ticks for the first two; burst extras get distinct ticks.
            let tick = if half < 2 {
                second * 2 + half
            } else {
                1_000_000 + second * 100 + half
            };
            let body = if tick.is_multiple_of(2) {
                "R".repeat(body_bytes)
            } else {
                if body_bytes == 512 {
                    entropy_body(seed, index, tick)
                } else {
                    entropy_body_sized(seed, index, tick, body_bytes)
                }
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
    log_factor: u64,
    body_bytes: usize,
    out: PathBuf,
    /// Seconds `[from, to)` offered at `factor` times the log rate.
    burst: Option<(u64, u64, u64)>,
    begin_witness: Option<PathBuf>,
}

fn parse() -> Result<Args, String> {
    parse_arguments(&std::env::args().skip(1).collect::<Vec<_>>())
}

fn parse_arguments(raw: &[String]) -> Result<Args, String> {
    let mut map = std::collections::HashMap::new();
    for pair in raw.chunks(2) {
        let [k, v] = pair else {
            return Err("arguments are --key value pairs".into());
        };
        let key = k.trim_start_matches("--").to_owned();
        if key == "begin-witness" && map.contains_key(&key) {
            return Err("duplicate --begin-witness".into());
        }
        map.insert(key, v.clone());
    }
    let get = |k: &str| map.get(k).cloned().ok_or(format!("missing --{k}"));
    let seed = u64::from_str_radix(get("seed")?.trim_start_matches("0x"), 16)
        .map_err(|_| "seed must be hex".to_string())?;
    if !SEEDS.contains(&seed) {
        return Err("unregistered seed".into());
    }
    let log_factor = map
        .get("log-factor")
        .map_or(Ok(1_u64), |v| v.parse())
        .map_err(|_| "bad --log-factor")?;
    let body_bytes = map
        .get("body-bytes")
        .map_or(Ok(512_usize), |v| v.parse())
        .map_err(|_| "bad --body-bytes")?;
    if !(1..=100).contains(&log_factor)
        || !(1..=16384).contains(&body_bytes)
        || log_factor as usize * 2 * body_bytes > 900_000
    {
        return Err("log factor/body size exceeds bounded fixture limits".into());
    }
    Ok(Args {
        url: get("server-url")?,
        ca: PathBuf::from(get("ca")?),
        tokens: PathBuf::from(get("tokens")?),
        seed,
        seconds: get("seconds")?.parse().map_err(|_| "bad --seconds")?,
        workers: get("workers")?.parse().map_err(|_| "bad --workers")?,
        log_factor,
        body_bytes,
        out: PathBuf::from(get("out")?),
        begin_witness: map.get("begin-witness").map(PathBuf::from),
        burst: match (
            map.get("burst-from"),
            map.get("burst-to"),
            map.get("burst-factor"),
        ) {
            (None, None, None) => None,
            (Some(a), Some(b), Some(f)) => Some((
                a.parse().map_err(|_| "bad --burst-from")?,
                b.parse().map_err(|_| "bad --burst-to")?,
                f.parse().map_err(|_| "bad --burst-factor")?,
            )),
            _ => return Err("--burst-from, --burst-to and --burst-factor go together".into()),
        },
    })
}

fn main() -> ExitCode {
    let args = match parse() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("spindle_sim: {error}");
            return ExitCode::from(2);
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("spindle_sim: {error}");
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
        File::options()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&token_file)?
            .write_all(format!("{}\n", line.trim()).as_bytes())?;
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
    let per_worker = if args.begin_witness.is_some() {
        witness_partition(count, args.workers, args.seconds)?
    } else {
        count.div_ceil(args.workers.max(1))
    };
    if let Some(path) = &args.begin_witness {
        witness_require(
            path.parent() == Some(args.out.as_path()),
            "begin witness must use the owned output directory",
        )?;
    }
    let shared = Arc::new(Shared {
        events: Mutex::new(BufWriter::new(File::create(args.out.join("events.jsonl"))?)),
        transcript: Mutex::new(BufWriter::new(File::create(
            args.out.join("transcript.jsonl"),
        )?)),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let lo = if args.begin_witness.is_some() {
        monotonic_ns()?
    } else {
        0
    };
    let began = Instant::now();
    let began_ns = unix_ns();
    let hi = if args.begin_witness.is_some() {
        monotonic_ns()?
    } else {
        0
    };
    let mut witness = if let Some(path) = &args.begin_witness {
        Some(BeginWitness::publish(
            path.clone(),
            begin_value(&args, count, lo, hi, began_ns)?,
            |_| Ok(()),
        )?)
    } else {
        None
    };
    let completion = witness
        .as_ref()
        .map(|_| Duration::from_secs(args.seconds) + Duration::from_nanos(hi - lo));
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
        let (seed, seconds, burst, log_factor, body_bytes) = (
            args.seed,
            args.seconds,
            args.burst,
            args.log_factor,
            args.body_bytes,
        );
        handles.push(std::thread::spawn(move || {
            let mut next_second = 0_u64;
            loop {
                let elapsed = began.elapsed();
                // Open loop: offer each due second regardless of delivery.
                while next_second < seconds && elapsed >= Duration::from_secs(next_second) {
                    let now = unix_ns();
                    for id in &mut slice {
                        let factor = match burst {
                            Some((from, to, f)) if (from..to).contains(&next_second) => f,
                            _ => 1,
                        };
                        let (logs, metrics) = if log_factor == 1 && body_bytes == 512 {
                            offer(seed, id.index, next_second, now, factor)
                        } else {
                            offer_sized(seed, id.index, next_second, now, factor * log_factor, body_bytes)
                        };
                        let batch = Batch {
                            version: 1,
                            node_id: id.node_id.to_vec(),
                            generation: 1,
                            sequence: id.next_sequence,
                            metrics,
                            logs,
                            cursors: vec![],
                            collection_gaps: vec![],
                            traces: Vec::new(),
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
                                "{{\"e\":\"created\",\"id\":{},\"seq\":{},\"t\":{now},\"generated_ns\":{}}}",
                                id.index, id.next_sequence, unix_ns()
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
                if done_offering && slice.iter().all(|id| id.pending.is_empty())
                    && completion.is_none_or(|duration| began.elapsed() >= duration) {
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
    if let Some(witness) = &mut witness {
        witness.verify()?;
        let elapsed = began.elapsed().as_nanos();
        let ended = monotonic_ns()?;
        witness_require(
            elapsed >= u128::from(args.seconds) * 1_000_000_000
                && ended >= hi + args.seconds * 1_000_000_000,
            "native lifetime ended early",
        )?;
        let summary = serde_json::json!({"identities": count, "seconds": args.seconds, "seed": args.seed,
            "began_unix_ns": began_ns, "undelivered": undelivered, "workers": args.workers,
            "elapsed_ns": elapsed, "ended_monotonic_ns": ended,
            "begin_witness_sha256": hex(&Sha256::digest(&witness.raw)), "process": witness.value["process"]});
        std::fs::write(
            args.out.join("sim-summary.json"),
            serde_json::to_vec(&summary)?,
        )?;
        return witness_require(undelivered == 0, "native witness has undelivered work");
    }
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

    struct WitnessDirectory(PathBuf);

    impl WitnessDirectory {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path =
                PathBuf::from(std::env::var_os("FABRIC_SCRATCH_ROOT").expect("contained scratch"))
                    .join(format!(
                        "native-witness-{}-{}",
                        std::process::id(),
                        NEXT.fetch_add(1, Ordering::SeqCst)
                    ));
            std::fs::create_dir(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
    }

    impl Drop for WitnessDirectory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).expect("owned witness fixture cleanup");
        }
    }

    #[test]
    fn native_witness_complete_private_publication_order() {
        let root = WitnessDirectory::new();
        let path = root.0.join("begin.json");
        let value = serde_json::json!({"literal": "complete record"});
        let mut seen = Vec::new();
        let mut witness = BeginWitness::publish(path.clone(), value.clone(), |step| {
            seen.push(step.to_owned());
            if step == "unlink" {
                assert_eq!(
                    std::fs::read(&path).unwrap(),
                    serde_json::to_vec(&value).unwrap()
                );
                assert_eq!(std::fs::metadata(&path).unwrap().nlink(), 2);
            } else {
                assert!(!path.exists());
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, ["write", "sync", "link", "unlink"]);
        assert_eq!(std::fs::metadata(&path).unwrap().nlink(), 1);
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o7777, 0o400);
        witness.verify().unwrap();
    }

    #[test]
    fn native_witness_refuses_each_publication_failure() {
        for failed in ["write", "sync", "link", "unlink"] {
            let root = WitnessDirectory::new();
            let path = root.0.join("begin.json");
            let result =
                BeginWitness::publish(path.clone(), serde_json::json!({"literal": 1}), |step| {
                    witness_require(step != failed, "literal injected file-operation failure")
                });
            assert!(result.is_err());
            assert_eq!(path.exists(), failed == "unlink");
            if path.exists() {
                assert_eq!(std::fs::read(path).unwrap(), b"{\"literal\":1}");
            }
        }
    }

    #[test]
    fn native_witness_preserves_existing_destination_and_stage_alias() {
        for staged in [false, true] {
            let root = WitnessDirectory::new();
            let path = root.0.join("begin.json");
            let target = root.0.join("existing");
            std::fs::write(&target, b"preserve these bytes").unwrap();
            let alias = if staged {
                root.0
                    .join(format!(".begin-witness-{}.stage", std::process::id()))
            } else {
                path.clone()
            };
            std::os::unix::fs::symlink(&target, alias).unwrap();
            assert!(BeginWitness::publish(path, serde_json::json!({}), |_| Ok(())).is_err());
            assert_eq!(std::fs::read(target).unwrap(), b"preserve these bytes");
        }
    }

    #[test]
    fn native_witness_refuses_changed_bytes_metadata_or_name() {
        for mutation in 0..3 {
            let root = WitnessDirectory::new();
            let path = root.0.join("begin.json");
            let mut witness =
                BeginWitness::publish(path.clone(), serde_json::json!({"literal": 1}), |_| Ok(()))
                    .unwrap();
            if mutation == 0 {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
                std::fs::write(&path, b"{\"literal\":2}").unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
            } else if mutation == 1 {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            } else {
                std::fs::remove_file(&path).unwrap();
                std::fs::write(&path, &witness.raw).unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
            }
            assert!(witness.verify().is_err());
        }
    }

    #[test]
    fn native_witness_clock_brackets_are_bounded() {
        assert!(clock_bracket(2, 1).is_err());
        assert!(clock_bracket(0, 1_000_001).is_err());
        assert_eq!(
            clock_bracket(1, 1_000_001).unwrap(),
            Duration::from_millis(1)
        );
        assert!(monotonic_ns().unwrap() <= monotonic_ns().unwrap());
    }

    #[test]
    fn native_witness_duplicate_option_refuses_before_io() {
        let raw = ["--begin-witness", "/a", "--begin-witness", "/b"].map(str::to_owned);
        assert!(parse_arguments(&raw).err().unwrap().contains("duplicate"));
    }

    #[test]
    fn native_witness_requires_duration_and_actual_workers() {
        for (count, workers, seconds) in [(0, 1, 2), (2, 0, 2), (2, 2, 0), (2, 2, 7201), (5, 4, 2)]
        {
            assert!(witness_partition(count, workers, seconds).is_err());
        }
        assert_eq!(witness_partition(5, 3, 2).unwrap(), 2);
        assert_eq!(witness_partition(2, 2, 7200).unwrap(), 1);
    }

    #[test]
    fn sized_workload_preserves_clock_and_exact_event_count() {
        for factor in [25, 75] {
            let (logs, _) = offer_sized(0xA11FA001, 7, 12, 123456, factor, 900);
            let decoded = ExportLogsServiceRequest::decode(logs.as_slice()).unwrap();
            let rows = &decoded.resource_logs[0].scope_logs[0].log_records;
            assert_eq!(rows.len(), (2 * factor) as usize);
            for row in rows {
                assert_eq!(row.observed_time_unix_nano, 123456);
                let Some(any_value::Value::StringValue(body)) =
                    row.body.as_ref().and_then(|v| v.value.as_ref())
                else {
                    panic!("string body required")
                };
                assert_eq!(body.len(), 900);
            }
        }
    }

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
        // sha256 of the same six bodies from tools/qualification/workload.py.
        assert_eq!(
            hex(&Sha256::digest(joined.as_bytes())),
            "5c1ecbdbd540dc9cea7b9f63227d423df0143972e2dc54d928c717378e5469aa"
        );
    }
}
