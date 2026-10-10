//! Snapshot-range evidence ablation: candidate calls the promoted production helper.
#[cfg(feature = "responsibility-alloc-probe")]
mod allocation {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    struct Counted;
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);
    static TOTAL: AtomicUsize = AtomicUsize::new(0);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn add(n: usize) {
        let live = LIVE.fetch_add(n, Relaxed) + n;
        PEAK.fetch_max(live, Relaxed);
        TOTAL.fetch_add(n, Relaxed);
        CALLS.fetch_add(1, Relaxed);
    }
    // Delegate pointer/layout contracts to System; observation allocates nothing.
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
                    let live = LIVE.fetch_add(size - layout.size(), Relaxed) + size - layout.size();
                    PEAK.fetch_max(live, Relaxed);
                } else {
                    LIVE.fetch_sub(layout.size() - size, Relaxed);
                }
                TOTAL.fetch_add(size, Relaxed);
                CALLS.fetch_add(1, Relaxed);
            }
            q
        }
    }
    #[global_allocator]
    static ALLOCATOR: Counted = Counted;
    pub fn facts() -> [usize; 4] {
        [
            LIVE.load(Relaxed),
            PEAK.load(Relaxed),
            TOTAL.load(Relaxed),
            CALLS.load(Relaxed),
        ]
    }
    pub fn reset() -> [usize; 4] {
        PEAK.store(LIVE.load(Relaxed), Relaxed);
        TOTAL.store(0, Relaxed);
        CALLS.store(0, Relaxed);
        facts()
    }
}
#[cfg(not(feature = "responsibility-alloc-probe"))]
mod allocation {
    pub fn facts() -> [usize; 4] {
        [0; 4]
    }
    pub fn reset() -> [usize; 4] {
        [0; 4]
    }
}

use fabric_frame::envelope::Batch;
use fabric_server::{
    rows::{Rows, extract},
    segment::{self, Manifest},
    store::{Entry, Group},
};
use opentelemetry_proto::tonic::{
    collector::{
        logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
        trace::v1::ExportTraceServiceRequest,
    },
    common::v1::{AnyValue, KeyValue, any_value},
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
    metrics::v1::{
        Gauge, Histogram, HistogramDataPoint, Metric, NumberDataPoint, ResourceMetrics,
        ScopeMetrics, Sum, metric, number_data_point,
    },
    trace::v1::{ResourceSpans, ScopeSpans, Span},
};
use prost::Message;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    hint::black_box,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
const SEED: u64 = 42;
const CAP: u64 = 2 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Evidence {
    received: [u64; 2],
    freshness: BTreeMap<String, u64>,
}
impl Evidence {
    fn empty() -> Self {
        Self {
            received: [u64::MAX, 0],
            freshness: BTreeMap::new(),
        }
    }
    fn add(&mut self, entry: &Entry, newest: Option<u64>) {
        self.received[0] = self.received[0].min(entry.received_unix_nano);
        self.received[1] = self.received[1].max(entry.received_unix_nano);
        if let Some(newest) = newest {
            let old = self.freshness.entry(entry.label.clone()).or_default();
            *old = (*old).max(newest);
        }
    }
}
fn invalid(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}
fn baseline(group: u64, entry: &Entry) -> io::Result<Option<u64>> {
    let mut rows = Rows::default();
    extract(group, entry, &mut rows)?;
    Ok(rows
        .logs
        .iter()
        .map(|r| r.observed_ns)
        .chain(rows.metrics.iter().map(|r| r.time_ns))
        .chain(rows.spans.iter().map(|r| r.start_ns))
        .max())
}
// Candidate is the same production helper used by boundary evidence.
fn latest_observation_ns(_group: u64, entry: &Entry) -> io::Result<Option<u64>> {
    fabric_server::rows::latest_observation_ns(entry)
}

type Project = fn(u64, &Entry) -> io::Result<Option<u64>>;
fn fold(groups: &[Group], range: [u64; 2], project: Project) -> io::Result<Evidence> {
    let mut evidence = Evidence::empty();
    for group in groups {
        if group.group_sequence < range[0] || group.group_sequence > range[1] {
            continue;
        }
        for entry in &group.entries {
            evidence.add(entry, project(group.group_sequence, entry)?);
        }
    }
    Ok(evidence)
}
fn end_scan(
    dir: &Path,
    manifest: &Manifest,
    range: [u64; 2],
    project: Project,
) -> io::Result<Evidence> {
    let mut evidence = Evidence::empty();
    let mut error = None;
    segment::scan_batches(dir, manifest, |group, entry| {
        if error.is_some() || group < range[0] || group > range[1] {
            return;
        }
        match project(group, &entry) {
            Ok(newest) => evidence.add(&entry, newest),
            Err(e) => error = Some(e),
        }
    })?;
    if let Some(error) = error {
        return Err(error);
    }
    Ok(evidence)
}
fn attr(index: usize) -> KeyValue {
    KeyValue {
        key: format!("attribute-{index}"),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(format!("value-{index}-λ"))),
        }),
        ..Default::default()
    }
}
fn body(n: usize) -> String {
    let mut state = SEED;
    (0..n)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (b'a' + (state % 26) as u8) as char
        })
        .collect()
}
fn fixture(n: usize, body_bytes: usize, nodes: usize) -> Vec<Group> {
    let body = body(body_bytes);
    (0..n)
        .map(|index| {
            let t = 2000 + index as u64 * 100;
            let attrs = (0..8).map(attr).collect::<Vec<_>>();
            let logs = ExportLogsServiceRequest {
                resource_logs: vec![ResourceLogs {
                    scope_logs: vec![ScopeLogs {
                        log_records: (0..4)
                            .map(|j| LogRecord {
                                observed_time_unix_nano: t + j,
                                body: Some(AnyValue {
                                    value: Some(any_value::Value::StringValue(body.clone())),
                                }),
                                attributes: attrs.clone(),
                                ..Default::default()
                            })
                            .collect(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
            }
            .encode_to_vec();
            let points = |offset| {
                (0..2)
                    .map(|j| NumberDataPoint {
                        time_unix_nano: t + offset + j,
                        start_time_unix_nano: 1,
                        value: Some(number_data_point::Value::AsInt(j as i64)),
                        attributes: attrs.clone(),
                        ..Default::default()
                    })
                    .collect()
            };
            let metrics = ExportMetricsServiceRequest {
                resource_metrics: vec![ResourceMetrics {
                    scope_metrics: vec![ScopeMetrics {
                        metrics: vec![
                            Metric {
                                name: "gauge".into(),
                                data: Some(metric::Data::Gauge(Gauge {
                                    data_points: points(10),
                                })),
                                ..Default::default()
                            },
                            Metric {
                                name: "counter".into(),
                                data: Some(metric::Data::Sum(Sum {
                                    data_points: points(20),
                                    aggregation_temporality: 2,
                                    is_monotonic: true,
                                })),
                                ..Default::default()
                            },
                            Metric {
                                name: "ignored-histogram".into(),
                                data: Some(metric::Data::Histogram(Histogram {
                                    data_points: vec![HistogramDataPoint {
                                        time_unix_nano: 999999,
                                        ..Default::default()
                                    }],
                                    ..Default::default()
                                })),
                                ..Default::default()
                            },
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
            }
            .encode_to_vec();
            let traces = ExportTraceServiceRequest {
                resource_spans: vec![ResourceSpans {
                    scope_spans: vec![ScopeSpans {
                        spans: (0..2)
                            .map(|j| Span {
                                trace_id: vec![7; 16],
                                span_id: vec![8; 8],
                                parent_span_id: vec![9; 8],
                                name: "known-span".into(),
                                start_time_unix_nano: t + 30 + j,
                                end_time_unix_nano: t + 90 + j,
                                attributes: attrs.clone(),
                                ..Default::default()
                            })
                            .collect(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
            }
            .encode_to_vec();
            let entry = Entry {
                label: format!("node-{}", index % nodes),
                received_unix_nano: 1000 + index as u64 * 100,
                batch: Batch {
                    version: 1,
                    node_id: vec![(index % nodes + 1) as u8; 16],
                    generation: 1,
                    sequence: (index / nodes + 1) as u64,
                    logs,
                    metrics,
                    traces,
                    collection_gaps: vec!["known-gap".into()],
                    ..Default::default()
                }
                .encode_to_vec(),
            };
            Group {
                group_sequence: index as u64 + 1,
                entries: vec![entry],
            }
        })
        .collect()
}
fn expected(range: [u64; 2], nodes: usize) -> Evidence {
    let mut out = Evidence::empty();
    if range[0] > range[1] {
        return out;
    }
    out.received = [1000 + (range[0] - 1) * 100, 1000 + (range[1] - 1) * 100];
    for group in range[0]..=range[1] {
        let index = group - 1;
        out.freshness.insert(
            format!("node-{}", index as usize % nodes),
            2000 + index * 100 + 31,
        );
    }
    out
}
fn error_signature(result: io::Result<Option<u64>>) -> Value {
    match result {
        Ok(value) => json!({"ok":value}),
        Err(e) => json!({"kind":format!("{:?}",e.kind()),"error":e.to_string()}),
    }
}
fn direct_controls(groups: &[Group], nodes: usize) -> Vec<Value> {
    let original = &groups[0].entries[0];
    let mut controls = Vec::new();
    for field in ["envelope", "node", "logs", "metrics", "traces"] {
        let mut entry = original.clone();
        if field == "envelope" {
            entry.batch = vec![0xff];
        } else {
            let mut batch = Batch::decode(entry.batch.as_slice()).unwrap();
            match field {
                "node" => batch.node_id = vec![7; 15],
                "logs" => batch.logs = vec![0xff],
                "metrics" => batch.metrics = vec![0xff],
                "traces" => batch.traces = vec![0xff],
                _ => unreachable!(),
            }
            entry.batch = batch.encode_to_vec();
        }
        let a = error_signature(baseline(1, &entry));
        let b = error_signature(latest_observation_ns(1, &entry));
        let passed = a == b && a.get("error").is_some();
        controls.push(
            json!({"name":format!("malformed-{field}"),"passed":passed,"baseline":a,"candidate":b}),
        );
    }
    let mut error_order = Vec::new();
    for clear_logs in [false, true] {
        let mut entry = original.clone();
        let mut batch = Batch::decode(entry.batch.as_slice()).unwrap();
        batch.logs = if clear_logs { Vec::new() } else { vec![0xff] };
        batch.metrics = vec![0xff];
        batch.traces = vec![0xff];
        entry.batch = batch.encode_to_vec();
        let expected = if clear_logs {
            ExportMetricsServiceRequest::decode(&[0xff][..])
                .unwrap_err()
                .to_string()
        } else {
            ExportLogsServiceRequest::decode(&[0xff][..])
                .unwrap_err()
                .to_string()
        };
        let truth = json!({"kind":"InvalidData","error":expected});
        let a = error_signature(baseline(1, &entry));
        let b = error_signature(latest_observation_ns(1, &entry));
        error_order.push(json!({"logs_removed":clear_logs,"expected":truth,"baseline":a,"candidate":b,"passed":a==truth&&b==truth}));
    }
    controls.push(json!({"name":"decode-error-order","passed":error_order.iter().all(|v|v["passed"]==true),"cases":error_order}));
    let mut empty = original.clone();
    let mut batch = Batch::decode(empty.batch.as_slice()).unwrap();
    batch.logs.clear();
    batch.metrics.clear();
    batch.traces.clear();
    batch.collection_gaps.clear();
    empty.batch = batch.encode_to_vec();
    for (name, entry, truth) in [
        ("empty-signals", empty.clone(), None),
        (
            "gap-only",
            {
                let mut e = empty.clone();
                let mut b = Batch::decode(e.batch.as_slice()).unwrap();
                b.collection_gaps = vec!["gap".into()];
                e.batch = b.encode_to_vec();
                e
            },
            None,
        ),
        (
            "zero-observation",
            {
                let mut e = empty.clone();
                let mut b = Batch::decode(e.batch.as_slice()).unwrap();
                b.logs = ExportLogsServiceRequest {
                    resource_logs: vec![ResourceLogs {
                        scope_logs: vec![ScopeLogs {
                            log_records: vec![LogRecord::default()],
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                }
                .encode_to_vec();
                e.batch = b.encode_to_vec();
                e
            },
            Some(0),
        ),
        (
            "unsupported-histogram",
            {
                let mut e = empty;
                let mut b = Batch::decode(e.batch.as_slice()).unwrap();
                let mut m = ExportMetricsServiceRequest::decode(
                    Batch::decode(original.batch.as_slice())
                        .unwrap()
                        .metrics
                        .as_slice(),
                )
                .unwrap();
                m.resource_metrics[0].scope_metrics[0].metrics.drain(..2);
                b.metrics = m.encode_to_vec();
                e.batch = b.encode_to_vec();
                e
            },
            None,
        ),
    ] {
        let a = baseline(1, &entry).unwrap();
        let b = latest_observation_ns(1, &entry).unwrap();
        controls.push(json!({"name":name,"passed":a==truth&&b==truth,"expected":truth,"baseline":a,"candidate":b}));
    }
    let mut excluded = groups.to_vec();
    let mut batch = Batch::decode(excluded.last().unwrap().entries[0].batch.as_slice()).unwrap();
    batch.traces = vec![0xff];
    excluded.last_mut().unwrap().entries[0].batch = batch.encode_to_vec();
    controls.push(json!({"name":"excluded-malformed-otlp","passed":fold(&excluded,[1,groups.len() as u64-1],baseline).unwrap()==expected([1,groups.len() as u64-1],nodes)&&fold(&excluded,[1,groups.len() as u64-1],latest_observation_ns).unwrap()==expected([1,groups.len() as u64-1],nodes)}));
    controls.push(json!({"name":"zero-inclusion","passed":fold(groups,[1,0],baseline).unwrap()==Evidence::empty()&&fold(groups,[1,0],latest_observation_ns).unwrap()==Evidence::empty()}));
    controls
}

mod process_clock {
    use std::ffi::{c_int, c_long};
    #[repr(C)]
    struct Timespec {
        seconds: c_long,
        nanoseconds: c_long,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: c_int, out: *mut Timespec) -> c_int;
        fn clock_getres(clock: c_int, out: *mut Timespec) -> c_int;
    }
    const PROCESS_CPU: c_int = 2;
    fn read(resolution: bool) -> u64 {
        let mut ts = Timespec {
            seconds: 0,
            nanoseconds: 0,
        };
        // Linux CLOCK_PROCESS_CPUTIME_ID and writable initialized Timespec.
        let rc = unsafe {
            if resolution {
                clock_getres(PROCESS_CPU, &mut ts)
            } else {
                clock_gettime(PROCESS_CPU, &mut ts)
            }
        };
        assert_eq!(rc, 0, "Linux process CPU clock unavailable");
        assert!(ts.seconds >= 0 && (0..1_000_000_000).contains(&ts.nanoseconds));
        ts.seconds as u64 * 1_000_000_000 + ts.nanoseconds as u64
    }
    pub fn now() -> u64 {
        read(false)
    }
    pub fn resolution() -> u64 {
        read(true)
    }
}
fn file_bytes(root: &Path) -> u64 {
    fs::read_dir(root)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            assert!(!e.file_type().unwrap().is_symlink());
            if e.file_type().unwrap().is_dir() {
                file_bytes(&e.path())
            } else {
                e.metadata().unwrap().len()
            }
        })
        .sum()
}
struct Scratch {
    root: PathBuf,
    success: bool,
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.success || std::thread::panicking() {
            eprintln!("range evidence failure preserved {}", self.root.display());
        }
    }
}
fn digest_manifest(root: &Path, groups: &[Group], corrupt_last: bool) -> io::Result<Manifest> {
    use arrow_array::{
        BinaryArray, Int64Array, RecordBatch, StringArray, UInt64Array,
        builder::FixedSizeBinaryBuilder,
    };
    use parquet::{
        arrow::ArrowWriter,
        basic::{Compression, ZstdLevel},
        file::properties::WriterProperties,
    };
    let mut digest = FixedSizeBinaryBuilder::new(32);
    for (i, g) in groups.iter().enumerate() {
        let hash = Sha256::digest(&g.entries[0].batch);
        digest
            .append_value(if corrupt_last && i + 1 == groups.len() {
                &[0u8; 32]
            } else {
                hash.as_slice()
            })
            .map_err(invalid)?;
    }
    let batch = RecordBatch::try_new(
        segment::batches_schema(),
        vec![
            Arc::new(UInt64Array::from_iter_values(
                groups.iter().map(|g| g.group_sequence),
            )),
            Arc::new(StringArray::from_iter_values(
                groups.iter().map(|g| g.entries[0].label.as_str()),
            )),
            Arc::new(Int64Array::from_iter_values(
                groups
                    .iter()
                    .map(|g| g.entries[0].received_unix_nano as i64),
            )),
            Arc::new(digest.finish()),
            Arc::new(BinaryArray::from_iter_values(
                groups.iter().map(|g| g.entries[0].batch.as_slice()),
            )),
        ],
    )
    .map_err(invalid)?;
    let path = root.join("batches.parquet");
    let properties = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(3).map_err(invalid)?))
        .build();
    let mut writer = ArrowWriter::try_new(
        File::create(&path)?,
        segment::batches_schema(),
        Some(properties),
    )
    .map_err(invalid)?;
    writer.write(&batch).map_err(invalid)?;
    writer.close().map_err(invalid)?;
    Ok(Manifest {
        version: 1,
        journal_label: 1,
        first_group: 1,
        last_group: groups.len() as u64,
        records: groups.len() as u64,
        received_min_ns: 1000,
        received_max_ns: 1000 + (groups.len() as u64 - 1) * 100,
        freshness: BTreeMap::new(),
        files: BTreeMap::from([(
            "batches.parquet".into(),
            segment::FileEntry {
                bytes: path.metadata()?.len(),
                rows: groups.len() as u64,
                sha256: format!("{:x}", Sha256::digest(fs::read(&path)?)),
            },
        )]),
    })
}
fn measure(
    mut run: impl FnMut() -> io::Result<Evidence>,
    repeats: usize,
    truth: &Evidence,
) -> Value {
    let before = allocation::reset();
    let cpu_start = process_clock::now();
    let start = Instant::now();
    let mut actual = None;
    for index in 0..repeats {
        let result = run().unwrap();
        if index + 1 == repeats {
            actual = Some(black_box(result));
        } else {
            drop(black_box(result));
        }
    }
    let wall_ns = start.elapsed().as_nanos();
    let cpu_ns = process_clock::now() - cpu_start;
    let after = allocation::facts();
    let actual = actual.unwrap();
    let matched = &actual == truth;
    let value = json!({"wall_ns":wall_ns,"cpu_ns":cpu_ns,"allocator_counted":cfg!(feature="responsibility-alloc-probe"),"requested_bytes":after[2],"allocation_calls":after[3],"peak_extra_requested_bytes":after[1].saturating_sub(before[0]),"expected_match":matched,"actual":actual});
    if !matched {
        println!(
            "{}",
            json!({"event":"measurement-error","result":value,"expected":truth})
        );
        panic!("measured output differs from arithmetic expected");
    }
    value
}
fn main() {
    let repeats = 16;
    assert!(
        std::env::args().len() == 2,
        "usage: range_evidence_probe SCRATCH_ROOT"
    );
    let base =
        PathBuf::from(std::env::var("FABRIC_SCRATCH_ROOT").expect("contained mounted scratch"));
    let root = PathBuf::from(std::env::args().nth(1).unwrap());
    assert!(
        root.is_absolute()
            && root.starts_with(&base)
            && root != base
            && !root
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir)),
        "owned child scratch must stay under FABRIC_SCRATCH_ROOT"
    );
    fs::create_dir(&root).unwrap();
    let mut scratch = Scratch {
        root,
        success: false,
    };
    println!(
        "{}",
        json!({"event":"config","seed":SEED,"repeats":repeats,"cells":["A","B","C","D"],"allocator_counted":cfg!(feature="responsibility-alloc-probe"),"cpu_clock":"CLOCK_PROCESS_CPUTIME_ID","cpu_clock_resolution_ns":process_clock::resolution(),"raw_scan":"identical public scan_batches default Arrow batch size; tiny bounded fixture, not one-row production memory comparison","scratch":scratch.root})
    );
    let mut trials = 0;
    let mut controls_total = 0;
    for (cell, n, body_bytes, range, nodes) in [
        ("A", 16, 128, [1, 8], 1),
        ("B", 16, 4096, [1, 8], 1),
        ("C", 64, 4096, [1, 32], 1),
        ("D", 64, 4096, [9, 64], 8),
    ] {
        let groups = fixture(n, body_bytes, nodes);
        let expected = expected(range, nodes);
        let raw_batch_bytes: usize = groups
            .iter()
            .flat_map(|g| &g.entries)
            .map(|e| e.batch.len())
            .sum();
        let included_batch_bytes: usize = groups
            .iter()
            .filter(|g| g.group_sequence >= range[0] && g.group_sequence <= range[1])
            .flat_map(|g| &g.entries)
            .map(|e| e.batch.len())
            .sum();
        let included = range[1] - range[0] + 1;
        let mut sha = Sha256::new();
        let mut encoded_bytes = 0;
        for group in &groups {
            let bytes = group.encode_to_vec();
            encoded_bytes += bytes.len();
            sha.update(bytes);
        }
        let fixture_sha256 = format!("{:x}", sha.finalize());
        let state = scratch.root.join(cell);
        fs::create_dir(&state).unwrap();
        let manifest = segment::build(&state, 1, &groups).unwrap();
        let dir = segment::segments_dir(&state)
            .unwrap()
            .join(segment::segment_name(1));
        fs::write(state.join("fixture.json"),json!({"seed":SEED,"n":n,"body_bytes":body_bytes,"nodes":nodes,"range":range,"fixture_sha256":fixture_sha256,"expected":expected}).to_string()).unwrap();
        assert!(
            file_bytes(&scratch.root) <= CAP,
            "all generated Segment files exceed2MiB; preserve failure"
        );
        let mut controls = direct_controls(&groups, nodes);
        controls.push(json!({"name":"middle-interval","passed":fold(&groups,[2,3],baseline).unwrap()==self::expected([2,3],nodes)&&fold(&groups,[2,3],latest_observation_ns).unwrap()==self::expected([2,3],nodes)}));
        assert_eq!(fold(&groups, range, baseline).unwrap(), expected);
        assert_eq!(
            fold(&groups, range, latest_observation_ns).unwrap(),
            expected
        );
        for (name, project) in [
            ("baseline", baseline as Project),
            ("candidate", latest_observation_ns as Project),
        ] {
            assert_eq!(
                end_scan(&dir, &manifest, range, project).unwrap(),
                expected,
                "initial raw {name}"
            );
        }
        for (name, control_range) in [("zero-inclusion", [1, 0]), ("middle-interval", [2, 3])] {
            let truth = self::expected(control_range, nodes);
            let raw_passed = end_scan(&dir, &manifest, control_range, baseline).unwrap() == truth
                && end_scan(&dir, &manifest, control_range, latest_observation_ns).unwrap()
                    == truth;
            let control = controls.iter_mut().find(|v| v["name"] == name).unwrap();
            let passed = control["passed"] == true && raw_passed;
            control["passed"] = json!(passed);
            control["raw_scan_passed"] = json!(raw_passed);
        }
        let excluded_range = [1, n as u64 - 1];
        let mut malformed_groups = groups.clone();
        let last = &mut malformed_groups.last_mut().unwrap().entries[0];
        let mut last_batch = Batch::decode(last.batch.as_slice()).unwrap();
        last_batch.traces = vec![0xff];
        last.batch = last_batch.encode_to_vec();
        let malformed_root = state.join("excluded-malformed-correct-digest");
        fs::create_dir(&malformed_root).unwrap();
        let malformed_manifest =
            digest_manifest(&malformed_root, &malformed_groups, false).unwrap();
        let malformed_truth = self::expected(excluded_range, nodes);
        let raw_passed = end_scan(
            &malformed_root,
            &malformed_manifest,
            excluded_range,
            baseline,
        )
        .unwrap()
            == malformed_truth
            && end_scan(
                &malformed_root,
                &malformed_manifest,
                excluded_range,
                latest_observation_ns,
            )
            .unwrap()
                == malformed_truth;
        let control = controls
            .iter_mut()
            .find(|v| v["name"] == "excluded-malformed-otlp")
            .unwrap();
        let passed = control["passed"] == true && raw_passed;
        control["passed"] = json!(passed);
        control["raw_scan_passed"] = json!(raw_passed);
        assert!(
            file_bytes(&scratch.root) <= CAP,
            "generated raw controls exceed2MiB"
        );
        for phase in ["decode_fold", "end_scan"] {
            for pair in 1..=3 {
                let order = if pair == 2 {
                    ["metadata", "rows"]
                } else {
                    ["rows", "metadata"]
                };
                for (position, arm) in order.into_iter().enumerate() {
                    let project = if arm == "rows" {
                        baseline as Project
                    } else {
                        latest_observation_ns as Project
                    };
                    let mut result = if phase == "decode_fold" {
                        measure(|| fold(&groups, range, project), repeats, &expected)
                    } else {
                        measure(
                            || end_scan(&dir, &manifest, range, project),
                            repeats,
                            &expected,
                        )
                    };
                    let fields = result.as_object_mut().unwrap();
                    fields.insert("event".into(), json!("trial"));
                    fields.insert("cell".into(), json!(cell));
                    fields.insert("phase".into(), json!(phase));
                    fields.insert("pair".into(), json!(pair));
                    fields.insert("position".into(), json!(position + 1));
                    fields.insert("arm".into(), json!(arm));
                    fields.insert("repeats".into(), json!(repeats));
                    fields.insert(
                        "raw_bytes_hashed_logical".into(),
                        json!(if phase == "end_scan" {
                            raw_batch_bytes * repeats
                        } else {
                            0
                        }),
                    );
                    fields.insert("included_entries".into(), json!(included * repeats as u64));
                    fields.insert(
                        "included_observations".into(),
                        json!(included * 10 * repeats as u64),
                    );
                    println!("{result}");
                    trials += 1;
                }
            }
        }
        // Preserve original projection files in a sibling; raw-only metadata must
        // remain exact when projected tables are missing and then corrupt.
        let saved = state.join("saved-projections");
        fs::create_dir(&saved).unwrap();
        let mut names = Vec::new();
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_os_string();
            if name != "batches.parquet" && name != "manifest.json" {
                fs::rename(&path, saved.join(&name)).unwrap();
                names.push(name);
            }
        }
        let exact = |project| end_scan(&dir, &manifest, range, project).unwrap() == expected;
        controls.push(json!({"name":"missing-projections","passed":exact(baseline as Project)&&exact(latest_observation_ns as Project)}));
        for name in &names {
            fs::write(dir.join(name), b"corrupt").unwrap();
        }
        controls.push(json!({"name":"corrupt-projections","passed":exact(baseline as Project)&&exact(latest_observation_ns as Project)}));
        for name in &names {
            fs::remove_file(dir.join(name)).unwrap();
            fs::rename(saved.join(name), dir.join(name)).unwrap();
        }
        fs::remove_dir(saved).unwrap();
        let corrupt = state.join("excluded-digest-control");
        fs::create_dir(&corrupt).unwrap();
        let corrupt_manifest = digest_manifest(&corrupt, &groups, true).unwrap();
        let excluded_range = [1, n as u64 - 1];
        let a = end_scan(&corrupt, &corrupt_manifest, excluded_range, baseline).unwrap_err();
        let b = end_scan(
            &corrupt,
            &corrupt_manifest,
            excluded_range,
            latest_observation_ns,
        )
        .unwrap_err();
        let zero_a = end_scan(&corrupt, &corrupt_manifest, [1, 0], baseline).unwrap_err();
        let zero_b =
            end_scan(&corrupt, &corrupt_manifest, [1, 0], latest_observation_ns).unwrap_err();
        controls.push(json!({"name":"excluded-raw-digest","passed":a.kind()==io::ErrorKind::InvalidData&&a.to_string()=="segment record digest mismatch"&&a.kind()==b.kind()&&a.to_string()==b.to_string()&&zero_a.kind()==a.kind()&&zero_a.to_string()==a.to_string()&&zero_b.kind()==a.kind()&&zero_b.to_string()==a.to_string(),"zero_inclusion_rejects":true,"baseline_error":a.to_string(),"candidate_error":b.to_string()}));
        let controls_passed = controls.iter().all(|c| c["passed"] == true);
        controls_total += controls.len();
        let result = json!({"event":"cell","cell":cell,"n":n,"body_bytes":body_bytes,"included":range[1]-range[0]+1,"range":range,"nodes":nodes,"fixture_sha256":fixture_sha256,"fixture_encoded_bytes":encoded_bytes,"raw_batch_bytes":raw_batch_bytes,"included_batch_bytes":included_batch_bytes,"included_observations":included*10,"expected":expected,"controls":controls});
        fs::write(state.join("cell-result.json"), result.to_string()).unwrap();
        println!("{result}");
        assert!(
            controls_passed,
            "control failed; preserved result before assertion"
        );
        assert!(
            file_bytes(&scratch.root) <= CAP,
            "fixture archive filebytes exceed2MiB"
        );
    }
    assert_eq!(trials, 48);
    assert_eq!(controls_total, 64);
    println!(
        "{}",
        json!({"event":"complete","trials":trials,"controls":controls_total,"controls_all_passed":true,"fixture_file_bytes":file_bytes(&scratch.root),"scratch":scratch.root,"interpretation":"plain timings for comparison; counted timing diagnostic; raw SHA+typed protobuf decode unchanged; Parquet buffers and public batchsize unchanged"})
    );
    scratch.success = true;
}
