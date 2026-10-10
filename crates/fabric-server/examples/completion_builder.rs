//! Private C2 fixture, derived from the registered study generator's semantics.
//! gen ROOT SHAPE MIB; run NEW_STATE SEALED_INPUT reference|bounded.
//! entropy-gen ROOT 8192 produces the separately registered encoded-page probe.
//! Build snapshots exclude generation/readback; differential readers are shared.
use fabric_frame::{envelope::Batch, frame::FrameLog};
use fabric_server::{
    segment,
    store::{Entry, Group},
    text_filter::GroupFilter,
};
use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
};
use opentelemetry_proto::tonic::common::v1::{AnyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, metric, number_data_point,
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::statistics::Statistics;
use prost::Message;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[cfg(feature = "responsibility-alloc-probe")]
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;
const MAX_GROUP_PAYLOAD: usize = segment::MAX_GROUP_PAYLOAD;
#[cfg(feature = "responsibility-alloc-probe")]
struct Counted;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "responsibility-alloc-probe")]
fn add(n: usize) {
    let live = LIVE.fetch_add(n, Relaxed) + n;
    PEAK.fetch_max(live, Relaxed);
}
// These methods delegate allocation/layout validity to System. Counters never
// allocate, and unsuccessful allocations/reallocations do not change the ledger.
#[cfg(feature = "responsibility-alloc-probe")]
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            TOTAL.fetch_add(layout.size(), Relaxed);
            CALLS.fetch_add(1, Relaxed);
            add(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            TOTAL.fetch_add(layout.size(), Relaxed);
            CALLS.fetch_add(1, Relaxed);
            add(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, layout, size) };
        if !q.is_null() {
            TOTAL.fetch_add(size, Relaxed);
            CALLS.fetch_add(1, Relaxed);
            if size >= layout.size() {
                add(size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - size, Relaxed);
            }
        }
        q
    }
}
#[global_allocator]
#[cfg(feature = "responsibility-alloc-probe")]
static ALLOCATOR: Counted = Counted;
const T0: u64 = 1_800_000_000_000_000_000;
const NODES: usize = 100;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

static BODY: AtomicUsize = AtomicUsize::new(512);
fn body(rng: &mut Rng, tick: u64) -> String {
    let n = BODY.load(Relaxed);
    if tick.is_multiple_of(2) {
        return "R".repeat(n);
    }
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    (0..n)
        .map(|_| T[(rng.next() % 64) as usize] as char)
        .collect()
}

/// One node-second batch; `obs` gives each row's time.
fn make_batch(
    rng: &mut Rng,
    node: usize,
    seq: u64,
    second: u64,
    obs: &mut dyn FnMut() -> u64,
) -> Vec<u8> {
    let records = (0..2)
        .map(|h| LogRecord {
            observed_time_unix_nano: obs(),
            body: Some(AnyValue {
                value: Some(any_value::Value::StringValue(body(rng, second * 2 + h))),
            }),
            ..Default::default()
        })
        .collect();
    let logs = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: records,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    let metrics = if second.is_multiple_of(15) {
        let metrics = (0..32)
            .map(|p| Metric {
                name: format!("sim.metric.{p}"),
                unit: "1".into(),
                data: Some(metric::Data::Gauge(Gauge {
                    data_points: vec![NumberDataPoint {
                        time_unix_nano: obs(),
                        value: Some(number_data_point::Value::AsInt((rng.next() >> 1) as i64)),
                        ..Default::default()
                    }],
                })),
                ..Default::default()
            })
            .collect();
        ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                scope_metrics: vec![ScopeMetrics {
                    metrics,
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
        .encode_to_vec()
    } else {
        Vec::new()
    };
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&(node as u64 + 1).to_be_bytes());
    Batch {
        version: 1,
        node_id: id.to_vec(),
        generation: 1,
        sequence: seq,
        metrics,
        logs,
        cursors: Vec::new(),
        traces: Vec::new(),
        collection_gaps: if seq.is_multiple_of(500) {
            vec![format!("gap at {seq}")]
        } else {
            Vec::new()
        },
    }
    .encode_to_vec()
}

fn generate(kind: &str, dir: &Path, file_bytes: u64) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    if kind == "bigrows" {
        BODY.store(16384, Relaxed);
    }
    let mut log = FrameLog::open(dir, u64::MAX / 4, MAX_GROUP_PAYLOAD, |_, _| Ok(()))?;
    let mut rng = Rng(0xA11FA001);
    let mut seqs = [0u64; NODES];
    let mut backlog: Vec<VecDeque<u64>> = vec![VecDeque::new(); NODES];
    let mut group_seq = 1u64;
    let span_s = 520u64; // approximate duration of 64 MiB, for adversarial times
    for s in 0.. {
        // Which (node, batch-second) pairs are sent this second.
        let mut sends: Vec<(usize, u64)> = Vec::new();
        for (n, pending) in backlog.iter_mut().enumerate() {
            let offline = kind == "outage" && n < 20 && (120..360).contains(&s);
            if offline {
                pending.push_back(s);
                continue;
            }
            for _ in 0..10 {
                match pending.pop_front() {
                    Some(old) => sends.push((n, old)),
                    None => break,
                }
            }
            sends.push((n, s));
        }
        for tick in 0..20u64 {
            let mut entries = Vec::new();
            for &(n, bs) in sends.iter().filter(|(n, _)| (*n as u64) % 20 == tick) {
                seqs[n] += 1;
                let jitter = (n as u64 * 7919 % 50) * 1_000_000;
                let mut r2 = Rng(rng.next() | 1);
                let mut obs = || {
                    if kind == "adversarial" {
                        T0 + r2.next() % (span_s * 1_000_000_000)
                    } else {
                        T0 + bs * 1_000_000_000 + jitter
                    }
                };
                let batch = make_batch(&mut rng, n, seqs[n], bs, &mut obs);
                entries.push(Entry {
                    label: format!("node-{n:04}"),
                    batch,
                    received_unix_nano: T0 + s * 1_000_000_000 + tick * 50_000_000 + 5_000_000,
                });
            }
            if entries.is_empty() {
                continue;
            }
            let payload = Group {
                group_sequence: group_seq,
                entries,
            }
            .encode_to_vec();
            if log.active_bytes() >= file_bytes {
                log.rotate(1)?;
                return Ok(());
            }
            log.append(&payload)?;
            group_seq += 1;
        }
    }
    Ok(())
}

fn digest(path: &Path) -> io::Result<String> {
    let mut reader = File::open(path)?;
    let mut h = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        h.update(&buffer[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// Stream 32 rows per ordinary envelope; never retain the whole probe fixture.
/// The first 16 base64 characters encode the row index, guaranteeing distinct
/// bodies independently of the pseudo-random suffix.
fn generate_entropy(dir: &Path, rows: usize) -> io::Result<()> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    std::fs::create_dir(dir)?;
    let mut log = FrameLog::open(dir, u64::MAX / 4, MAX_GROUP_PAYLOAD, |_, _| Ok(()))?;
    let mut rng = Rng(0xA11FA001);
    for first in (0..rows).step_by(32) {
        let records = (first..(first + 32).min(rows))
            .map(|row| {
                let mut body = String::with_capacity(16384);
                for digit in 0..16 {
                    body.push(ALPHABET[((row as u64 >> ((digit % 11) * 6)) & 63) as usize] as char);
                }
                for _ in 16..16384 {
                    body.push(ALPHABET[(rng.next() & 63) as usize] as char);
                }
                LogRecord {
                    observed_time_unix_nano: T0 + row as u64 * 1_000_000_000,
                    body: Some(AnyValue {
                        value: Some(any_value::Value::StringValue(body)),
                    }),
                    ..Default::default()
                }
            })
            .collect();
        let logs = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                scope_logs: vec![ScopeLogs {
                    log_records: records,
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
        .encode_to_vec();
        let sequence = (first / 32 + 1) as u64;
        let mut node_id = vec![0u8; 16];
        node_id[..8].copy_from_slice(&1u64.to_be_bytes());
        let batch = Batch {
            version: 1,
            node_id,
            generation: 1,
            sequence,
            logs,
            ..Default::default()
        }
        .encode_to_vec();
        let payload = Group {
            group_sequence: sequence,
            entries: vec![Entry {
                label: "node-0000".into(),
                batch,
                received_unix_nano: T0 + (first + 31) as u64 * 1_000_000_000 + 5_000_000,
            }],
        }
        .encode_to_vec();
        log.append(&payload)?;
    }
    log.rotate(1)?;
    Ok(())
}

fn proc_io() -> io::Result<BTreeMap<String, u64>> {
    std::fs::read_to_string("/proc/self/io")?
        .lines()
        .map(|line| {
            let (key, value) = line
                .split_once(':')
                .ok_or_else(|| io::Error::other("bad proc io"))?;
            let value = value.trim().parse().map_err(io::Error::other)?;
            Ok((key.to_owned(), value))
        })
        .collect()
}

fn ledger(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

type TimestampGroupBounds = (u64, u64, u64); // minimum, maximum, row count

fn groups(
    dir: &Path,
    name: &str,
    column: usize,
) -> Result<Vec<TimestampGroupBounds>, Box<dyn std::error::Error>> {
    let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(dir.join(name))?)?;
    reader
        .metadata()
        .row_groups()
        .iter()
        .map(|group| match group.column(column).statistics() {
            Some(Statistics::Int64(s)) => match (s.min_opt(), s.max_opt()) {
                (Some(lo), Some(hi)) if *lo >= 0 && *hi >= *lo => {
                    Ok((*lo as u64, *hi as u64, group.num_rows() as u64))
                }
                _ => Err("missing/invalid registered timestamp statistics".into()),
            },
            _ => Err("missing registered timestamp statistics".into()),
        })
        .collect()
}

fn pruning(times: &mut [u64], bounds: &[(u64, u64, u64)]) -> Value {
    times.sort_unstable();
    let mut reports = BTreeMap::new();
    for seconds in [10u64, 60] {
        let mut windows = Vec::new();
        let (mut total_read, mut total_matched) = (0u64, 0u64);
        if let (Some(lo), Some(hi)) = (times.first(), times.last()) {
            let width = seconds * 1_000_000_000;
            let mut from = *lo;
            while from <= *hi {
                let to = from + width;
                let matched = (times.partition_point(|t| *t < to)
                    - times.partition_point(|t| *t < from)) as u64;
                let read = if matched > 0 {
                    bounds
                        .iter()
                        .filter(|(lo, hi, _)| *hi >= from && *lo < to)
                        .map(|b| b.2)
                        .sum()
                } else {
                    0
                };
                total_read += read;
                total_matched += matched;
                windows.push(
                    json!({"from_ns":from,"to_ns":to,"matched_rows":matched,"read_rows":read}),
                );
                from += width / 2;
            }
        }
        reports.insert(seconds.to_string(),json!({"read_rows":total_read,"matched_rows":total_matched,
            "amplification":if total_matched>0 {Some(total_read as f64/total_matched as f64)} else {None},"windows":windows}));
    }
    json!({"row_groups":bounds.len(),"group_rows":bounds.iter().map(|b| b.2).collect::<Vec<_>>(),"windows":reports})
}

fn filter_check(
    dir: &Path,
    manifest: &segment::Manifest,
    bounds: &[(u64, u64, u64)],
) -> Result<Value, Box<dyn std::error::Error>> {
    let filters =
        segment::read_text_filter(dir, manifest).ok_or("filter authentication/decode failed")?;
    let declared = manifest
        .files
        .get("text_filter.bin")
        .ok_or("missing text filter manifest")?
        .rows;
    if filters.len() != bounds.len() || filters.len() as u64 != declared {
        return Err("filter group alignment failed".into());
    }
    let missing_control = !filters.is_empty() && filters.len() - 1 != bounds.len();
    let cleared = GroupFilter::build(std::iter::empty::<&str>());
    let (mut trigrams, mut false_negatives, mut cleared_rejected) = (0u64, 0u64, false);
    let mut decoded_rows = 0u64;
    for (index, bound) in bounds.iter().enumerate() {
        let mut rows = 0u64;
        segment::scan_logs_groups(dir, manifest, vec![index], 0, u64::MAX, |row| {
            rows += 1;
            for needle in row.body.as_bytes().windows(3) {
                trigrams += 1;
                if !filters[index].may_contain(needle) {
                    false_negatives += 1;
                }
                if !cleared.may_contain(needle) {
                    cleared_rejected = true;
                }
            }
        })?;
        if rows != bound.2 {
            return Err("group readback row count differs".into());
        }
        decoded_rows += rows;
    }
    if false_negatives != 0 || trigrams == 0 || !cleared_rejected || !missing_control {
        return Err("filter content or negative controls failed".into());
    }
    Ok(
        json!({"aligned":true,"physical_groups":filters.len(),"declared_filter_rows":declared,"logical_rows":decoded_rows,
        "body_trigrams":trigrams,"false_negatives":false_negatives,"missing_control_rejected":missing_control,"cleared_control_rejected":cleared_rejected}),
    )
}

fn leftovers(root: &Path) -> io::Result<Vec<String>> {
    let mut found = Vec::new();
    if root.exists() {
        for entry in std::fs::read_dir(root)? {
            let path = entry?.path();
            let name = path
                .file_name()
                .ok_or_else(|| io::Error::other("missing name"))?
                .to_string_lossy();
            if name.starts_with(".building-") || name.contains(".run-") {
                found.push(path.to_string_lossy().into_owned());
            }
            if path.is_dir() {
                found.extend(leftovers(&path)?);
            }
        }
    }
    Ok(found)
}

fn run(state: &Path, input: &Path, builder: &str) -> Result<Value, Box<dyn std::error::Error>> {
    if state.exists() {
        return Err("state must be fresh".into());
    }
    let input_hash = digest(input)?;
    let journal_bytes = std::fs::metadata(input)?.len();
    let io_before = proc_io()?;
    let stat_before = std::fs::read_to_string("/proc/self/stat")?;
    let heap_start = LIVE.load(Relaxed);
    PEAK.store(heap_start, Relaxed);
    TOTAL.store(0, Relaxed);
    CALLS.store(0, Relaxed);
    let began = Instant::now();
    let result = if builder == "reference" {
        let decoded = segment::read_sealed(input)?;
        segment::build(state, 1, &decoded)
    } else {
        segment::build_sealed(state, 1, input)
    };
    let build_seconds = began.elapsed().as_secs_f64();
    let heap_peak = PEAK.load(Relaxed);
    let heap_after = LIVE.load(Relaxed);
    let allocated = TOTAL.load(Relaxed);
    let allocation_calls = CALLS.load(Relaxed);
    let stat_after = std::fs::read_to_string("/proc/self/stat")?;
    let io_after = proc_io()?;
    let manifest = match result {
        Ok(m) => m,
        Err(error) => {
            println!(
                "{}",
                json!({"builder":builder,"build_error":error.to_string(),"leftovers":leftovers(state)?,"input_sha256":input_hash})
            );
            return Err(error.into());
        }
    };
    let io_delta: BTreeMap<_, _> = io_after
        .iter()
        .map(|(k, v)| (k.clone(), v.saturating_sub(*io_before.get(k).unwrap_or(&0))))
        .collect();
    let dir = state.join("segments").join(segment::segment_name(1));
    segment::verify(&dir, &manifest)?;
    let mut ledgers = BTreeMap::new();
    let mut counts = BTreeMap::new();
    let mut h = Sha256::new();
    let mut log_times = Vec::new();
    segment::scan_logs(&dir, &manifest, 0, u64::MAX, |row| {
        ledger(&mut h, &serde_json::to_vec(&row).expect("row serializes"));
        log_times.push(row.observed_ns);
    })?;
    counts.insert("logs", log_times.len() as u64);
    ledgers.insert("logs", format!("{:x}", h.finalize()));
    let mut h = Sha256::new();
    let mut metric_times = Vec::new();
    segment::scan_metrics(&dir, &manifest, 0, u64::MAX, |row| {
        ledger(&mut h, &serde_json::to_vec(&row).expect("row serializes"));
        metric_times.push(row.time_ns);
    })?;
    counts.insert("metrics", metric_times.len() as u64);
    ledgers.insert("metrics", format!("{:x}", h.finalize()));
    let mut h = Sha256::new();
    let mut n = 0u64;
    segment::scan_gaps(&dir, &manifest, |row| {
        n += 1;
        ledger(&mut h, format!("{row:?}").as_bytes());
    })?;
    counts.insert("gaps", n);
    ledgers.insert("gaps", format!("{:x}", h.finalize()));
    let mut h = Sha256::new();
    let mut n = 0u64;
    segment::scan_batches(&dir, &manifest, |group, entry| {
        n += 1;
        ledger(&mut h, &group.to_le_bytes());
        ledger(&mut h, &entry.encode_to_vec());
    })?;
    counts.insert("batches", n);
    ledgers.insert("batches", format!("{:x}", h.finalize()));
    let log_bounds = groups(&dir, "logs.parquet", 5)?;
    let metric_bounds = groups(&dir, "metrics.parquet", 9)?;
    let filters = filter_check(&dir, &manifest, &log_bounds)?;
    let pruning_logs = pruning(&mut log_times, &log_bounds);
    let pruning_metrics = pruning(&mut metric_times, &metric_bounds);
    if digest(input)? != input_hash {
        return Err("fixture mutated during build".into());
    }
    Ok(
        json!({"builder":builder,"input_sha256":input_hash,"journal_bytes":journal_bytes,"manifest":manifest,
        "ordered_row_ledgers":ledgers,"logical_row_counts":counts,"filter_check":filters,
        "pruning":{"logs":pruning_logs,"metrics":pruning_metrics},"leftovers":leftovers(state)?,
        "build_seconds":build_seconds,
        "heap_start_bytes":cfg!(feature="responsibility-alloc-probe").then_some(heap_start),
        "peak_live_heap_bytes":cfg!(feature="responsibility-alloc-probe").then_some(heap_peak),
        "incremental_peak_heap_bytes":cfg!(feature="responsibility-alloc-probe").then_some(heap_peak.saturating_sub(heap_start)),
        "heap_after_bytes":cfg!(feature="responsibility-alloc-probe").then_some(heap_after),
        "allocator_counted":cfg!(feature="responsibility-alloc-probe"),
        "spill_workspace_experiment":option_env!("FABRIC_SPILL_WORKSPACE_EXPERIMENT").is_some(),
        "cumulative_requested_bytes":cfg!(feature="responsibility-alloc-probe").then_some(allocated),
        "successful_alloc_realloc_calls":cfg!(feature="responsibility-alloc-probe").then_some(allocation_calls),
        "io_delta":io_delta,"stat_before":stat_before,"stat_after":stat_after,
        "spill_bytes":null,"spill_bytes_reason":"no cumulative spill instrumentation in public builder API",
        "measurement_complete":false,"scope":"C2 differential builder subset; not BS-1 through BS-9 completion"}),
    )
}

#[cfg(all(test, feature = "responsibility-alloc-probe"))]
mod heap_acceptance_tests {
    use super::*;

    #[test]
    fn bounded_registered_steady_128_heap_stays_below_ceiling()
    -> Result<(), Box<dyn std::error::Error>> {
        // The launcher owns the data-drive scratch root. No system-disk fallback.
        // One filtered libtest runs alone, so unrelated tests cannot affect PEAK.
        let scratch_root = PathBuf::from(std::env::var("FABRIC_SCRATCH_ROOT")?).canonicalize()?;
        let work = scratch_root.join(format!("sealer-heap-mutant-{}", std::process::id()));
        std::fs::create_dir(&work)?;
        let fixture = work.join("fixture");
        generate("steady", &fixture, 128 * 1024 * 1024)?;
        let input = fixture.join("sealed-00000000000000000001.faj");
        // run snapshots allocator counters after fixture generation and before
        // readback. This asserts the registered ceiling, not a buffer estimate.
        let measured = run(&work.join("state"), &input, "bounded")?;
        let peak = measured["incremental_peak_heap_bytes"]
            .as_u64()
            .ok_or("counting allocator measurement missing")?;
        let logs = measured["logical_row_counts"]["logs"]
            .as_u64()
            .ok_or("logical log count missing")?;
        let ceiling = 80 * 1024 * 1024_u64;
        let receipts = PathBuf::from(std::env::var("CARGO_TARGET_DIR")?)
            .join("verification/sealer-heap-mutant");
        std::fs::create_dir_all(&receipts)?;
        let receipt = receipts.join(format!("steady-128-{}.json", std::process::id()));
        let mut evidence = json!({
            "origin":"registered BS-3 steady-128; M-SEAL-RETAIN-RUNS sensitivity",
            "seed":0xA11FA001_u64,"input_sha256":measured["input_sha256"],
            "journal_bytes":measured["journal_bytes"],"logical_log_rows":logs,
            "minimum_log_body_bytes":logs * 512,"ceiling_bytes":ceiling,
            "incremental_peak_heap_bytes":peak,
            "heap_start_bytes":measured["heap_start_bytes"],
            "heap_after_bytes":measured["heap_after_bytes"],
            "allocator_counted":measured["allocator_counted"],
            "leftovers":measured["leftovers"],"scratch":work,
            "command":std::env::args().collect::<Vec<_>>(),
            "executable_sha256":digest(&std::env::current_exe()?)?,
            "cleanup_removed":false,
            "scope":"one registered ceiling witness, not full BS-3 acceptance"
        });
        // Preserve the reproducible fixture identity and observed violation
        // before cleaning owned scratch; never dump telemetry bodies on panic.
        std::fs::write(&receipt, serde_json::to_vec_pretty(&evidence)?)?;
        std::fs::remove_dir_all(&work)?;
        evidence["cleanup_removed"] = json!(true);
        std::fs::write(&receipt, serde_json::to_vec_pretty(&evidence)?)?;
        assert!(
            logs * 512 > ceiling,
            "registered fixture is too small to expose retained log payloads"
        );
        assert!(measured["allocator_counted"].as_bool() == Some(true));
        assert!(measured["leftovers"].as_array().is_some_and(Vec::is_empty));
        assert!(
            peak <= ceiling,
            "incremental heap {peak} exceeds {ceiling}; receipt {}",
            receipt.display()
        );
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let scratch = PathBuf::from(std::env::var("FABRIC_SCRATCH_ROOT")?).canonicalize()?;
    if args.len() != 4 && !(args.len() == 3 && args[0] == "entropy-gen") {
        return Err(
            "gen ROOT SHAPE MIB | entropy-gen ROOT 8192 | run NEW_STATE INPUT reference|bounded"
                .into(),
        );
    }
    let destination = PathBuf::from(&args[1]);
    if destination.exists()
        || !destination
            .parent()
            .ok_or("missing parent")?
            .canonicalize()?
            .starts_with(&scratch)
    {
        return Err("fresh destination under scratch required".into());
    }
    match args[0].as_str() {
        "entropy-gen" => {
            let rows: usize = args[2].parse()?;
            if rows != 8192 {
                return Err("encoded-page probe requires exactly 8192 rows".into());
            }
            generate_entropy(&destination, rows)?;
            let input = destination.join("sealed-00000000000000000001.faj");
            println!(
                "{}",
                json!({"shape":"entropy","seed":0xA11FA001u64,
                "rows":rows,"body_bytes":16384,"logical_body_bytes":rows * 16384,
                "journal_bytes":std::fs::metadata(&input)?.len(),
                "input_sha256":digest(&input)?,"input":input})
            );
        }
        "gen" => {
            let mib: u64 = args[3].parse()?;
            if ![16, 32, 64, 128, 256].contains(&mib)
                || !["steady", "outage", "adversarial", "bigrows"].contains(&args[2].as_str())
            {
                return Err("unsupported registered fixture".into());
            }
            generate(&args[2], &destination, mib * 1024 * 1024)?;
            let input = destination.join("sealed-00000000000000000001.faj");
            println!(
                "{}",
                json!({"shape":args[2],"target_mib":mib,"seed":0xA11FA001u64,
                "journal_bytes":std::fs::metadata(&input)?.len(),"input_sha256":digest(&input)?,"input":input})
            );
        }
        "run" => {
            if !["reference", "bounded"].contains(&args[3].as_str()) {
                return Err("unknown builder".into());
            }
            let input = PathBuf::from(&args[2]).canonicalize()?;
            if !input.starts_with(&scratch) {
                return Err("input outside scratch".into());
            }
            println!("{}", run(&destination, &input, &args[3])?);
        }
        _ => return Err("unknown command".into()),
    }
    Ok(())
}
