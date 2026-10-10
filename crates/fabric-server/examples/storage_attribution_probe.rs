//! Example-local phase attribution; copied fixture/ledgers, production unchanged.
//! storage_attribution_probe <new-root> <shape> <MiB> <seed> <reference|bounded>
//! Generation allocations are dropped before counted build measurement.
use fabric_frame::envelope::Batch;
use fabric_server::{
    segment,
    store::{Entry, Group},
};
use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
    trace::v1::ExportTraceServiceRequest,
};
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, metric, number_data_point,
};
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use sha2::{Digest, Sha256};
use std::alloc::{GlobalAlloc, Layout, System};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

struct Counted;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
fn add(n: usize) {
    let live = LIVE.fetch_add(n, Relaxed) + n;
    PEAK.fetch_max(live, Relaxed);
}
// These methods delegate allocation/layout validity to System. Counters never
// allocate, and unsuccessful allocations/reallocations do not change the ledger.
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            add(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
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
static ALLOCATOR: Counted = Counted;
const START: u64 = 1_800_000_000_000_000_000;
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}
fn text(value: String) -> Option<AnyValue> {
    Some(AnyValue {
        value: Some(any_value::Value::StringValue(value)),
    })
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 5 {
        return Err("expected new-root shape MiB seed builder".into());
    }
    let dir = PathBuf::from(&args[0]);
    let shape = args[1].as_str();
    if !["steady", "outage", "adversarial", "bigrows"].contains(&shape) {
        return Err("unknown shape".into());
    }
    let mib: usize = args[2].parse()?;
    if ![16, 32, 64, 128, 256].contains(&mib) {
        return Err("unsupported MiB".into());
    }
    let body_size = if shape == "bigrows" { 16384 } else { 512 };
    let rows_per_batch = if shape == "bigrows" { 32u64 } else { 64u64 };
    let builder = args[4].as_str();
    if !["reference", "bounded"].contains(&builder) {
        return Err("unknown builder".into());
    }
    let seed: u64 = args[3].parse()?;
    if seed == 0 {
        return Err("seed must be nonzero".into());
    }
    if dir.exists() {
        return Err("state directory must not exist".into());
    }
    let mut rng = seed;
    std::fs::create_dir(&dir)?;
    let journal = dir.join("input");
    std::fs::create_dir(&journal)?;
    let mut log = fabric_frame::frame::FrameLog::open(
        &journal,
        1024 * 1024 * 1024,
        segment::MAX_GROUP_PAYLOAD,
        |_, _| Ok(()),
    )?;
    let mut groups = 0u64;
    let mut encoded_bytes = 0;
    let mut input_hash = Sha256::new();
    while encoded_bytes < mib * 1024 * 1024 {
        let seq = groups + 1;
        let node = (seq - 1) % 100;
        let arrival = ((seq - 1) / 100) * 32;
        let observed = if shape == "outage" && node < 20 && (240..480).contains(&arrival) {
            arrival - 240
        } else {
            arrival
        };
        let logs = (0..rows_per_batch)
            .map(|row| {
                let body: String = (0..body_size)
                    .map(|_| {
                        if (seq * 64 + row) % 2 == 1 {
                            (b'!' + (next(&mut rng) % 90) as u8) as char
                        } else {
                            'R'
                        }
                    })
                    .collect();
                LogRecord {
                    observed_time_unix_nano: START
                        + if shape == "adversarial" {
                            next(&mut rng) % 520_000_000_000
                        } else {
                            (observed + row / 2) * 1_000_000_000
                        },
                    body: text(body),
                    attributes: vec![
                        KeyValue {
                            key: "source".into(),
                            value: text(format!("/logs/{node}.log")),
                            ..Default::default()
                        },
                        KeyValue {
                            key: "offset".into(),
                            value: text((seq * 64 + row).to_string()),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }
            })
            .collect();
        let logs = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                scope_logs: vec![ScopeLogs {
                    log_records: logs,
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
        .encode_to_vec();
        let metrics = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                scope_metrics: vec![ScopeMetrics {
                    metrics: vec![Metric {
                        name: "fixture.value".into(),
                        data: Some(metric::Data::Gauge(Gauge {
                            data_points: (0..32)
                                .map(|point| NumberDataPoint {
                                    time_unix_nano: START
                                        + if shape == "adversarial" {
                                            next(&mut rng) % 520_000_000_000
                                        } else {
                                            observed * 1_000_000_000
                                        },
                                    value: Some(number_data_point::Value::AsInt(
                                        (seq * 32 + point) as i64,
                                    )),
                                    ..Default::default()
                                })
                                .collect(),
                        })),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
        .encode_to_vec();
        let traces = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                scope_spans: vec![ScopeSpans {
                    spans: vec![Span {
                        trace_id: vec![7; 16],
                        span_id: seq.to_be_bytes().to_vec(),
                        name: "fixture.span".into(),
                        kind: 2,
                        start_time_unix_nano: START + observed * 1_000_000_000,
                        end_time_unix_nano: START + observed * 1_000_000_000 + 1,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
        .encode_to_vec();
        let batch = Batch {
            version: 1,
            node_id: vec![node as u8; 16],
            generation: 1,
            sequence: (seq - 1) / 100 + 1,
            logs,
            metrics,
            traces,
            collection_gaps: if seq.is_multiple_of(500) {
                vec!["fixture gap".into()]
            } else {
                vec![]
            },
            ..Default::default()
        };
        batch.validate()?;
        let batch_bytes = batch.encode_to_vec();
        if batch_bytes.len() > fabric_server::store::MAX_BATCH_BYTES {
            return Err("fixture Batch exceeds production intake limit".into());
        }
        let group = Group {
            group_sequence: seq,
            entries: vec![Entry {
                label: format!("node-{node}"),
                batch: batch_bytes,
                received_unix_nano: START + arrival * 1_000_000_000,
            }],
        };
        let encoded = group.encode_to_vec();
        if encoded.len() > segment::MAX_GROUP_PAYLOAD {
            return Err("fixture Group exceeds production frame limit".into());
        }
        input_hash.update((encoded.len() as u64).to_le_bytes());
        input_hash.update(&encoded);
        encoded_bytes += encoded.len();
        log.append(&encoded)?;
        groups += 1;
    }
    drop(log);
    let input = journal.join(fabric_frame::frame::ACTIVE);
    let input_sha256 = format!("{:x}", input_hash.finalize());
    let journal_bytes = std::fs::metadata(&input)?.len();
    let state = dir.join("state");
    let observed = std::env::var("FABRIC_STORAGE_ATTRIBUTION_OBSERVE")
        .ok()
        .as_deref()
        == Some("1");
    if observed && !cfg!(feature = "phase-probe") {
        return Err("observed arm requires phase-probe feature".into());
    }
    let before_observer = LIVE.load(Relaxed);
    #[cfg(feature = "phase-probe")]
    if observed {
        // Existing ledger preallocates 262144 records once. Its fixed live bytes
        // are part of heap_start, outside incremental counted build memory.
        fabric_frame::probe::install(|| {
            [0, LIVE.load(Relaxed) as u64, PEAK.load(Relaxed) as u64, 0]
        });
    }
    let observer_live_bytes = LIVE.load(Relaxed).saturating_sub(before_observer);
    let heap_start = LIVE.load(Relaxed);
    PEAK.store(heap_start, Relaxed);
    let io_before = std::fs::read_to_string("/proc/self/io").ok();
    let stat_before = std::fs::read_to_string("/proc/self/stat").ok();
    let started = Instant::now();
    let manifest = if builder == "reference" {
        let decoded = segment::read_sealed(&input)?;
        segment::build(&state, 1, &decoded)?
    } else {
        segment::build_sealed(&state, 1, &input)?
    };
    let build_seconds = started.elapsed().as_secs_f64();
    let peak = PEAK.load(Relaxed);
    let heap_after = LIVE.load(Relaxed);
    #[cfg(feature = "phase-probe")]
    let phase_records = fabric_frame::probe::take();
    let io_after = std::fs::read_to_string("/proc/self/io").ok();
    let stat_after = std::fs::read_to_string("/proc/self/stat").ok();
    let output = state.join("segments").join(segment::segment_name(1));
    segment::verify(&output, &manifest)?;
    let mut ledgers = std::collections::BTreeMap::new();
    let mut h = Sha256::new();
    segment::scan_logs(&output, &manifest, 0, u64::MAX, |row| {
        ledger(&mut h, &serde_json::to_vec(&row).unwrap())
    })?;
    ledgers.insert("logs", format!("{:x}", h.finalize()));
    let mut h = Sha256::new();
    segment::scan_metrics(&output, &manifest, 0, u64::MAX, |row| {
        ledger(&mut h, &serde_json::to_vec(&row).unwrap())
    })?;
    ledgers.insert("metrics", format!("{:x}", h.finalize()));
    let mut h = Sha256::new();
    segment::scan_spans(&output, &manifest, 0, u64::MAX, |row| {
        ledger(&mut h, &serde_json::to_vec(&row).unwrap())
    })?;
    ledgers.insert("spans", format!("{:x}", h.finalize()));
    let mut h = Sha256::new();
    segment::scan_gaps(&output, &manifest, |row| {
        ledger(&mut h, format!("{row:?}").as_bytes())
    })?;
    ledgers.insert("gaps", format!("{:x}", h.finalize()));
    let mut h = Sha256::new();
    segment::scan_batches(&output, &manifest, |group, entry| {
        ledger(&mut h, &group.to_le_bytes());
        ledger(&mut h, &entry.encode_to_vec());
    })?;
    ledgers.insert("batches", format!("{:x}", h.finalize()));
    let segment_entries =
        std::fs::read_dir(state.join("segments"))?.collect::<Result<Vec<_>, _>>()?;
    let leftovers: Vec<_> = segment_entries
        .into_iter()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".building-"))
        .map(|e| e.path())
        .collect();
    let output_entries = std::fs::read_dir(&output)?.collect::<Result<Vec<_>, _>>()?;
    let run_leftovers: Vec<_> = output_entries
        .into_iter()
        .filter(|e| e.file_name().to_string_lossy().contains(".run-"))
        .map(|e| e.path())
        .collect();
    #[cfg(feature = "phase-probe")]
    let phase_count = if observed { phase_records.len() } else { 0 };
    #[cfg(not(feature = "phase-probe"))]
    let phase_count = 0usize;
    #[cfg(feature = "phase-probe")]
    if observed {
        if phase_count > 16384 {
            return Err("16384-record attribution evidence envelope exceeded".into());
        }
        use std::io::Write;
        let mut trace =
            std::io::BufWriter::new(std::fs::File::create(dir.join("phase-events.jsonl"))?);
        for record in &phase_records {
            let value = serde_json::json!({"name":record.name,"thread":format!("{:?}",record.thread),
                "depth":record.depth,"start_ns":record.start_ns,"wall_ns":record.wall_ns,
                "before":record.before,"after":record.after});
            serde_json::to_writer(&mut trace, &value)?;
            trace.write_all(b"\n")?;
        }
        trace.flush()?;
    }
    println!(
        "{}",
        serde_json::json!({"shape":shape,"target_mib":mib,"seed":seed,
        "builder":builder,"groups":groups,"encoded_group_bytes":encoded_bytes,"journal_bytes":journal_bytes,
        "phase_probe_observed":observed,"phase_records":phase_count,"observer_live_bytes":observer_live_bytes,
        "phase_sampling":"[CPU unsupported=0, live bytes, global peak bytes, requested unsupported=0]",
        "input_sha256":input_sha256,"build_seconds_instrumented":build_seconds,
        "heap_start_bytes":heap_start,"peak_live_heap_bytes":peak,
        "incremental_peak_heap_bytes":peak.saturating_sub(heap_start),"heap_after_build_bytes":heap_after,
        "manifest":manifest,"ordered_row_ledgers":ledgers,"building_leftovers":leftovers,
        "run_leftovers":run_leftovers,"io_before":io_before,"io_after":io_after,
        "stat_before":stat_before,"stat_after":stat_after})
    );
    Ok(())
}
fn ledger(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}
