//! Isolated Segment-build pilot; not an ingest or durability qualification.
//! seal_output_probe <new-state-dir> <repeat|entropy> <MiB> <seed>
//! Inputs remain resident before timing. All timings include allocation accounting.
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
    if args.len() != 4 {
        return Err("expected new-state-dir shape MiB seed".into());
    }
    let dir = PathBuf::from(&args[0]);
    let entropy = match args[1].as_str() {
        "repeat" => false,
        "entropy" => true,
        _ => return Err("unknown shape".into()),
    };
    let mib: usize = args[2].parse()?;
    if ![16, 64].contains(&mib) {
        return Err("pilot supports 16 or 64 MiB only".into());
    }
    let seed: u64 = args[3].parse()?;
    if seed == 0 {
        return Err("seed must be nonzero".into());
    }
    if dir.exists() {
        return Err("state directory must not exist".into());
    }
    let mut rng = seed;
    let mut groups = Vec::new();
    let mut encoded_bytes = 0;
    let mut input_hash = Sha256::new();
    while encoded_bytes < mib * 1024 * 1024 {
        let seq = groups.len() as u64 + 1;
        let node = seq % 16;
        let logs = (0..64)
            .map(|row| {
                let body: String = (0..1024)
                    .map(|_| {
                        if entropy {
                            (b'!' + (next(&mut rng) % 90) as u8) as char
                        } else {
                            'R'
                        }
                    })
                    .collect();
                LogRecord {
                    observed_time_unix_nano: START + ((seq * 17 + row * 11) % 100_000),
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
                            data_points: vec![NumberDataPoint {
                                time_unix_nano: START + seq,
                                value: Some(number_data_point::Value::AsInt(seq as i64)),
                                ..Default::default()
                            }],
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
                        start_time_unix_nano: START + seq,
                        end_time_unix_nano: START + seq + 1,
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
            sequence: seq,
            logs,
            metrics,
            traces,
            collection_gaps: vec!["fixture gap".into()],
            ..Default::default()
        };
        batch.validate()?;
        let group = Group {
            group_sequence: seq,
            entries: vec![Entry {
                label: format!("node-{node}"),
                batch: batch.encode_to_vec(),
                received_unix_nano: START + seq,
            }],
        };
        let encoded = group.encode_to_vec();
        input_hash.update((encoded.len() as u64).to_le_bytes());
        input_hash.update(&encoded);
        encoded_bytes += encoded.len();
        groups.push(group);
    }
    std::fs::create_dir(&dir)?;
    let input_sha256 = format!("{:x}", input_hash.finalize());
    let heap_start = LIVE.load(Relaxed);
    PEAK.store(heap_start, Relaxed);
    let started = Instant::now();
    let manifest = segment::build(&dir, 1, &groups)?;
    let build_seconds = started.elapsed().as_secs_f64();
    let peak = PEAK.load(Relaxed);
    let heap_after = LIVE.load(Relaxed);
    // Save build-only counters before verification allocates or reads output.
    segment::verify(
        &segment::segments_dir(&dir)?.join(segment::segment_name(1)),
        &manifest,
    )?;
    println!(
        "{}",
        serde_json::json!({"shape":args[1],"target_mib":mib,"seed":seed,"groups":groups.len(),"encoded_group_bytes":encoded_bytes,"input_sha256":input_sha256,"build_seconds_instrumented":build_seconds,"heap_start_bytes":heap_start,"peak_live_heap_bytes":peak,"incremental_peak_heap_bytes":peak.saturating_sub(heap_start),"heap_after_build_bytes":heap_after,"manifest":manifest})
    );
    Ok(())
}
