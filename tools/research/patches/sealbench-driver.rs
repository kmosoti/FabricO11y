//! Scratch study driver. `gen`, `run`, `verify`.
//!   sealbench gen <steady|outage|adversarial> <dir> <total-bytes> <file-bytes>
//!   sealbench run <alg> <out-dir> <sealed-file>...
//!   sealbench verify <base-seg-dir> <cand-seg-dir>

use fabric_frame::envelope::Batch;
use fabric_frame::frame::FrameLog;
use fabric_server::rows::{GapRow, LogRow, MetricRow};
use fabric_server::sealbench::*;
use fabric_server::segment::{self, MAX_GROUP_PAYLOAD};
use fabric_server::store::{Entry, Group};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, metric, number_data_point,
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::statistics::Statistics;
use prost::Message;
use sha2::{Digest, Sha256};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

struct Counting;
static CUR: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let c = CUR.fetch_add(l.size(), Relaxed) + l.size();
            PEAK.fetch_max(c, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        CUR.fetch_sub(l.size(), Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, n) };
        if !q.is_null() {
            if n > l.size() {
                let c = CUR.fetch_add(n - l.size(), Relaxed) + n - l.size();
                PEAK.fetch_max(c, Relaxed);
            } else {
                CUR.fetch_sub(l.size() - n, Relaxed);
            }
        }
        q
    }
}
#[global_allocator]
static A: Counting = Counting;

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
static CORPUS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
fn body(rng: &mut Rng, tick: u64) -> String {
    // SEALBENCH_CORPUS=<file>: real log lines, picked at random, instead of the synthetic body.
    let corpus = CORPUS.get_or_init(|| match std::env::var("SEALBENCH_CORPUS") {
        Ok(path) => std::fs::read_to_string(path).expect("corpus").lines().filter(|l| !l.is_empty()).map(str::to_owned).collect(),
        Err(_) => Vec::new(),
    });
    if !corpus.is_empty() {
        // SEALBENCH_CORPUS_ORDER=stream: each caller walks the corpus in order from an
        // offset drawn once per generator (locality as in a real stream); otherwise random.
        static STREAM: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        static POS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let stream = *STREAM.get_or_init(|| std::env::var("SEALBENCH_CORPUS_ORDER").as_deref() == Ok("stream"));
        let i = if stream { POS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + tick } else { rng.next() };
        return corpus[(i % corpus.len() as u64) as usize].clone();
    }
    let n = BODY.load(Relaxed);
    if tick % 2 == 0 {
        return "R".repeat(n);
    }
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    (0..n).map(|_| T[(rng.next() % 64) as usize] as char).collect()
}

/// One node-second batch; `obs` gives each row's time.
fn make_batch(rng: &mut Rng, node: usize, seq: u64, second: u64, obs: &mut dyn FnMut() -> u64, gap_only: bool) -> Vec<u8> {
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
    let metrics = if second % 15 == 0 {
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
    let (logs, metrics, gaps) = if gap_only {
        (Vec::new(), Vec::new(), vec![format!("gap-only {seq}")])
    } else {
        (logs, metrics, if seq % 500 == 0 { vec![format!("gap at {seq}")] } else { Vec::new() })
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
        collection_gaps: gaps,
    }
    .encode_to_vec()
}

fn generate(kind: &str, dir: &Path, total: u64, file_bytes: u64) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    if kind == "bigrows" {
        BODY.store(16384, Relaxed);
    }
    if kind == "tiny" {
        BODY.store(16, Relaxed);
    }
    let mut needle_seqs: std::collections::HashMap<usize, u64> = std::collections::HashMap::new();
    let mut log = FrameLog::open(dir, u64::MAX / 4, MAX_GROUP_PAYLOAD, |_, _| Ok(()))?;
    let mut rng = Rng(0xA11FA001);
    let mut seqs = [0u64; NODES];
    let mut backlog: Vec<VecDeque<u64>> = vec![VecDeque::new(); NODES];
    let (mut group_seq, mut first_in_file, mut sealed_total) = (1u64, 1u64, 0u64);
    let span_s = 520u64; // approximate duration of 64 MiB, for adversarial times
    for s in 0.. {
        // Which (node, batch-second) pairs are sent this second.
        let mut sends: Vec<(usize, u64)> = Vec::new();
        for n in 0..NODES {
            let offline = kind == "outage" && n < 20 && (120..360).contains(&s);
            if offline {
                backlog[n].push_back(s);
                continue;
            }
            for _ in 0..10 {
                match backlog[n].pop_front() {
                    Some(old) => sends.push((n, old)),
                    None => break,
                }
            }
            sends.push((n, s));
            if kind == "tiny" {
                for _ in 0..99 {
                    sends.push((n, s));
                }
            }
        }
        for tick in 0..20u64 {
            let mut entries = Vec::new();
            for &(n, bs) in sends.iter().filter(|(n, _)| (*n as u64) % 20 == tick) {
                // needle: 1,000 identities, each sending every tenth second
                let ident = if kind == "needle" { n * 10 + (bs % 10) as usize } else { n };
                let seq = if kind == "needle" {
                    let e = needle_seqs.entry(ident).or_insert(0);
                    *e += 1;
                    *e
                } else {
                    seqs[n] += 1;
                    seqs[n]
                };
                let jitter = (n as u64 * 7919 % 50) * 1_000_000;
                let mut r2 = Rng(rng.next() | 1);
                let mut obs = || {
                    if kind == "adversarial" {
                        T0 + r2.next() % (span_s * 1_000_000_000)
                    } else if kind == "skew" && n % 10 == 0 {
                        // the node clock runs 5 s ahead of the server clock
                        T0 + (bs + 5) * 1_000_000_000 + jitter
                    } else {
                        T0 + bs * 1_000_000_000 + jitter
                    }
                };
                let batch = make_batch(&mut rng, ident, seq, bs, &mut obs, kind == "skew" && n % 25 == 7);
                entries.push(Entry {
                    label: format!("node-{ident:04}"),
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
                sealed_total += log.active_bytes();
                log.rotate(first_in_file)?;
                first_in_file = group_seq;
                if sealed_total >= total {
                    return Ok(());
                }
            }
            log.append(&payload)?;
            group_seq += 1;
            if file_bytes > total && log.active_bytes() >= total {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn proc_field(path: &str, key: &str) -> u64 {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix(key).map(|v| v.trim().trim_end_matches(" kB").parse().unwrap_or(0)))
        .unwrap_or(0)
}

fn cpu_ticks() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/self/stat").unwrap();
    let after = &s[s.rfind(')').unwrap() + 2..];
    let f: Vec<&str> = after.split(' ').collect();
    (f[11].parse().unwrap(), f[12].parse().unwrap())
}

fn label_of(p: &Path) -> u64 {
    p.file_name().unwrap().to_str().unwrap().trim_start_matches("sealed-").trim_end_matches(".faj").parse().unwrap()
}

fn run(alg: &str, out: &Path, inputs: &[PathBuf]) -> io::Result<()> {
    std::fs::create_dir_all(out)?;
    let base_heap = CUR.load(Relaxed);
    PEAK.store(base_heap, Relaxed);
    let (r0, w0) = (proc_field("/proc/self/io", "rchar:"), proc_field("/proc/self/io", "wchar:"));
    let (u0, s0) = cpu_ticks();
    let t = Instant::now();
    let mut rep = Report::default();
    let mut out_bytes = 0u64;
    let (name, param) = alg.split_once(':').unwrap_or((alg, "0"));
    let param: usize = param.parse().unwrap();
    for input in inputs {
        let label = label_of(input);
        let manifest = match name {
            "base" => {
                let groups = segment::read_sealed(input)?;
                let m = segment::build(out, label, &groups)?;
                drop(groups);
                m
            }
            "chunk" => build_streaming(input, out, label, &mut ChunkSort::<LogRow>::new(), &mut ChunkSort::<MetricRow>::new())?,
            "watermark" => {
                let (mut l, mut m) = (Watermark::<LogRow>::new(param), Watermark::<MetricRow>::new(param));
                let man = build_streaming(input, out, label, &mut l, &mut m)?;
                rep.late_logs += l.late_rows;
                rep.late_metrics += m.late_rows;
                man
            }
            "partition" => {
                let (ls, ms, kb) = splitters(input, param)?;
                rep.key_index_bytes = rep.key_index_bytes.max(kb);
                let dir = out.to_path_buf();
                let (mut l, mut m) = (RangePartition::<LogRow>::new(ls, dir.clone(), "l"), RangePartition::<MetricRow>::new(ms, dir, "m"));
                let man = build_streaming(input, out, label, &mut l, &mut m)?;
                rep.spill_bytes += l.spill_bytes + m.spill_bytes;
                man
            }
            "merge" => {
                let dir = out.to_path_buf();
                let (mut l, mut m) = (ExternalMerge::<LogRow>::new(param, dir.clone(), "l"), ExternalMerge::<MetricRow>::new(param, dir, "m"));
                let man = build_streaming(input, out, label, &mut l, &mut m)?;
                rep.spill_bytes += l.spill_bytes + m.spill_bytes;
                man
            }
            "hybrid" => {
                let dir = out.to_path_buf();
                let (mut l, mut m) = (Hybrid::<LogRow>::new(param, 32768, dir.clone(), "l"), Hybrid::<MetricRow>::new(param, 32768, dir, "m"));
                let man = build_streaming(input, out, label, &mut l, &mut m)?;
                rep.late_logs += l.late_rows;
                rep.late_metrics += m.late_rows;
                rep.spill_bytes += l.late.spill_bytes + m.late.spill_bytes;
                man
            }
            _ => panic!("unknown algorithm {alg}"),
        };
        out_bytes += manifest.files.values().map(|f| f.bytes).sum::<u64>();
    }
    let wall = t.elapsed().as_secs_f64();
    let (u1, s1) = cpu_ticks();
    let (r1, w1) = (proc_field("/proc/self/io", "rchar:"), proc_field("/proc/self/io", "wchar:"));
    let input_bytes: u64 = inputs.iter().map(|p| std::fs::metadata(p).unwrap().len()).sum();
    println!(
        "{}",
        serde_json::json!({
            "alg": alg, "files": inputs.len(), "input_bytes": input_bytes,
            "wall_s": wall, "cpu_user_s": (u1 - u0) as f64 / 100.0, "cpu_sys_s": (s1 - s0) as f64 / 100.0,
            "peak_heap_bytes": PEAK.load(Relaxed) - base_heap,
            "vmhwm_kib": proc_field("/proc/self/status", "VmHWM:"),
            "read_bytes": r1 - r0, "write_bytes": w1 - w0,
            "output_bytes": out_bytes, "spill_bytes": rep.spill_bytes,
            "key_index_bytes": rep.key_index_bytes,
            "late_logs": rep.late_logs, "late_metrics": rep.late_metrics,
        })
    );
    Ok(())
}

fn h(hash: &mut Sha256, r: &impl SortRow) {
    let mut v = Vec::new();
    r.encode(&mut v);
    hash.update((v.len() as u32).to_le_bytes());
    hash.update(&v);
}

/// Rows read per row matched, over sliding windows.
fn amplification(dir: &Path, file: &str, col: usize, times: &[u64], window_s: u64) -> (f64, usize, f64) {
    let b = ParquetRecordBatchReaderBuilder::try_new(File::open(dir.join(file)).unwrap()).unwrap();
    let rgs: Vec<(u64, u64, u64)> = b
        .metadata()
        .row_groups()
        .iter()
        .map(|rg| match rg.column(col).statistics() {
            Some(Statistics::Int64(s)) => (*s.min_opt().unwrap() as u64, *s.max_opt().unwrap() as u64, rg.num_rows() as u64),
            _ => panic!("no stats"),
        })
        .collect();
    let (lo, hi) = (times[0], *times.last().unwrap());
    let w = window_s * 1_000_000_000;
    let (mut read, mut matched) = (0u64, 0u64);
    let mut from = lo;
    while from <= hi {
        let to = from + w;
        let m = (times.partition_point(|t| *t < to) - times.partition_point(|t| *t < from)) as u64;
        if m > 0 {
            matched += m;
            read += rgs.iter().filter(|(mn, mx, _)| *mx >= from && *mn < to).map(|r| r.2).sum::<u64>();
        }
        from += w / 2;
    }
    // overlap: mean number of row groups covering a random row's time
    let cover: f64 = times.iter().step_by(97).map(|t| rgs.iter().filter(|(mn, mx, _)| mn <= t && t <= mx).count() as f64).sum::<f64>()
        / times.iter().step_by(97).count() as f64;
    (read as f64 / matched as f64, rgs.len(), cover)
}

fn verify(base: &Path, cand: &Path) -> io::Result<()> {
    let (mb, mc) = (segment::read_manifest(base)?, segment::read_manifest(cand)?);
    let same_manifest = mb.first_group == mc.first_group && mb.last_group == mc.last_group && mb.records == mc.records
        && mb.received_min_ns == mc.received_min_ns && mb.received_max_ns == mc.received_max_ns && mb.freshness == mc.freshness;
    let mut out = serde_json::Map::new();
    out.insert("manifest_equal".into(), same_manifest.into());
    // logs
    for (name, which) in [("logs", 0), ("metrics", 1)] {
        let (mut ob, mut oc) = (Sha256::new(), Sha256::new());
        let (mut kb, mut kc): (Vec<(Key, Vec<u8>)>, Vec<(Key, Vec<u8>)>) = (Vec::new(), Vec::new());
        let mut times = Vec::new();
        let enc = |r: &dyn Fn(&mut Vec<u8>)| { let mut v = Vec::new(); r(&mut v); v };
        if which == 0 {
            segment::scan_logs(base, &mb, 0, u64::MAX, |r: LogRow| { h(&mut ob, &r); times.push(r.observed_ns); kb.push((r.key(), enc(&|v| r.encode(v)))); })?;
            segment::scan_logs(cand, &mc, 0, u64::MAX, |r: LogRow| { h(&mut oc, &r); kc.push((r.key(), enc(&|v| r.encode(v)))); })?;
        } else {
            segment::scan_metrics(base, &mb, 0, u64::MAX, |r: MetricRow| { h(&mut ob, &r); times.push(r.time_ns); kb.push((r.key(), enc(&|v| r.encode(v)))); })?;
            segment::scan_metrics(cand, &mc, 0, u64::MAX, |r: MetricRow| { h(&mut oc, &r); kc.push((r.key(), enc(&|v| r.encode(v)))); })?;
        }
        kb.sort();
        kc.sort();
        times.sort_unstable();
        let file = format!("{name}.parquet");
        let col = if which == 0 { 5 } else { 9 };
        let (amp60, rgs, cover) = amplification(cand, &file, col, &times, 60);
        let (amp10, _, _) = amplification(cand, &file, col, &times, 10);
        out.insert(name.into(), serde_json::json!({
            "rows": kc.len(), "rows_equal": kb.len() == kc.len(),
            "same_rows": kb == kc, "same_order": ob.finalize() == oc.finalize(),
            "byte_identical": mb.files[&file].sha256 == mc.files[&file].sha256,
            "row_groups": rgs, "read_amp_60s": amp60, "read_amp_10s": amp10, "rowgroups_per_point": cover,
            "bytes": mc.files[&file].bytes,
        }));
    }
    // gaps and batches: exact order
    let (mut gb, mut gc) = (Sha256::new(), Sha256::new());
    let gh = |hh: &mut Sha256, r: GapRow| hh.update(format!("{}|{}|{:?}|{}|{}|{}", r.group, r.node, r.node_id, r.sequence, r.received_ns, r.text));
    segment::scan_gaps(base, &mb, |r| gh(&mut gb, r))?;
    segment::scan_gaps(cand, &mc, |r| gh(&mut gc, r))?;
    let (mut bb, mut bc) = (Sha256::new(), Sha256::new());
    let bh = |hh: &mut Sha256, g: u64, e: Entry| { hh.update(g.to_le_bytes()); hh.update(e.label.as_bytes()); hh.update(e.received_unix_nano.to_le_bytes()); hh.update(&e.batch); };
    segment::scan_batches(base, &mb, |g, e| bh(&mut bb, g, e))?;
    segment::scan_batches(cand, &mc, |g, e| bh(&mut bc, g, e))?;
    out.insert("gaps_same".into(), (gb.finalize() == gc.finalize()).into());
    out.insert("batches_same".into(), (bb.finalize() == bc.finalize()).into());
    segment::verify(cand, &mc)?;
    out.insert("hashes_verified".into(), true.into());
    println!("{}", serde_json::Value::Object(out));
    Ok(())
}

fn main() -> io::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    match a[1].as_str() {
        "gen" => generate(&a[2], Path::new(&a[3]), a[4].parse().unwrap(), a[5].parse().unwrap()),
        "fob" => fob(Path::new(&a[2]), a.get(3).map(|n| n.parse().unwrap()).unwrap_or(64)),
        "fobscan" => fobscan(Path::new(&a[2]), Path::new(&a[3]), &a[4], a.get(5).map(|n| n.parse().unwrap()).unwrap_or(64)),
        "run" => run(&a[2], Path::new(&a[3]), &a[4..].iter().map(PathBuf::from).collect::<Vec<_>>()),
        "verify" => verify(Path::new(&a[2]), Path::new(&a[3])),
        _ => panic!("usage"),
    }
}


/// Measures the FOB1 observation encoding against the raw OTLP Batch bytes
/// over the first `n` sealed files of a journal directory.
fn fob(dir: &Path, n: usize) -> io::Result<()> {
    use fabric_observation as fo;
    use fabric_server::rows::{Rows, extract, Number as RN};
    use prost::Message as _;
    let mut files: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("sealed-") && n.ends_with(".faj"))).collect();
    files.sort(); files.truncate(n);
    let (mut raw, mut raw_z, mut per_batch, mut per_batch_z, mut per_file, mut per_file_z, mut lines, mut points, mut batches, mut parquet_like) = (0u64,0u64,0u64,0u64,0u64,0u64,0u64,0u64,0u64,0u64);
    let mut raw_concat_z = 0u64;
    let mut t_enc = std::time::Duration::ZERO; let mut t_dec = std::time::Duration::ZERO;
    for f in &files {
        let groups = segment::read_sealed(f)?;
        let mut file_records: Vec<fo::Observation> = Vec::new();
        let mut raw_concat: Vec<u8> = Vec::new();
        let mut fob_concat: Vec<u8> = Vec::new();
        for g in &groups { for e in &g.entries {
            batches += 1; raw += e.batch.len() as u64; raw_concat.extend_from_slice(&e.batch);
            raw_z += zstd::bulk::compress(&e.batch, 3)?.len() as u64;
            let batch = fabric_frame::envelope::Batch::decode(e.batch.as_slice()).map_err(|x| io::Error::new(io::ErrorKind::InvalidData, x.to_string()))?;
            let mut rows = Rows::default(); extract(g.group_sequence, e, &mut rows)?;
            let strand = fo::Strand { node_id: batch.node_id.as_slice().try_into().unwrap(), generation: batch.generation };
            let mut recs: Vec<fo::Observation> = Vec::new();
            for r in &rows.logs { lines += 1; recs.push(fo::Observation { strand, sequence: r.sequence, index: r.index, time_ns: r.observed_ns, locators: None,
                attributes: r.attributes.iter().map(|(k, v)| (k.clone(), fo::Value::Str(v.clone()))).collect(),
                signal: fo::Signal::Log { severity: 0, event: String::new(), body: r.body.clone() } }); }
            for r in &rows.metrics { points += 1; recs.push(fo::Observation { strand, sequence: r.sequence, index: r.index, time_ns: r.time_ns, locators: None,
                attributes: r.attributes.iter().map(|(k, v)| (k.clone(), fo::Value::Str(v.clone()))).collect(),
                signal: fo::Signal::Point { name: r.name.clone(), unit: r.unit.clone(), kind: if r.sum { fo::PointKind::Sum { monotonic: r.monotonic, start_ns: r.start_ns } } else { fo::PointKind::Gauge },
                    value: match r.value { RN::Int(i) => fo::Number::Int(i), RN::Double(d) => fo::Number::Double(d) } } }); }
            if recs.is_empty() { continue; }
            let t = std::time::Instant::now(); let bytes = fo::encode(&recs).map_err(|x| io::Error::new(io::ErrorKind::InvalidData, x.to_string()))?; t_enc += t.elapsed();
            let t = std::time::Instant::now(); let back = fo::decode(&bytes).map_err(|x| io::Error::new(io::ErrorKind::InvalidData, x.to_string()))?; t_dec += t.elapsed();
            assert_eq!(back, recs);
            per_batch += bytes.len() as u64; per_batch_z += zstd::bulk::compress(&bytes, 3)?.len() as u64; fob_concat.extend_from_slice(&bytes);
            file_records.extend(recs);
        } }
        raw_concat_z += zstd::bulk::compress(&raw_concat, 3)?.len() as u64;
        parquet_like += zstd::bulk::compress(&fob_concat, 3)?.len() as u64;
        // one block per file (chunks of MAX_RECORDS)
        for chunk in file_records.chunks(fo::MAX_RECORDS) {
            let bytes = fo::encode(chunk).map_err(|x| io::Error::new(io::ErrorKind::InvalidData, x.to_string()))?;
            per_file += bytes.len() as u64; per_file_z += zstd::bulk::compress(&bytes, 3)?.len() as u64;
        }
    }
    let recs = lines + points;
    println!("{}", serde_json::json!({
        "files": files.len(), "batches": batches, "lines": lines, "points": points, "records": recs,
        "raw_batch_bytes": raw, "raw_batch_zstd_each": raw_z, "raw_batch_zstd_per_file": raw_concat_z,
        "fob_per_batch": per_batch, "fob_per_batch_zstd_each": per_batch_z, "fob_per_batch_zstd_per_file": parquet_like,
        "fob_per_file": per_file, "fob_per_file_zstd": per_file_z,
        "bytes_per_record": {"raw": raw as f64 / recs as f64, "raw_zstd_per_file": raw_concat_z as f64 / recs as f64, "fob_per_batch": per_batch as f64 / recs as f64, "fob_per_file": per_file as f64 / recs as f64, "fob_per_file_zstd": per_file_z as f64 / recs as f64},
        "encode_ns_per_record": t_enc.as_nanos() as f64 / recs as f64, "decode_ns_per_record": t_dec.as_nanos() as f64 / recs as f64
    }));
    Ok(())
}


/// Text search over one compressed canonical copy (FOB1 blocks, one per journal
/// file, Zstd 3) through `decode_view`, against the Parquet logs projection of the
/// same records. Reports the median of eight passes and the bytes each layout reads.
fn fobscan(journal_dir: &Path, segments_dir: &Path, token: &str, n: usize) -> io::Result<()> {
    use fabric_observation as fo;
    use fabric_server::rows::{Rows, extract};
    use prost::Message as _;
    let mut files: Vec<_> = std::fs::read_dir(journal_dir)?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("sealed-") && n.ends_with(".faj"))).collect();
    files.sort(); files.truncate(n);
    // one block per file, compressed as a Segment would store it
    let mut blocks: Vec<Vec<u8>> = Vec::new(); let mut raw_total = 0usize; let mut records = 0usize; let mut lines = 0usize;
    for f in &files {
        let groups = segment::read_sealed(f)?; let mut recs = Vec::new();
        for g in &groups { for e in &g.entries {
            let batch = fabric_frame::envelope::Batch::decode(e.batch.as_slice()).map_err(|x| io::Error::new(io::ErrorKind::InvalidData, x.to_string()))?;
            let mut rows = Rows::default(); extract(g.group_sequence, e, &mut rows)?;
            let strand = fo::Strand { node_id: batch.node_id.as_slice().try_into().unwrap(), generation: batch.generation };
            for r in &rows.logs { lines += 1; recs.push(fo::Observation { strand, sequence: r.sequence, index: r.index, time_ns: r.observed_ns, locators: None, attributes: r.attributes.iter().map(|(k, v)| (k.clone(), fo::Value::Str(v.clone()))).collect(), signal: fo::Signal::Log { severity: 0, event: String::new(), body: r.body.clone() } }); }
            for r in &rows.metrics { recs.push(fo::Observation { strand, sequence: r.sequence, index: r.index, time_ns: r.time_ns, locators: None, attributes: r.attributes.iter().map(|(k, v)| (k.clone(), fo::Value::Str(v.clone()))).collect(), signal: fo::Signal::Point { name: r.name.clone(), unit: r.unit.clone(), kind: fo::PointKind::Gauge, value: fo::Number::Int(0) } }); }
        } }
        records += recs.len();
        for chunk in recs.chunks(fo::MAX_RECORDS) {
            let bytes = fo::encode(chunk).map_err(|x| io::Error::new(io::ErrorKind::InvalidData, x.to_string()))?; raw_total += bytes.len();
            blocks.push(zstd::bulk::compress(&bytes, 3)?);
        }
    }
    let compressed_total: usize = blocks.iter().map(|b| b.len()).sum();
    let median = |mut v: Vec<f64>| { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[v.len() / 2] };
    // FOB1: decompress, view-decode, substring over bodies
    let mut fob_ms = Vec::new(); let mut fob_hits = 0usize;
    for _ in 0..8 {
        let t = std::time::Instant::now(); let mut hits = 0usize;
        for b in &blocks {
            let raw = zstd::bulk::decompress(b, 256 << 20)?;
            let view = fo::decode_view(&raw).map_err(|x| io::Error::new(io::ErrorKind::InvalidData, x.to_string()))?;
            for r in &view { if let fo::SignalRef::Log { body, .. } = r.signal { if body.contains(token) { hits += 1; } } }
        }
        fob_ms.push(t.elapsed().as_secs_f64() * 1e3); fob_hits = hits;
    }
    // FOB1, keys only: decompress and view-decode without the search (the fixed cost)
    let mut fobkeys_ms = Vec::new();
    for _ in 0..8 {
        let t = std::time::Instant::now(); let mut n_rows = 0usize;
        for b in &blocks { let raw = zstd::bulk::decompress(b, 256 << 20)?; n_rows += fo::decode_view(&raw).map_err(|x| io::Error::new(io::ErrorKind::InvalidData, x.to_string()))?.len(); }
        assert_eq!(n_rows, records); fobkeys_ms.push(t.elapsed().as_secs_f64() * 1e3);
    }
    // Parquet projection: the stock scan over every Segment's logs table
    let mut segs: Vec<_> = std::fs::read_dir(segments_dir)?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("seg-"))).collect();
    segs.sort(); segs.truncate(n);
    let manifests: Vec<_> = segs.iter().map(|d| segment::read_manifest(d)).collect::<io::Result<_>>()?;
    let parquet_bytes: u64 = segs.iter().map(|d| std::fs::metadata(d.join("logs.parquet")).map(|m| m.len()).unwrap_or(0)).sum();
    let mut pq_ms = Vec::new(); let mut pq_hits = 0usize;
    for _ in 0..8 {
        let t = std::time::Instant::now(); let mut hits = 0usize;
        for (d, m) in segs.iter().zip(&manifests) { segment::scan_logs(d, m, 0, u64::MAX, |r| { if r.body.contains(token) { hits += 1; } })?; }
        pq_ms.push(t.elapsed().as_secs_f64() * 1e3); pq_hits = hits;
    }
    println!("{}", serde_json::json!({
        "files": files.len(), "records": records, "lines": lines, "token": token,
        "fob1_raw_bytes": raw_total, "fob1_zstd_bytes": compressed_total, "parquet_logs_bytes": parquet_bytes,
        "fob1_search_ms": median(fob_ms.clone()), "fob1_search_min_ms": fob_ms.iter().cloned().fold(f64::MAX, f64::min),
        "fob1_keys_only_ms": median(fobkeys_ms), "parquet_search_ms": median(pq_ms.clone()), "parquet_search_min_ms": pq_ms.iter().cloned().fold(f64::MAX, f64::min),
        "fob1_hits": fob_hits, "parquet_hits": pq_hits
    }));
    Ok(())
}
