//! Converged telemetry daemon, one-shot collector, and `daemon.sock` RPC responder.

use crate::cgroups::walk_cgroups_and_processes;
use crate::channel::BoundedTelemetryQueue;
use crate::fs_watch::FsChurnTracker;
use crate::host::{
    read_gpu_stats, read_hostname, read_memory_snapshot, read_mount_usages, read_psi_metrics,
};
use crate::host_export::{HostExporter, configured_host_export_node, is_forwarded_host_signal};
use crate::self_guard::{SelfPreservationGuard, build_doctor_report_from_summary};
use crate::spindle_bridge::{
    IngestEnvelope, poll_spindle_projection_records, read_bounded_line,
    serve_ingest_socket_until_shutdown,
};
use crate::spindle_range::SpindleProjectionRange;
use crate::systemd::collect_unit_records;
use chrono::Utc;
use fabric_core::{
    BlameReport, BlameResource, DDSketch, DEFAULT_TMP_DIR, DriftContext, DriftReport,
    EwmaMadDetector, FsEventRecord, HostSnapshot, LogRecord, MetricPoint, PillarKind,
    SCHEMA_VERSION, SpindleProvenance, StorageLayout, TraceSpanRecord, evaluate_five_pillars,
};
use fabric_graph::{CausalGraph, Direction, LineageSubgraph};
use fabric_store::{
    CatalogStore, HotMetricRing, JanitorConfig, OlapParquetStore, RetentionJanitor,
    validate_non_tmp_path_on_disk,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader as TokioBufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;

const MAX_SPINDLE_STAGED_ITEMS: usize = 1024;
const MAX_SPINDLE_STAGED_BYTES: usize = 4 * 1024 * 1024;
const MAX_CONCURRENT_RPC_CONNS: usize = 12;
const RPC_LINE_LIMIT_BYTES: usize = 64 * 1024;
const RPC_IO_TIMEOUT: Duration = Duration::from_secs(5);

/// Configuration for running `fabric daemon`.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub data_dir: PathBuf,
    pub tmp_dir: PathBuf,
    pub interval_ms: u64,
    pub max_ticks: Option<u64>,
    pub ring_budget_bytes: usize,
    pub max_disk_bytes: u64,
    pub spindle_socket: Option<PathBuf>,
    pub runtime_dir: Option<PathBuf>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self::with_ring_budget(crate::resource_budget::automatic_ring_budget().budget_bytes)
    }
}

impl DaemonConfig {
    /// Initialize the remaining defaults using a precomputed ring budget.
    #[must_use]
    pub fn with_ring_budget(ring_budget_bytes: usize) -> Self {
        Self {
            data_dir: PathBuf::from(fabric_core::DEFAULT_DATA_DIR),
            tmp_dir: PathBuf::from(DEFAULT_TMP_DIR),
            interval_ms: 2000,
            max_ticks: None,
            ring_budget_bytes,
            max_disk_bytes: fabric_store::janitor::DEFAULT_MAX_DISK_BYTES,
            spindle_socket: default_spindle_socket(),
            runtime_dir: None,
        }
    }
}

/// Complete state assembled in one collection cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectedState {
    pub snapshot: HostSnapshot,
    pub drift: DriftReport,
    pub graph: CausalGraph,
    pub metrics: Vec<MetricPoint>,
    pub fs_events: Vec<FsEventRecord>,
}

/// Synchronous JSON RPC request handled by `daemon.sock`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DaemonRequest {
    Ping,
    Snapshot,
    Doctor,
    Blame {
        resource: String,
    },
    Lineage {
        target: String,
        direction: String,
        max_depth: usize,
    },
    Drift {
        pillar: Option<String>,
    },
    HotMetrics {
        since_ms: i64,
    },
    Flush,
}

/// JSON RPC response returned by `daemon.sock`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DaemonResponse {
    Pong { pid: u32, uptime_ms: u64 },
    Snapshot(Box<HostSnapshot>),
    Doctor(fabric_core::DoctorReport),
    Blame(BlameReport),
    Lineage(LineageSubgraph),
    Drift(DriftReport),
    HotMetrics { points: Vec<MetricPoint> },
    Flushed { rows: usize },
    Error { message: String },
}

/// Resolves validated non-`/tmp` runtime socket paths.
#[must_use]
pub fn default_daemon_socket(layout: &StorageLayout) -> PathBuf {
    runtime_socket_path(layout, "daemon.sock")
}

#[must_use]
pub fn default_ingest_socket(layout: &StorageLayout) -> PathBuf {
    runtime_socket_path(layout, "ingest.sock")
}

/// Resolves the default validated non-`/tmp` Spindle Unix socket path if available.
#[must_use]
pub fn default_spindle_socket() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("SPINDLE_SOCKET")
        && !raw.is_empty()
    {
        let path = PathBuf::from(raw);
        if let Ok(resolved) = validate_non_tmp_path_on_disk(&path) {
            return Some(resolved);
        }
    }
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(xdg).join("spindle").join("spindle.sock");
        if let Ok(resolved) = validate_non_tmp_path_on_disk(&path) {
            return Some(resolved);
        }
    }
    None
}

fn metric_approx_bytes(m: &MetricPoint) -> usize {
    std::mem::size_of::<MetricPoint>()
        + m.name.len()
        + m.host.len()
        + m.slice.len()
        + m.unit.len()
        + m.labels_json.len()
}

fn log_approx_bytes(l: &LogRecord) -> usize {
    std::mem::size_of::<LogRecord>()
        + l.level.len()
        + l.source.len()
        + l.unit.len()
        + l.message.len()
        + l.attributes_json.len()
}

fn trace_approx_bytes(t: &TraceSpanRecord) -> usize {
    std::mem::size_of::<TraceSpanRecord>()
        + t.trace_id.len()
        + t.span_id.len()
        + t.parent_span_id.as_ref().map_or(0, String::len)
        + t.name.len()
        + t.service.len()
        + t.status.len()
        + t.attributes_json.len()
}

fn envelope_approx_bytes(env: &IngestEnvelope) -> usize {
    match env {
        IngestEnvelope::Metric(m) => metric_approx_bytes(m),
        IngestEnvelope::Log(l) => log_approx_bytes(l),
        IngestEnvelope::Trace(t) => trace_approx_bytes(t),
    }
}

type PendingOlapPlan = (
    fabric_store::OlapFlushPlan,
    SpindleProjectionRange,
    usize,
    usize,
);
type SpindleRow<T> = (Option<SpindleProvenance>, T);

fn spindle_provenance_bytes(provenance: Option<&SpindleProvenance>) -> usize {
    std::mem::size_of::<Option<SpindleProvenance>>()
        + provenance.map_or(0, |key| {
            let allocated =
                key.origin.capacity() + key.producer.capacity() + key.generation.capacity();
            let text_bytes = key.origin.len() + key.producer.len() + key.generation.len();
            // Arrow conversion includes JSON field names, two u64s, and escaped text.
            allocated.max(128 + 6 * text_bytes)
        })
}

#[derive(Default)]
struct SpindleStaging {
    metrics: Vec<SpindleRow<MetricPoint>>,
    logs: Vec<SpindleRow<LogRecord>>,
    traces: Vec<SpindleRow<TraceSpanRecord>>,
    range: Option<SpindleProjectionRange>,
    pending_plan: Option<PendingOlapPlan>,
    pending_volatile_plan: Option<fabric_store::OlapFlushPlan>,
    inflight_items: usize,
    staged_bytes: usize,
    inflight_bytes: usize,
    deferred_range: Option<SpindleProjectionRange>,
    uncommitted_partitions: std::collections::BTreeSet<(&'static str, chrono::NaiveDate)>,
}

impl SpindleStaging {
    fn total_len(&self) -> usize {
        self.metrics.len() + self.logs.len() + self.traces.len()
    }

    fn pending_items(&self) -> usize {
        self.pending_plan.as_ref().map_or(0, |p| p.2)
    }

    fn pending_bytes(&self) -> usize {
        self.pending_plan.as_ref().map_or(0, |p| p.3)
    }

    fn pending_or_staged_items(&self) -> usize {
        self.total_len().saturating_add(self.pending_items())
    }

    #[cfg(test)]
    fn effective_watermark(&self) -> Option<u64> {
        self.range
            .as_ref()
            .or_else(|| self.pending_plan.as_ref().map(|p| &p.1))
            .map(SpindleProjectionRange::last_id)
    }

    fn recompute_bytes(&mut self) {
        self.staged_bytes = self
            .metrics
            .iter()
            .map(|(key, row)| spindle_provenance_bytes(key.as_ref()) + metric_approx_bytes(row))
            .sum::<usize>()
            + self
                .logs
                .iter()
                .map(|(key, row)| spindle_provenance_bytes(key.as_ref()) + log_approx_bytes(row))
                .sum::<usize>()
            + self
                .traces
                .iter()
                .map(|(key, row)| spindle_provenance_bytes(key.as_ref()) + trace_approx_bytes(row))
                .sum::<usize>();
    }

    fn take_pending_plan(&mut self) -> Option<PendingOlapPlan> {
        let pending = self.pending_plan.take()?;
        self.inflight_items = self.inflight_items.saturating_add(pending.2);
        self.inflight_bytes = self.inflight_bytes.saturating_add(pending.3);
        Some(pending)
    }

    fn take_for_flush(&mut self) -> Self {
        let taken = Self {
            metrics: std::mem::take(&mut self.metrics),
            logs: std::mem::take(&mut self.logs),
            traces: std::mem::take(&mut self.traces),
            range: self.range.take(),
            pending_plan: None,
            pending_volatile_plan: None,
            inflight_items: 0,
            staged_bytes: self.staged_bytes,
            inflight_bytes: 0,
            deferred_range: None,
            uncommitted_partitions: std::collections::BTreeSet::new(),
        };
        self.inflight_items = self.inflight_items.saturating_add(taken.total_len());
        self.inflight_bytes = self.inflight_bytes.saturating_add(self.staged_bytes);
        self.staged_bytes = 0;
        taken
    }

    fn restore_front(&mut self, mut older: SpindleStaging) -> Result<(), String> {
        SpindleProjectionRange::append(&mut older.range, self.range.take())?;
        if !older.metrics.is_empty() {
            older.metrics.append(&mut self.metrics);
            self.metrics = older.metrics;
        }
        if !older.logs.is_empty() {
            older.logs.append(&mut self.logs);
            self.logs = older.logs;
        }
        if !older.traces.is_empty() {
            older.traces.append(&mut self.traces);
            self.traces = older.traces;
        }
        self.range = older.range;
        self.recompute_bytes();
        Ok(())
    }
}

fn enqueue_spindle_record(
    key: SpindleProvenance,
    env: IngestEnvelope,
    runtime: &DaemonRuntime,
    cache_hot: bool,
) -> bool {
    let now_ms = Utc::now().timestamp_millis();
    let item_bytes = envelope_approx_bytes(&env) + spindle_provenance_bytes(Some(&key));
    let mut hot_metric: Option<MetricPoint> = None;
    {
        let mut staging = runtime.spindle_staging.lock();
        if staging
            .pending_or_staged_items()
            .saturating_add(staging.inflight_items)
            >= MAX_SPINDLE_STAGED_ITEMS
            || staging
                .staged_bytes
                .saturating_add(staging.pending_bytes())
                .saturating_add(staging.inflight_bytes)
                .saturating_add(item_bytes)
                > MAX_SPINDLE_STAGED_BYTES
        {
            return false;
        }
        if SpindleProjectionRange::include(&mut staging.range, &key).is_err() {
            return false;
        }
        let provenance = Some(key);
        match env {
            IngestEnvelope::Metric(mut pt) => {
                if pt.timestamp_ms <= 0 {
                    pt.timestamp_ms = now_ms;
                }
                hot_metric = cache_hot.then(|| pt.clone());
                staging.metrics.push((provenance, pt));
            }
            IngestEnvelope::Log(mut log) => {
                if log.timestamp_ms <= 0 {
                    log.timestamp_ms = now_ms;
                }
                staging.logs.push((provenance, log));
            }
            IngestEnvelope::Trace(mut span) => {
                if span.timestamp_ms <= 0 {
                    span.timestamp_ms = now_ms;
                }
                staging.traces.push((provenance, span));
            }
        }
        staging.staged_bytes = staging.staged_bytes.saturating_add(item_bytes);
    }
    if let Some(pt) = hot_metric {
        runtime
            .ring
            .write()
            .push_hot_only(std::slice::from_ref(&pt));
    }
    true
}

#[cfg(test)]
fn enqueue_test_spindle_record(id: u64, env: IngestEnvelope, runtime: &DaemonRuntime) -> bool {
    runtime
        .catalog
        .bind_spindle_projection("test-origin")
        .expect("bind authenticated test origin");
    enqueue_spindle_record(
        SpindleProvenance {
            origin: "test-origin".into(),
            record_id: id,
            producer: "test-producer".into(),
            generation: "test-generation".into(),
            sequence: id,
        },
        env,
        runtime,
        true,
    )
}

fn runtime_socket_path(layout: &StorageLayout, name: &str) -> PathBuf {
    if let Some(rt_env) = std::env::var_os("FABRIC_RUNTIME_DIR")
        && !rt_env.is_empty()
    {
        let dir = PathBuf::from(rt_env);
        if let Ok(resolved) = validate_non_tmp_path_on_disk(&dir) {
            return resolved.join(name);
        }
    }
    if layout.data_dir() == Path::new(fabric_core::DEFAULT_DATA_DIR)
        && let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR")
    {
        let dir = PathBuf::from(xdg).join("fabrico11y");
        if let Ok(resolved) = validate_non_tmp_path_on_disk(&dir) {
            return resolved.join(name);
        }
    }
    layout.state_dir().join(name)
}

/// Queries a specific `daemon.sock` path if it exists and is listening.
#[must_use]
pub fn query_daemon_socket(sock_path: &Path, req: &DaemonRequest) -> Option<DaemonResponse> {
    if !sock_path.exists() {
        return None;
    }
    let mut stream = StdUnixStream::connect(sock_path).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(750)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(750)));
    let mut line = serde_json::to_vec(req).ok()?;
    line.push(b'\n');
    stream.write_all(&line).ok()?;
    stream.flush().ok()?;
    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    reader.read_line(&mut reply).ok()?;
    serde_json::from_str::<DaemonResponse>(reply.trim()).ok()
}

/// Queries the resident `fabric daemon` if `daemon.sock` is listening.
#[must_use]
pub fn query_running_daemon(layout: &StorageLayout, req: &DaemonRequest) -> Option<DaemonResponse> {
    query_daemon_socket(&default_daemon_socket(layout), req)
}

/// Read one resident Doctor response with a bounded size and total I/O deadline.
#[must_use]
pub async fn query_running_doctor(layout: &StorageLayout) -> Option<fabric_core::DoctorReport> {
    let socket = default_daemon_socket(layout);
    tokio::time::timeout(Duration::from_millis(750), async {
        let mut stream = UnixStream::connect(socket).await.ok()?;
        let mut request = serde_json::to_vec(&DaemonRequest::Doctor).ok()?;
        request.push(b'\n');
        stream.write_all(&request).await.ok()?;
        let mut reader = TokioBufReader::new(stream);
        let line = read_bounded_line(&mut reader, RPC_LINE_LIMIT_BYTES)
            .await
            .ok()?
            .flatten()?;
        match serde_json::from_str::<DaemonResponse>(&line).ok()? {
            DaemonResponse::Doctor(report) => Some(report),
            _ => None,
        }
    })
    .await
    .ok()
    .flatten()
}

/// Performs a single deterministic collection cycle without starting the resident daemon.
pub fn collect_once(data_dir: &Path, tmp_dir: &Path) -> Result<CollectedState, String> {
    let data_dir = validate_non_tmp_path_on_disk(data_dir).map_err(|e| e.to_string())?;
    let tmp_dir = validate_non_tmp_path_on_disk(tmp_dir).map_err(|e| e.to_string())?;
    let dropped = Arc::new(AtomicU64::new(0));
    let guard = SelfPreservationGuard::new(
        data_dir.to_string_lossy().into_owned(),
        tmp_dir.to_string_lossy().into_owned(),
        dropped,
    );
    let mut tracker = FsChurnTracker::new();
    let mut vol_detector = EwmaMadDetector::default();
    let mut sketch = DDSketch::default();
    Ok(run_collect_cycle(
        &guard,
        &mut tracker,
        &mut vol_detector,
        &mut sketch,
    ))
}

fn run_collect_cycle(
    guard: &SelfPreservationGuard,
    fs_tracker: &mut FsChurnTracker,
    vol_detector: &mut EwmaMadDetector,
    sketch: &mut DDSketch,
) -> CollectedState {
    let start = std::time::Instant::now();
    let now_ms = Utc::now().timestamp_millis();
    let hostname = read_hostname();
    let psi = read_psi_metrics();
    let memory = read_memory_snapshot();
    let mounts = read_mount_usages();
    let gpus = read_gpu_stats();
    let mut walk = walk_cgroups_and_processes();
    let units = collect_unit_records(&walk.cgroups, &walk.processes);
    let (fs_churn, fs_events) = fs_tracker.sample(&mounts, &walk.processes);

    // Enrich agent write bytes using attributed fs_churn sizes.
    for agent in &mut walk.agents {
        let write_sum: u64 = fs_churn
            .iter()
            .filter(|f| {
                f.owner_pid.is_some_and(|pid| agent.pids.contains(&pid))
                    || (f.owner_unit.is_some() && f.owner_unit == agent.unit)
            })
            .map(|f| f.size_bytes)
            .sum();
        agent.write_bytes = write_sum;
    }

    let self_health = guard.inspect_summary();
    let snapshot = HostSnapshot {
        timestamp_unix_ms: now_ms,
        hostname: hostname.clone(),
        psi: psi.clone(),
        memory: memory.clone(),
        mounts: mounts.clone(),
        slices: walk.slices.clone(),
        cgroups: walk.cgroups.clone(),
        processes: walk.processes.clone(),
        units,
        fs_churn: fs_churn.clone(),
        gpus,
        agents: walk.agents.clone(),
        self_health,
    };

    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    sketch.insert(elapsed_ms.max(0.01));
    let total_delta_bytes: f64 = fs_churn
        .iter()
        .map(|f| f.recent_delta_bytes.max(0) as f64)
        .sum();
    let vol_score = vol_detector.observe(total_delta_bytes);

    let ctx = DriftContext {
        now_unix_ms: now_ms,
        last_collection_watermark_ms: now_ms,
        max_staleness_ms: 15_000,
        observed_schemas: vec![SCHEMA_VERSION.to_owned()],
        expected_schemas: vec![SCHEMA_VERSION.to_owned()],
        volume_delta_bytes: total_delta_bytes,
        volume_score: Some(vol_score),
        latency_sketch: Some(sketch.clone()),
    };
    let drift = evaluate_five_pillars(&snapshot, &ctx, None);
    let graph = CausalGraph::from_snapshot(&snapshot, &drift.findings);

    let mut metrics = build_host_metrics(now_ms, &hostname, &snapshot);
    metrics.push(MetricPoint {
        timestamp_ms: now_ms,
        name: "fabric.collect.duration_ms".into(),
        value: elapsed_ms,
        host: hostname,
        slice: "fabrico11y.slice".into(),
        unit: "fabric-daemon.service".into(),
        pid: Some(std::process::id()),
        labels_json: "{}".into(),
    });

    CollectedState {
        snapshot,
        drift,
        graph,
        metrics,
        fs_events,
    }
}

fn build_host_metrics(now_ms: i64, host: &str, snap: &HostSnapshot) -> Vec<MetricPoint> {
    let mut pts = Vec::with_capacity(16 + snap.slices.len() * 2);
    let base = |name: &str, value: f64, slice: &str, unit: &str, pid: Option<u32>| MetricPoint {
        timestamp_ms: now_ms,
        name: name.to_owned(),
        value,
        host: host.to_owned(),
        slice: slice.to_owned(),
        unit: unit.to_owned(),
        pid,
        labels_json: "{}".into(),
    };
    pts.push(base(
        "host.mem.available_bytes",
        snap.memory.mem_available_bytes as f64,
        "host",
        "kernel",
        None,
    ));
    pts.push(base(
        "host.swap.used_bytes",
        snap.memory.swap_used_bytes as f64,
        "host",
        "zram0",
        None,
    ));
    pts.push(base(
        "host.psi.memory_some_avg10",
        snap.psi.memory_some.avg10,
        "host",
        "psi",
        None,
    ));
    pts.push(base(
        "host.psi.io_some_avg10",
        snap.psi.io_some.avg10,
        "host",
        "psi",
        None,
    ));
    for m in &snap.mounts {
        pts.push(base(
            "host.mount.used_bytes",
            m.used_bytes as f64,
            "host",
            &m.mount_point,
            None,
        ));
    }
    for s in &snap.slices {
        pts.push(base(
            "cgroup.slice.memory_current_bytes",
            s.memory_current_bytes as f64,
            &s.slice_name,
            "slice",
            None,
        ));
        pts.push(base(
            "cgroup.slice.cpu_usage_usec",
            s.cpu_usage_usec as f64,
            &s.slice_name,
            "slice",
            None,
        ));
    }
    for p in snap.processes.iter().take(20) {
        pts.push(base(
            "proc.rss_bytes",
            p.rss_bytes as f64,
            &p.slice,
            p.unit.as_deref().unwrap_or(&p.comm),
            Some(p.pid),
        ));
    }
    pts
}

struct DaemonRuntime {
    start_instant: std::time::Instant,
    config: DaemonConfig,
    catalog: CatalogStore,
    olap: OlapParquetStore,
    janitor: RetentionJanitor,
    _writer_lock: fabric_store::WriterLockGuard,
    flush_lock: Mutex<()>,
    ring: RwLock<HotMetricRing>,
    latest: RwLock<Option<CollectedState>>,
    pending_logs: RwLock<Vec<LogRecord>>,
    pending_traces: RwLock<Vec<TraceSpanRecord>>,
    pending_fs_events: RwLock<Vec<FsEventRecord>>,
    polled_spindle_cursor: AtomicU64,
    spindle_staging: Mutex<SpindleStaging>,
    last_spindle_error: RwLock<Option<String>>,
    overflow_dropped: Arc<AtomicU64>,
    guard: SelfPreservationGuard,
}

fn restore_bounded_queue<T>(
    queue: &RwLock<Vec<T>>,
    old: Vec<T>,
    max_len: usize,
    overflow_dropped: &AtomicU64,
) {
    if old.is_empty() {
        return;
    }
    let mut q = queue.write();
    let mut combined = old;
    combined.extend(q.drain(..));
    if combined.len() > max_len {
        let excess = combined.len() - max_len;
        combined.drain(..excess);
        overflow_dropped.fetch_add(excess as u64, Ordering::Relaxed);
    }
    *q = combined;
}

impl DaemonRuntime {
    fn protected_partition_set(
        &self,
    ) -> std::collections::BTreeSet<(&'static str, chrono::NaiveDate)> {
        let staging = self.spindle_staging.lock();
        let mut set = staging.uncommitted_partitions.clone();
        if let Some((plan, _, _, _)) = staging.pending_plan.as_ref() {
            set.extend(plan.uncommitted_partitions().iter().copied());
        }
        if let Some(vplan) = staging.pending_volatile_plan.as_ref() {
            set.extend(vplan.uncommitted_partitions().iter().copied());
        }
        set
    }

    fn merge_persisted_uncommitted(&self) {
        let owned = self.protected_partition_set();
        let borrowed: std::collections::BTreeSet<(&str, chrono::NaiveDate)> = owned
            .iter()
            .map(|&(domain, day)| (domain as &str, day))
            .collect();
        let _ = fabric_store::merge_uncommitted_partitions(self.olap.layout(), &borrowed);
    }

    fn clear_persisted_uncommitted(&self, committed_wm: Option<u64>) {
        let owned = self.protected_partition_set();
        let borrowed: std::collections::BTreeSet<(&str, chrono::NaiveDate)> = owned
            .iter()
            .map(|&(domain, day)| (domain as &str, day))
            .collect();
        let _ =
            fabric_store::clear_committed_uncommitted(self.olap.layout(), committed_wm, &borrowed);
    }

    fn store_failed_plan(
        &self,
        plan: fabric_store::OlapFlushPlan,
        range: SpindleProjectionRange,
        items: usize,
        bytes: usize,
    ) {
        let mut staging = self.spindle_staging.lock();
        staging.inflight_items = staging.inflight_items.saturating_sub(items);
        staging.inflight_bytes = staging.inflight_bytes.saturating_sub(bytes);
        staging.pending_plan = Some((plan, range, items, bytes));
    }

    fn finish_inflight_and_commit_watermark(
        &self,
        items: usize,
        bytes: usize,
        range: Option<SpindleProjectionRange>,
        batch_partitions: &std::collections::BTreeSet<(&'static str, chrono::NaiveDate)>,
    ) -> Result<(), String> {
        let published = {
            let mut staging = self.spindle_staging.lock();
            staging.inflight_items = staging.inflight_items.saturating_sub(items);
            staging.inflight_bytes = staging.inflight_bytes.saturating_sub(bytes);
            if range.is_some() || staging.deferred_range.is_some() {
                staging
                    .uncommitted_partitions
                    .extend(batch_partitions.iter().copied());
            }
            SpindleProjectionRange::append(&mut staging.deferred_range, range)?;
            staging.deferred_range.clone()
        };
        if let Some(published) = published {
            self.merge_persisted_uncommitted();
            let next_wm = published.commit_published(&self.catalog)?;
            let mut staging = self.spindle_staging.lock();
            // flush_lock keeps the deferred range unchanged during catalog persistence.
            staging.deferred_range = None;
            staging.uncommitted_partitions.clear();
            drop(staging);
            self.clear_persisted_uncommitted(Some(next_wm));
        } else {
            self.clear_persisted_uncommitted(None);
        }
        Ok(())
    }

    fn flush_to_olap(&self) -> Result<usize, String> {
        let _flush_guard = self.flush_lock.lock();
        let mut flushed_total = 0usize;
        struct SpindleCommitReceipt {
            items: usize,
            bytes: usize,
            range: SpindleProjectionRange,
            partitions: std::collections::BTreeSet<(&'static str, chrono::NaiveDate)>,
        }
        let mut spindle_commits: Vec<SpindleCommitReceipt> = Vec::new();

        let mut spindle_err: Option<String> = None;

        // Phase 1: Flush replayable Spindle custody records in watermark-tagged plans only.
        let pending = self.spindle_staging.lock().take_pending_plan();
        if let Some((mut pending_plan, range, pending_items, pending_bytes)) = pending {
            let rows_before = pending_plan.total_rows();
            if let Err(err) = self.olap.execute_flush_plan(&mut pending_plan) {
                self.store_failed_plan(pending_plan, range, pending_items, pending_bytes);
                spindle_err = Some(err);
            } else {
                flushed_total = flushed_total.saturating_add(rows_before);
                spindle_commits.push(SpindleCommitReceipt {
                    items: pending_items,
                    bytes: pending_bytes,
                    range,
                    partitions: pending_plan.uncommitted_partitions().clone(),
                });
            }
        }

        if spindle_err.is_none() {
            let staged = self.spindle_staging.lock().take_for_flush();
            let staged_items = staged.total_len();
            let staged_bytes = staged.staged_bytes;
            if let Some(range) = staged.range {
                match self
                    .olap
                    .plan_spindle_flush(&staged.metrics, &staged.logs, &staged.traces)
                {
                    Ok(plan) => {
                        let mut splan = plan.with_spindle_watermark(Some(range.last_id()));
                        if let Err(err) = self.olap.execute_flush_plan(&mut splan) {
                            self.store_failed_plan(splan, range, staged_items, staged_bytes);
                            spindle_err = Some(err);
                        } else {
                            flushed_total = flushed_total.saturating_add(staged_items);
                            spindle_commits.push(SpindleCommitReceipt {
                                items: staged_items,
                                bytes: staged_bytes,
                                range,
                                partitions: splan.uncommitted_partitions().clone(),
                            });
                        }
                    }
                    Err(err) => {
                        let mut restored = SpindleStaging {
                            metrics: staged.metrics,
                            logs: staged.logs,
                            traces: staged.traces,
                            range: Some(range),
                            ..SpindleStaging::default()
                        };
                        restored.recompute_bytes();
                        let mut staging = self.spindle_staging.lock();
                        staging.inflight_items =
                            staging.inflight_items.saturating_sub(staged_items);
                        staging.inflight_bytes =
                            staging.inflight_bytes.saturating_sub(staged_bytes);
                        staging.restore_front(restored)?;
                        spindle_err = Some(err);
                    }
                }
            }
        }

        // Phase 2: Flush non-replayable volatile host/socket telemetry without Spindle rollback watermarks.
        let vol_res: Result<usize, String> = (|| {
            let mut vol_flushed = 0usize;
            let pending_vol = self.spindle_staging.lock().pending_volatile_plan.take();
            if let Some(mut vplan) = pending_vol {
                let rows_before = vplan.total_rows();
                if let Err(err) = self.olap.execute_flush_plan(&mut vplan) {
                    self.spindle_staging.lock().pending_volatile_plan = Some(vplan);
                    return Err(err);
                }
                vol_flushed = vol_flushed.saturating_add(rows_before);
            }

            let volatile_metrics = self.ring.write().drain_unflushed();
            let volatile_logs = std::mem::take(&mut *self.pending_logs.write());
            let volatile_traces = std::mem::take(&mut *self.pending_traces.write());
            let fs_events = std::mem::take(&mut *self.pending_fs_events.write());
            let edges = self
                .latest
                .read()
                .as_ref()
                .map(|s| s.graph.to_edge_records(s.snapshot.timestamp_unix_ms))
                .unwrap_or_default();

            let vbatch = volatile_metrics.len()
                + volatile_logs.len()
                + volatile_traces.len()
                + fs_events.len()
                + edges.len();
            if vbatch > 0 {
                let mut vplan = match self.olap.plan_flush(
                    &volatile_metrics,
                    &volatile_logs,
                    &volatile_traces,
                    &fs_events,
                    &edges,
                ) {
                    Ok(plan) => plan,
                    Err(err) => {
                        self.ring.write().restore_unflushed(volatile_metrics);
                        restore_bounded_queue(
                            &self.pending_logs,
                            volatile_logs,
                            1024,
                            &self.overflow_dropped,
                        );
                        restore_bounded_queue(
                            &self.pending_traces,
                            volatile_traces,
                            1024,
                            &self.overflow_dropped,
                        );
                        restore_bounded_queue(
                            &self.pending_fs_events,
                            fs_events,
                            2048,
                            &self.overflow_dropped,
                        );
                        return Err(err);
                    }
                };
                if let Err(err) = self.olap.execute_flush_plan(&mut vplan) {
                    self.spindle_staging.lock().pending_volatile_plan = Some(vplan);
                    return Err(err);
                }
                vol_flushed = vol_flushed.saturating_add(vbatch);
            }
            Ok(vol_flushed)
        })();

        // Phase 3: Advance Spindle watermark for committed Spindle batches before propagating volatile errors.
        if !spindle_commits.is_empty() {
            let mut total_items = 0usize;
            let mut total_bytes = 0usize;
            let mut published_range = None;
            let mut all_parts = std::collections::BTreeSet::new();
            for SpindleCommitReceipt {
                items,
                bytes,
                range,
                partitions: parts,
            } in spindle_commits
            {
                total_items = total_items.saturating_add(items);
                total_bytes = total_bytes.saturating_add(bytes);
                SpindleProjectionRange::append(&mut published_range, Some(range))?;
                all_parts.extend(parts);
            }
            self.finish_inflight_and_commit_watermark(
                total_items,
                total_bytes,
                published_range,
                &all_parts,
            )?;
        } else if self.spindle_staging.lock().deferred_range.is_some() {
            self.finish_inflight_and_commit_watermark(
                0,
                0,
                None,
                &std::collections::BTreeSet::new(),
            )?;
        } else if vol_res.is_ok() && spindle_err.is_none() {
            let committed = self
                .catalog
                .read_watermark("spindle_watermark_id")
                .ok()
                .flatten();
            self.clear_persisted_uncommitted(committed);
        }

        if let Some(err) = spindle_err {
            return Err(err);
        }
        let vol_rows = vol_res?;
        Ok(flushed_total.saturating_add(vol_rows))
    }

    fn run_janitor_pass(&self) -> Result<fabric_store::JanitorStats, String> {
        let _flush_guard = self.flush_lock.lock();
        self.merge_persisted_uncommitted();
        let protected_owned = self.protected_partition_set();
        let protected: std::collections::BTreeSet<(&str, chrono::NaiveDate)> = protected_owned
            .iter()
            .map(|&(domain, day)| (domain as &str, day))
            .collect();
        self.janitor.run_pass_excluding(None, &protected)
    }

    fn handle_rpc(&self, req: DaemonRequest) -> DaemonResponse {
        match req {
            DaemonRequest::Ping => DaemonResponse::Pong {
                pid: std::process::id(),
                uptime_ms: self.start_instant.elapsed().as_millis() as u64,
            },
            DaemonRequest::Snapshot => match self.latest.read().as_ref() {
                Some(state) => DaemonResponse::Snapshot(Box::new(state.snapshot.clone())),
                None => DaemonResponse::Error {
                    message: "snapshot warming up".into(),
                },
            },
            DaemonRequest::Doctor => {
                let summary = self.guard.inspect_summary();
                DaemonResponse::Doctor(build_doctor_report_from_summary(
                    &summary,
                    true,
                    &self.config.data_dir.to_string_lossy(),
                    &self.config.tmp_dir.to_string_lossy(),
                ))
            }
            DaemonRequest::Blame { resource } => {
                let Ok(res) = resource.parse::<BlameResource>() else {
                    return DaemonResponse::Error {
                        message: format!("invalid blame target: {resource}"),
                    };
                };
                match self.latest.read().as_ref() {
                    Some(state) => DaemonResponse::Blame(state.graph.blame(&res, &state.snapshot)),
                    None => DaemonResponse::Error {
                        message: "snapshot warming up".into(),
                    },
                }
            }
            DaemonRequest::Lineage {
                target,
                direction,
                max_depth,
            } => {
                let dir = direction.parse::<Direction>().unwrap_or(Direction::Both);
                match self.latest.read().as_ref() {
                    Some(state) => {
                        DaemonResponse::Lineage(state.graph.lineage(&target, dir, max_depth))
                    }
                    None => DaemonResponse::Error {
                        message: "graph warming up".into(),
                    },
                }
            }
            DaemonRequest::Drift { pillar } => {
                let filter = pillar.as_deref().and_then(|s| s.parse::<PillarKind>().ok());
                match self.latest.read().as_ref() {
                    Some(state) => {
                        let mut drift = state.drift.clone();
                        if let Some(p) = filter {
                            let tag = p.to_string();
                            drift.findings.retain(|f| f.pillar == tag);
                            drift.total_findings = drift.findings.len();
                            drift.critical_count = drift
                                .findings
                                .iter()
                                .filter(|f| f.severity == "critical")
                                .count();
                            drift.warn_count = drift
                                .findings
                                .iter()
                                .filter(|f| f.severity == "warn")
                                .count();
                            drift.info_count = drift
                                .findings
                                .iter()
                                .filter(|f| f.severity == "info")
                                .count();
                        }
                        DaemonResponse::Drift(drift)
                    }
                    None => DaemonResponse::Error {
                        message: "drift evaluator warming up".into(),
                    },
                }
            }
            DaemonRequest::HotMetrics { since_ms } => DaemonResponse::HotMetrics {
                points: self.ring.read().query_since(since_ms),
            },
            DaemonRequest::Flush => match self.flush_to_olap() {
                Ok(rows) => DaemonResponse::Flushed { rows },
                Err(message) => DaemonResponse::Error { message },
            },
        }
    }
}

/// Runs the bounded converged telemetry daemon until SIGINT/SIGTERM or `max_ticks`.
pub async fn run_daemon(config: DaemonConfig) -> Result<(), String> {
    run_daemon_mode(config, false).await
}

/// Projects a dedicated Spindle origin into a separate data directory.
///
/// The daemon exposes query RPC under that directory's state path. It skips host
/// collection and the general ingest socket.
pub async fn run_spindle_only_daemon(config: DaemonConfig) -> Result<(), String> {
    run_daemon_mode(config, true).await
}

async fn run_daemon_mode(mut config: DaemonConfig, spindle_only: bool) -> Result<(), String> {
    config.data_dir = validate_non_tmp_path_on_disk(&config.data_dir).map_err(|e| e.to_string())?;
    config.tmp_dir = validate_non_tmp_path_on_disk(&config.tmp_dir).map_err(|e| e.to_string())?;
    config.spindle_socket = config
        .spindle_socket
        .take()
        .map(|s| validate_non_tmp_path_on_disk(&s).map_err(|e| e.to_string()))
        .transpose()?;
    if spindle_only {
        if config.data_dir == Path::new(fabric_core::DEFAULT_DATA_DIR) {
            return Err("Spindle-only projection requires a separate data directory".into());
        }
        if config.spindle_socket.is_none() {
            return Err("Spindle-only projection requires a Spindle socket".into());
        }
    } else {
        config.runtime_dir = config
            .runtime_dir
            .take()
            .map(|d| validate_non_tmp_path_on_disk(&d).map_err(|e| e.to_string()))
            .transpose()?;
    }
    let export_node = if spindle_only {
        None
    } else {
        configured_host_export_node()?
    };
    let mut host_exporter = export_node
        .as_deref()
        .map(|node| HostExporter::new(config.spindle_socket.as_deref(), node))
        .transpose()?;

    let catalog = CatalogStore::open(&config.data_dir)?;
    let layout = catalog.layout().clone();
    if spindle_only {
        // Tie the RPC to the same isolated data and writer lock as this origin.
        config.runtime_dir = Some(layout.state_dir());
    }
    let writer_lock = fabric_store::try_acquire_writer_lock(&layout)?;
    let initial_spindle_watermark = catalog.read_watermark("spindle_watermark_id")?.unwrap_or(0);
    let olap = OlapParquetStore::new(layout.clone());
    let janitor = RetentionJanitor::new(
        olap.clone(),
        JanitorConfig {
            max_disk_bytes: config.max_disk_bytes,
            ..Default::default()
        },
    );

    let (metrics_q, mut metrics_rx, dropped_m) = BoundedTelemetryQueue::<MetricPoint>::new(1024);
    let (logs_q, mut logs_rx, dropped_l) = BoundedTelemetryQueue::<LogRecord>::new(512);
    let (traces_q, mut traces_rx, dropped_t) = BoundedTelemetryQueue::<TraceSpanRecord>::new(512);

    let total_dropped = Arc::new(AtomicU64::new(0));
    let overflow_dropped = Arc::new(AtomicU64::new(0));
    let guard = SelfPreservationGuard::new(
        config.data_dir.to_string_lossy().into_owned(),
        config.tmp_dir.to_string_lossy().into_owned(),
        total_dropped.clone(),
    );

    let runtime = Arc::new(DaemonRuntime {
        start_instant: std::time::Instant::now(),
        config: config.clone(),
        catalog,
        olap,
        janitor,
        _writer_lock: writer_lock,
        flush_lock: Mutex::new(()),
        ring: RwLock::new(HotMetricRing::with_capacity_bytes(config.ring_budget_bytes)),
        latest: RwLock::new(None),
        pending_logs: RwLock::new(Vec::new()),
        pending_traces: RwLock::new(Vec::new()),
        pending_fs_events: RwLock::new(Vec::new()),
        polled_spindle_cursor: AtomicU64::new(initial_spindle_watermark),
        spindle_staging: Mutex::new(SpindleStaging::default()),
        last_spindle_error: RwLock::new(None),
        overflow_dropped: overflow_dropped.clone(),
        guard,
    });

    let daemon_sock = config
        .runtime_dir
        .as_ref()
        .map_or_else(|| default_daemon_socket(&layout), |d| d.join("daemon.sock"));
    if let Some(parent) = daemon_sock.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if daemon_sock.exists() {
        let _ = fs::remove_file(&daemon_sock);
    }
    let rpc_listener = UnixListener::bind(&daemon_sock)
        .map_err(|e| format!("failed binding {}: {e}", daemon_sock.display()))?;
    let rpc_runtime = runtime.clone();
    let rpc_permits = Arc::new(Semaphore::new(MAX_CONCURRENT_RPC_CONNS));
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let rpc_task = tokio::spawn(async move {
        let mut conn_tasks = tokio::task::JoinSet::new();
        loop {
            while conn_tasks.try_join_next().is_some() {}
            if *shutdown_rx.borrow() {
                break;
            }
            let permit = tokio::select! {
                biased;
                _ = shutdown_rx.changed() => break,
                res = rpc_permits.clone().acquire_owned() => match res {
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
                res = rpc_listener.accept() => match res {
                    Ok((s, _)) => s,
                    Err(_) => {
                        drop(permit);
                        continue;
                    }
                }
            };
            let rt = rpc_runtime.clone();
            conn_tasks.spawn(async move {
                let _permit = permit;
                let (reader, mut writer) = stream.into_split();
                let mut reader = TokioBufReader::with_capacity(8192, reader);
                let reply = match tokio::time::timeout(
                    RPC_IO_TIMEOUT,
                    read_bounded_line(&mut reader, RPC_LINE_LIMIT_BYTES),
                )
                .await
                {
                    Ok(Ok(Some(Some(line)))) => {
                        match serde_json::from_str::<DaemonRequest>(line.trim()) {
                            Ok(req) => rt.handle_rpc(req),
                            Err(e) => DaemonResponse::Error {
                                message: e.to_string(),
                            },
                        }
                    }
                    Ok(Ok(Some(None))) => DaemonResponse::Error {
                        message: format!("rpc request exceeds {RPC_LINE_LIMIT_BYTES} byte limit"),
                    },
                    _ => return,
                };
                let mut bytes = match serde_json::to_vec(&reply) {
                    Ok(b) => b,
                    Err(e) => serde_json::to_vec(&DaemonResponse::Error {
                        message: format!("serialize daemon reply failed: {e}"),
                    })
                    .unwrap_or_default(),
                };
                if !bytes.is_empty() {
                    bytes.push(b'\n');
                    let _ = tokio::time::timeout(RPC_IO_TIMEOUT, writer.write_all(&bytes)).await;
                    let _ = tokio::time::timeout(RPC_IO_TIMEOUT, writer.shutdown()).await;
                }
            });
        }
        conn_tasks.abort_all();
        while conn_tasks.join_next().await.is_some() {}
    });

    let ingest_host = read_hostname();
    let ingest = (!spindle_only).then(|| {
        let ingest_sock = config
            .runtime_dir
            .as_ref()
            .map_or_else(|| default_ingest_socket(&layout), |d| d.join("ingest.sock"));
        let socket_for_cleanup = ingest_sock.clone();
        let serve_host = ingest_host.clone();
        let ingest_shutdown_rx = shutdown_tx.subscribe();
        let task = tokio::spawn(async move {
            let _ = serve_ingest_socket_until_shutdown(
                &ingest_sock,
                metrics_q,
                logs_q,
                traces_q,
                serve_host,
                ingest_shutdown_rx,
            )
            .await;
        });
        (socket_for_cleanup, task)
    });

    let mut fs_tracker = FsChurnTracker::new();
    let mut vol_detector = EwmaMadDetector::default();
    let mut sketch = DDSketch::default();
    let mut last_flush = std::time::Instant::now();
    let mut tick_count = 0u64;
    let interval = Duration::from_millis(config.interval_ms.max(250));
    let mut ticker = tokio::time::interval(interval);

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| e.to_string())?;

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = sigterm.recv() => break,
            _ = ticker.tick() => {
                tick_count += 1;
                let cgroup_pressure = crate::resource_budget::cgroup_memory_pressure();

                // Drain bounded queues.
                let extra_metrics = drain_telemetry_queues(
                    &mut metrics_rx,
                    &mut logs_rx,
                    &mut traces_rx,
                    &runtime.pending_logs,
                    &runtime.pending_traces,
                    &dropped_m,
                    &dropped_l,
                    &dropped_t,
                    &overflow_dropped,
                    &total_dropped,
                );

                let mut spindle_backpressure = false;
                if let Some(ref spindle_sock) = config.spindle_socket
                    && spindle_sock.exists()
                {
                    let after_id = runtime.polled_spindle_cursor.load(Ordering::Acquire);
                    let sock = spindle_sock.clone();
                    let host = ingest_host.clone();
                    let catalog = runtime.catalog.clone();
                    match tokio::task::spawn_blocking(move || {
                        poll_spindle_projection_records(&sock, &catalog, after_id, 64, &host)
                    })
                    .await
                    {
                        Ok(Ok(poll)) => {
                            *runtime.last_spindle_error.write() = poll.tail_error;
                            let mut accepted_cursor = after_id;
                            for (key, env) in poll.records {
                                let id = key.record_id;
                                if is_forwarded_host_signal(&key, &env, export_node.as_deref()) {
                                    let included = SpindleProjectionRange::include(
                                        &mut runtime.spindle_staging.lock().range,
                                        &key,
                                    );
                                    if let Err(error) = included {
                                        *runtime.last_spindle_error.write() = Some(error);
                                        spindle_backpressure = true;
                                        break;
                                    }
                                } else if !enqueue_spindle_record(key, env, &runtime, !cgroup_pressure) {
                                    spindle_backpressure = true;
                                    break;
                                }
                                accepted_cursor = id;
                            }
                            if accepted_cursor > after_id {
                                runtime
                                    .polled_spindle_cursor
                                    .store(accepted_cursor, Ordering::Release);
                            }
                        }
                        Ok(Err(err)) => {
                            *runtime.last_spindle_error.write() = Some(err);
                        }
                        Err(err) => {
                            *runtime.last_spindle_error.write() = Some(err.to_string());
                        }
                    }
                }

                if !spindle_only && !cgroup_pressure {
                    let mut state = run_collect_cycle(&runtime.guard, &mut fs_tracker, &mut vol_detector, &mut sketch);
                    if let Some(exporter) = host_exporter.as_mut() {
                        exporter.collect(&mut state);
                    }
                    state.metrics.extend(extra_metrics);

                    runtime.ring.write().push_batch(&state.metrics);
                    {
                        let mut fsev = runtime.pending_fs_events.write();
                        if fsev.len() < 2048 {
                            fsev.extend(state.fs_events.iter().cloned());
                        }
                    }

                    let _ = runtime.catalog.commit_cycle(&state.snapshot, &state.drift, &state.graph);
                    *runtime.latest.write() = Some(state);
                } else if !extra_metrics.is_empty() {
                    runtime.ring.write().push_unflushed_batch(&extra_metrics);
                }

                // Self-shedding check, Spindle backpressure, or 60s flush cadence.
                let force_shed = runtime.guard.is_shedding();
                if cgroup_pressure || force_shed {
                    runtime.ring.write().release_hot_history();
                }
                let scheduled = tick_count == 1
                    || force_shed
                    || spindle_backpressure
                    || last_flush.elapsed() >= Duration::from_secs(60);
                if (scheduled || cgroup_pressure)
                    && let Err(err) = runtime.flush_to_olap()
                {
                    *runtime.last_spindle_error.write() = Some(format!("olap flush failed: {err}"));
                }
                if scheduled {
                    let _ = runtime.run_janitor_pass();
                    last_flush = std::time::Instant::now();
                }

                if let Some(limit) = config.max_ticks && tick_count >= limit {
                    break;
                }
            }
        }
    }

    let _ = shutdown_tx.send(true);
    drop(host_exporter);
    let _ = rpc_task.await;
    let ingest_sock = if let Some((socket, task)) = ingest {
        let _ = task.await;
        Some(socket)
    } else {
        None
    };
    let tail_metrics = drain_telemetry_queues(
        &mut metrics_rx,
        &mut logs_rx,
        &mut traces_rx,
        &runtime.pending_logs,
        &runtime.pending_traces,
        &dropped_m,
        &dropped_l,
        &dropped_t,
        &overflow_dropped,
        &total_dropped,
    );
    if !tail_metrics.is_empty() {
        runtime.ring.write().push_batch(&tail_metrics);
    }
    let final_flush = runtime.flush_to_olap();
    let _ = runtime.run_janitor_pass();
    let _ = fs::remove_file(&daemon_sock);
    if let Some(ingest_sock) = ingest_sock {
        let _ = fs::remove_file(&ingest_sock);
    }
    drop(runtime);
    final_flush.map(|_| ())
}

#[allow(clippy::too_many_arguments)]
fn drain_telemetry_queues(
    metrics_rx: &mut tokio::sync::mpsc::Receiver<MetricPoint>,
    logs_rx: &mut tokio::sync::mpsc::Receiver<LogRecord>,
    traces_rx: &mut tokio::sync::mpsc::Receiver<TraceSpanRecord>,
    pending_logs: &RwLock<Vec<LogRecord>>,
    pending_traces: &RwLock<Vec<TraceSpanRecord>>,
    dropped_m: &AtomicU64,
    dropped_l: &AtomicU64,
    dropped_t: &AtomicU64,
    overflow_dropped: &AtomicU64,
    total_dropped: &AtomicU64,
) -> Vec<MetricPoint> {
    let mut extra_metrics = Vec::new();
    while let Ok(m) = metrics_rx.try_recv() {
        if extra_metrics.len() < 4096 {
            extra_metrics.push(m);
        } else {
            dropped_m.fetch_add(1, Ordering::Relaxed);
        }
    }
    {
        let mut logs = pending_logs.write();
        while let Ok(l) = logs_rx.try_recv() {
            if logs.len() < 1024 {
                logs.push(l);
            } else {
                dropped_l.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    {
        let mut traces = pending_traces.write();
        while let Ok(t) = traces_rx.try_recv() {
            if traces.len() < 1024 {
                traces.push(t);
            } else {
                dropped_t.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    let drops = dropped_m.load(Ordering::Relaxed)
        + dropped_l.load(Ordering::Relaxed)
        + dropped_t.load(Ordering::Relaxed)
        + overflow_dropped.load(Ordering::Relaxed);
    total_dropped.store(drops, Ordering::Relaxed);
    extra_metrics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_olap_flush_preserves_ring_and_spindle_watermark() {
        let base = std::env::var_os("TMPDIR")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .filter(|p| !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var("HOME").expect("HOME set"))
                    .join(".cache")
                    .join("fabrico11y-tmp")
            });
        fs::create_dir_all(&base).expect("create non-tmp base");
        let root = tempfile::Builder::new()
            .prefix("daemon-wm-")
            .tempdir_in(&base)
            .expect("tempdir");
        let data_dir = root.path().join("data");
        let tmp_dir = root.path().join("tmp");
        fs::create_dir_all(&tmp_dir).expect("create tmp dir");

        let catalog = CatalogStore::open(&data_dir).expect("open catalog");
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());
        let janitor = RetentionJanitor::new(olap.clone(), JanitorConfig::default());
        let guard = SelfPreservationGuard::new(
            data_dir.to_string_lossy().into_owned(),
            tmp_dir.to_string_lossy().into_owned(),
            Arc::new(AtomicU64::new(0)),
        );
        let olap_for_verify = olap.clone();
        let runtime = DaemonRuntime {
            start_instant: std::time::Instant::now(),
            config: DaemonConfig {
                data_dir: data_dir.clone(),
                tmp_dir,
                ..Default::default()
            },
            catalog,
            olap,
            janitor,
            _writer_lock: fabric_store::try_acquire_writer_lock(&layout).expect("writer lock"),
            flush_lock: Mutex::new(()),
            ring: RwLock::new(HotMetricRing::with_capacity_bytes(4096)),
            latest: RwLock::new(None),
            pending_logs: RwLock::new(Vec::new()),
            pending_traces: RwLock::new(Vec::new()),
            pending_fs_events: RwLock::new(Vec::new()),
            polled_spindle_cursor: AtomicU64::new(0),
            spindle_staging: Mutex::new(SpindleStaging::default()),
            last_spindle_error: RwLock::new(None),
            overflow_dropped: Arc::new(AtomicU64::new(0)),
            guard,
        };

        // Stage metric, log, and trace Spindle records with IDs 1..=6.
        for id in 1..=2 {
            assert!(enqueue_test_spindle_record(
                id,
                IngestEnvelope::Metric(MetricPoint {
                    timestamp_ms: 0,
                    name: format!("spindle.metric.{id}"),
                    value: id as f64,
                    host: "h".into(),
                    slice: "spindle".into(),
                    unit: "svc".into(),
                    pid: None,
                    labels_json: r#"{"k":"v"}"#.into(),
                }),
                &runtime,
            ));
        }
        for id in 3..=4 {
            assert!(enqueue_test_spindle_record(
                id,
                IngestEnvelope::Log(LogRecord {
                    timestamp_ms: 0,
                    level: "info".into(),
                    source: "spindle".into(),
                    unit: "svc".into(),
                    message: format!("spindle-log-{id}"),
                    attributes_json: "{}".into(),
                }),
                &runtime,
            ));
        }
        for id in 5..=6 {
            assert!(enqueue_test_spindle_record(
                id,
                IngestEnvelope::Trace(TraceSpanRecord {
                    timestamp_ms: 0,
                    trace_id: format!("t-{id}"),
                    span_id: format!("s-{id}"),
                    parent_span_id: None,
                    name: "op".into(),
                    service: "spindle".into(),
                    duration_ms: 1.0,
                    status: "ok".into(),
                    attributes_json: "{}".into(),
                }),
                &runtime,
            ));
        }

        // Churn volatile host metrics beyond ring_budget_bytes (4096) to prove
        // volatile ring eviction never drops staged Spindle records or logs.
        let churn: Vec<MetricPoint> = (0..250)
            .map(|i| MetricPoint {
                timestamp_ms: 1_700_000_000_000 + i,
                name: format!("volatile.host.{i}"),
                value: i as f64,
                host: "h".into(),
                slice: "agent.slice".into(),
                unit: "agent.service".into(),
                pid: Some(i as u32),
                labels_json: "{}".into(),
            })
            .collect();
        runtime.ring.write().push_batch(&churn);

        // Force Parquet write failure by obstructing olap/metrics with a regular file.
        let metrics_dir = layout.olap_domain_dir("metrics");
        if metrics_dir.exists() {
            fs::remove_dir_all(&metrics_dir).expect("clear metrics dir");
        }
        fs::create_dir_all(metrics_dir.parent().expect("olap parent")).expect("create olap parent");
        fs::write(&metrics_dir, b"block-olap").expect("create obstructing file");

        assert!(
            runtime.flush_to_olap().is_err(),
            "expected OLAP write failure when olap/metrics is a file"
        );
        assert_eq!(
            runtime
                .catalog
                .read_watermark("spindle_watermark_id")
                .expect("read watermark after failed flush"),
            None
        );
        {
            let staging = runtime.spindle_staging.lock();
            assert!(staging.pending_plan.is_some());
            assert_eq!(staging.effective_watermark(), Some(6));
            assert_eq!(staging.pending_or_staged_items(), 6);
        }

        // Remove obstruction and flush again; watermark must advance only now and all 6 Spindle records persist.
        fs::remove_file(&metrics_dir).expect("remove obstructing file");
        fs::create_dir_all(&metrics_dir).expect("recreate metrics dir");
        assert!(runtime.flush_to_olap().expect("retry flush cleanly") >= 6);
        assert_eq!(
            runtime
                .catalog
                .read_watermark("spindle_watermark_id")
                .expect("read watermark after successful flush"),
            Some(6)
        );
        {
            let staging = runtime.spindle_staging.lock();
            assert!(staging.pending_plan.is_none());
            assert_eq!(staging.total_len(), 0);
        }
        assert_eq!(
            olap_for_verify
                .list_parquet_files("logs", None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            olap_for_verify
                .list_parquet_files("traces", None)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn inflight_staging_bound_and_deferred_watermark_retry_hold() {
        let base = std::env::var_os("TMPDIR")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .filter(|p| !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var("HOME").expect("HOME set"))
                    .join(".cache")
                    .join("fabrico11y-tmp")
            });
        fs::create_dir_all(&base).expect("create non-tmp base");
        let root = tempfile::Builder::new()
            .prefix("daemon-deferred-")
            .tempdir_in(&base)
            .expect("tempdir");
        let data_dir = root.path().join("data");
        let tmp_dir = root.path().join("tmp");
        fs::create_dir_all(&tmp_dir).expect("create tmp dir");

        let catalog = CatalogStore::open(&data_dir).expect("open catalog");
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());
        let janitor = RetentionJanitor::new(olap.clone(), JanitorConfig::default());
        let guard = SelfPreservationGuard::new(
            data_dir.to_string_lossy().into_owned(),
            tmp_dir.to_string_lossy().into_owned(),
            Arc::new(AtomicU64::new(0)),
        );
        let runtime = DaemonRuntime {
            start_instant: std::time::Instant::now(),
            config: DaemonConfig {
                data_dir: data_dir.clone(),
                tmp_dir,
                ..Default::default()
            },
            catalog,
            olap,
            janitor,
            _writer_lock: fabric_store::try_acquire_writer_lock(&layout).expect("writer lock"),
            flush_lock: Mutex::new(()),
            ring: RwLock::new(HotMetricRing::with_capacity_bytes(4096)),
            latest: RwLock::new(None),
            pending_logs: RwLock::new(Vec::new()),
            pending_traces: RwLock::new(Vec::new()),
            pending_fs_events: RwLock::new(Vec::new()),
            polled_spindle_cursor: AtomicU64::new(0),
            spindle_staging: Mutex::new(SpindleStaging::default()),
            last_spindle_error: RwLock::new(None),
            overflow_dropped: Arc::new(AtomicU64::new(0)),
            guard,
        };

        // 1. Inflight items count against MAX_SPINDLE_STAGED_ITEMS so restore never exceeds cap.
        runtime.spindle_staging.lock().inflight_items = MAX_SPINDLE_STAGED_ITEMS - 2;
        for id in 1..=5 {
            let accepted = enqueue_test_spindle_record(
                id,
                IngestEnvelope::Log(LogRecord {
                    timestamp_ms: 0,
                    level: "info".into(),
                    source: "spindle".into(),
                    unit: "svc".into(),
                    message: format!("msg-{id}"),
                    attributes_json: "{}".into(),
                }),
                &runtime,
            );
            assert_eq!(accepted, id <= 2);
        }
        assert_eq!(runtime.spindle_staging.lock().total_len(), 2);
        runtime.spindle_staging.lock().inflight_items = 0;

        // 2. If OLAP write succeeds and catalog watermark persistence fails transiently,
        // deferred_watermark is preserved and committed on the next flush even with 0 new records.
        assert!(enqueue_test_spindle_record(
            3,
            IngestEnvelope::Log(LogRecord {
                timestamp_ms: 0,
                level: "info".into(),
                source: "spindle".into(),
                unit: "svc".into(),
                message: "wm-retry".into(),
                attributes_json: "{}".into(),
            }),
            &runtime,
        ));
        // Seed initial watermark so `catalog.redb` exists on disk, then obstruct it.
        runtime
            .catalog
            .set_watermark("spindle_watermark_id", 0)
            .expect("seed initial watermark");
        let catalog_path = layout.catalog_redb();
        let catalog_backup = catalog_path.with_extension("redb.bak");
        fs::rename(&catalog_path, &catalog_backup).expect("move catalog aside");
        fs::create_dir_all(&catalog_path).expect("obstruct catalog.redb with dir");

        assert!(
            runtime.flush_to_olap().is_err(),
            "watermark commit failure must return Err while keeping deferred watermark"
        );
        assert_eq!(
            runtime
                .spindle_staging
                .lock()
                .deferred_range
                .as_ref()
                .map(SpindleProjectionRange::last_id),
            Some(3)
        );
        assert_eq!(runtime.spindle_staging.lock().total_len(), 0);

        fs::remove_dir(&catalog_path).expect("remove obstructing dir");
        fs::rename(&catalog_backup, &catalog_path).expect("restore catalog");

        // Flush again with zero new records queued; deferred watermark must now commit cleanly.
        assert_eq!(runtime.flush_to_olap().expect("retry watermark commit"), 0);
        assert_eq!(runtime.spindle_staging.lock().deferred_range, None);
        assert_eq!(
            runtime
                .catalog
                .read_watermark("spindle_watermark_id")
                .expect("read committed watermark"),
            Some(3)
        );
    }

    #[test]
    fn staged_byte_budget_and_partial_flush_recovery_hold() {
        let base = std::env::var_os("TMPDIR")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .filter(|p| !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var("HOME").expect("HOME set"))
                    .join(".cache")
                    .join("fabrico11y-tmp")
            });
        fs::create_dir_all(&base).expect("create non-tmp base");
        let root = tempfile::Builder::new()
            .prefix("daemon-bytecap-")
            .tempdir_in(&base)
            .expect("tempdir");
        let data_dir = root.path().join("data");
        let tmp_dir = root.path().join("tmp");
        fs::create_dir_all(&tmp_dir).expect("create tmp dir");

        let catalog = CatalogStore::open(&data_dir).expect("open catalog");
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());
        let janitor = RetentionJanitor::new(olap.clone(), JanitorConfig::default());
        let guard = SelfPreservationGuard::new(
            data_dir.to_string_lossy().into_owned(),
            tmp_dir.to_string_lossy().into_owned(),
            Arc::new(AtomicU64::new(0)),
        );
        let runtime = DaemonRuntime {
            start_instant: std::time::Instant::now(),
            config: DaemonConfig {
                data_dir,
                tmp_dir,
                ..Default::default()
            },
            catalog,
            olap,
            janitor,
            _writer_lock: fabric_store::try_acquire_writer_lock(&layout).expect("writer lock"),
            flush_lock: Mutex::new(()),
            ring: RwLock::new(HotMetricRing::with_capacity_bytes(4096)),
            latest: RwLock::new(None),
            pending_logs: RwLock::new(Vec::new()),
            pending_traces: RwLock::new(Vec::new()),
            pending_fs_events: RwLock::new(Vec::new()),
            polled_spindle_cursor: AtomicU64::new(0),
            spindle_staging: Mutex::new(SpindleStaging::default()),
            last_spindle_error: RwLock::new(None),
            overflow_dropped: Arc::new(AtomicU64::new(0)),
            guard,
        };

        // 1. Byte cap (4 MiB) rejects large records well before item count limit (1024).
        let big_payload = "x".repeat(900 * 1024);
        let mut accepted = 0usize;
        for id in 1..=8_u64 {
            if enqueue_test_spindle_record(
                id,
                IngestEnvelope::Log(LogRecord {
                    timestamp_ms: 1_700_000_000_000,
                    level: "info".into(),
                    source: "spindle".into(),
                    unit: "svc".into(),
                    message: big_payload.clone(),
                    attributes_json: "{}".into(),
                }),
                &runtime,
            ) {
                accepted += 1;
            }
        }
        assert_eq!(accepted, 4);
        {
            let mut staging = runtime.spindle_staging.lock();
            assert!(staging.staged_bytes <= MAX_SPINDLE_STAGED_BYTES);
            *staging = SpindleStaging::default();
        }

        // 2. Intra-domain multi-day partial OLAP failure (day1 succeeds, day2 fails):
        // persisted dt=day1 must not duplicate and watermark remains withheld until day2 & logs flush.
        assert!(enqueue_test_spindle_record(
            1,
            IngestEnvelope::Metric(MetricPoint {
                timestamp_ms: 1_700_000_000_100, // 2023-11-14
                name: "partial.day1".into(),
                value: 7.0,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: None,
                labels_json: r#"{"sequence":10}"#.into(),
            }),
            &runtime,
        ));
        assert!(enqueue_test_spindle_record(
            2,
            IngestEnvelope::Metric(MetricPoint {
                timestamp_ms: 1_700_086_400_100, // 2023-11-15
                name: "partial.day2".into(),
                value: 8.0,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: None,
                labels_json: r#"{"sequence":11}"#.into(),
            }),
            &runtime,
        ));
        assert!(enqueue_test_spindle_record(
            3,
            IngestEnvelope::Log(LogRecord {
                timestamp_ms: 1_700_000_000_200,
                level: "info".into(),
                source: "spindle".into(),
                unit: "svc".into(),
                message: "partial.log".into(),
                attributes_json: "{}".into(),
            }),
            &runtime,
        ));

        let day2_dir = layout.olap_domain_dir("metrics").join("dt=2023-11-15");
        fs::create_dir_all(day2_dir.parent().expect("metrics dir")).expect("create metrics dir");
        fs::write(&day2_dir, b"obstruct").expect("obstruct dt=day2");

        assert!(runtime.flush_to_olap().is_err());
        assert_eq!(
            runtime
                .olap
                .list_parquet_files("metrics", None)
                .unwrap()
                .len(),
            1,
            "dt=day1 metrics persisted prior to dt=day2 failure"
        );
        {
            let staging = runtime.spindle_staging.lock();
            assert!(staging.pending_plan.is_some());
            assert_eq!(staging.inflight_items, 0);
            assert_eq!(staging.inflight_bytes, 0);
            assert_eq!(staging.effective_watermark(), Some(3));
            assert!(staging.pending_bytes() > 0);
        }
        assert_eq!(
            runtime
                .catalog
                .read_watermark("spindle_watermark_id")
                .unwrap(),
            None
        );

        fs::remove_file(&day2_dir).expect("remove dt=day2 obstruction");
        runtime.flush_to_olap().expect("retry remaining day & logs");
        assert_eq!(
            runtime
                .olap
                .list_parquet_files("metrics", None)
                .unwrap()
                .len(),
            2,
            "dt=day1 must not be duplicated on retry"
        );
        assert_eq!(
            runtime.olap.list_parquet_files("logs", None).unwrap().len(),
            1
        );
        assert!(runtime.spindle_staging.lock().pending_plan.is_none());
        assert_eq!(
            runtime
                .catalog
                .read_watermark("spindle_watermark_id")
                .unwrap(),
            Some(3)
        );
    }

    #[test]
    fn daemon_rpc_messages_roundtrip_json() {
        let requests = vec![
            DaemonRequest::Ping,
            DaemonRequest::Snapshot,
            DaemonRequest::Doctor,
            DaemonRequest::Blame {
                resource: "swap".into(),
            },
            DaemonRequest::Lineage {
                target: "/tmp/build".into(),
                direction: "both".into(),
                max_depth: 3,
            },
            DaemonRequest::Drift {
                pillar: Some("freshness".into()),
            },
            DaemonRequest::HotMetrics { since_ms: 42 },
            DaemonRequest::Flush,
        ];
        for req in requests {
            let bytes = serde_json::to_vec(&req).expect("serialize request");
            let _: DaemonRequest = serde_json::from_slice(&bytes).expect("deserialize request");
        }

        let responses = vec![
            DaemonResponse::Pong {
                pid: 42,
                uptime_ms: 100,
            },
            DaemonResponse::Snapshot(Box::default()),
            DaemonResponse::Doctor(fabric_core::DoctorReport::default()),
            DaemonResponse::Blame(BlameReport {
                resource: "swap".into(),
                total_observed: 1,
                unit_label: "bytes".into(),
                pressure_summary: "ok".into(),
                entries: Vec::new(),
            }),
            DaemonResponse::Lineage(LineageSubgraph {
                target: "swap".into(),
                matched_roots: Vec::new(),
                direction: Direction::Both,
                max_depth: 2,
                nodes: Vec::new(),
                edges: Vec::new(),
            }),
            DaemonResponse::Drift(DriftReport {
                schema_version: SCHEMA_VERSION.into(),
                generated_at_unix_ms: 123,
                total_findings: 0,
                critical_count: 0,
                warn_count: 0,
                info_count: 0,
                findings: Vec::new(),
            }),
            DaemonResponse::HotMetrics {
                points: vec![MetricPoint {
                    timestamp_ms: 1_700_000_000_000,
                    name: "custom.tokens_per_sec".into(),
                    value: 99.5,
                    host: "fedora-blade".into(),
                    slice: "spindle".into(),
                    unit: "spindle.service".into(),
                    pid: Some(7),
                    labels_json: r#"{"metric_unit":"tok/s"}"#.into(),
                }],
            },
            DaemonResponse::Flushed { rows: 3 },
            DaemonResponse::Error {
                message: "boom".into(),
            },
        ];
        for resp in responses {
            let bytes = serde_json::to_vec(&resp).expect("serialize daemon response");
            let decoded: DaemonResponse =
                serde_json::from_slice(&bytes).expect("deserialize daemon response");
            match (resp, decoded) {
                (
                    DaemonResponse::HotMetrics { points: orig },
                    DaemonResponse::HotMetrics { points: back },
                ) => assert_eq!(orig, back),
                (DaemonResponse::Flushed { rows: a }, DaemonResponse::Flushed { rows: b }) => {
                    assert_eq!(a, b)
                }
                _ => {}
            }
        }
    }

    #[test]
    fn uncommitted_partitions_stay_protected_across_partial_flush_and_deferred_watermark() {
        let base = std::env::var_os("TMPDIR")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .filter(|p| !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var("HOME").expect("HOME set"))
                    .join(".cache")
                    .join("fabrico11y-tmp")
            });
        fs::create_dir_all(&base).expect("create non-tmp base");
        let root = tempfile::Builder::new()
            .prefix("daemon-uncommitted-")
            .tempdir_in(&base)
            .expect("tempdir");
        let data_dir = root.path().join("data");
        let tmp_dir = root.path().join("tmp");
        fs::create_dir_all(&tmp_dir).expect("create tmp dir");

        let catalog = CatalogStore::open(&data_dir).expect("open catalog");
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());
        let janitor = RetentionJanitor::new(
            olap.clone(),
            JanitorConfig {
                max_disk_bytes: 1,
                ..Default::default()
            },
        );
        let guard = SelfPreservationGuard::new(
            data_dir.to_string_lossy().into_owned(),
            tmp_dir.to_string_lossy().into_owned(),
            Arc::new(AtomicU64::new(0)),
        );
        let runtime = DaemonRuntime {
            start_instant: std::time::Instant::now(),
            config: DaemonConfig {
                data_dir,
                tmp_dir,
                max_disk_bytes: 1,
                ..Default::default()
            },
            catalog,
            olap,
            janitor,
            _writer_lock: fabric_store::try_acquire_writer_lock(&layout).expect("writer lock"),
            flush_lock: Mutex::new(()),
            ring: RwLock::new(HotMetricRing::with_capacity_bytes(4096)),
            latest: RwLock::new(None),
            pending_logs: RwLock::new(Vec::new()),
            pending_traces: RwLock::new(Vec::new()),
            pending_fs_events: RwLock::new(Vec::new()),
            polled_spindle_cursor: AtomicU64::new(0),
            spindle_staging: Mutex::new(SpindleStaging::default()),
            last_spindle_error: RwLock::new(None),
            overflow_dropped: Arc::new(AtomicU64::new(0)),
            guard,
        };

        runtime
            .catalog
            .set_watermark("spindle_watermark_id", 9)
            .expect("seed retained prefix");
        assert!(enqueue_test_spindle_record(
            10,
            IngestEnvelope::Metric(MetricPoint {
                timestamp_ms: 1_700_000_000_100, // 2023-11-14
                name: "protect.day1".into(),
                value: 1.0,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: None,
                labels_json: r#"{"sequence":10}"#.into(),
            }),
            &runtime,
        ));
        assert!(enqueue_test_spindle_record(
            11,
            IngestEnvelope::Metric(MetricPoint {
                timestamp_ms: 1_700_086_400_100, // 2023-11-15
                name: "protect.day2".into(),
                value: 2.0,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: None,
                labels_json: r#"{"sequence":11}"#.into(),
            }),
            &runtime,
        ));

        let day2_dir = layout.olap_domain_dir("metrics").join("dt=2023-11-15");
        fs::write(&day2_dir, b"obstruct").expect("obstruct dt=day2");
        assert!(runtime.flush_to_olap().is_err());
        assert_eq!(
            runtime
                .olap
                .list_parquet_files("metrics", None)
                .unwrap()
                .len(),
            1
        );

        // Even under 1-byte disk quota, already-published day1 stays protected while pending_plan is active,
        // both for in-process maintenance and for an external janitor pass with no explicit exclusions.
        let stats1 = runtime
            .run_janitor_pass()
            .expect("janitor during partial flush");
        assert_eq!(stats1.evicted_partitions, 0);
        let external_janitor = RetentionJanitor::new(
            runtime.olap.clone(),
            JanitorConfig {
                max_disk_bytes: 1,
                ..Default::default()
            },
        );
        let ext_stats1 = external_janitor
            .run_pass(None)
            .expect("external janitor during partial flush");
        assert_eq!(ext_stats1.evicted_partitions, 0);
        assert_eq!(
            runtime
                .olap
                .list_parquet_files("metrics", None)
                .unwrap()
                .len(),
            1
        );

        // Unblock day2 OLAP write, but obstruct catalog.redb so pending_plan finishes while watermark fails.
        fs::remove_file(&day2_dir).expect("remove dt=day2 obstruction");
        let catalog_backup = layout.catalog_redb().with_extension("redb.bak");
        fs::rename(layout.catalog_redb(), &catalog_backup).expect("move catalog aside");
        fs::create_dir_all(layout.catalog_redb()).expect("obstruct catalog.redb");
        assert!(runtime.flush_to_olap().is_err());
        assert_eq!(
            runtime
                .olap
                .list_parquet_files("metrics", None)
                .unwrap()
                .len(),
            2
        );
        {
            let staging = runtime.spindle_staging.lock();
            assert!(staging.pending_plan.is_none());
            assert_eq!(
                staging
                    .deferred_range
                    .as_ref()
                    .map(SpindleProjectionRange::last_id),
                Some(11)
            );
            assert_eq!(staging.uncommitted_partitions.len(), 2);
        }

        // Both day1 and day2 must still remain protected while deferred_watermark is uncommitted.
        let stats2 = runtime
            .run_janitor_pass()
            .expect("janitor during deferred watermark");
        assert_eq!(stats2.evicted_partitions, 0);
        let ext_stats2 = external_janitor
            .run_pass(None)
            .expect("external janitor during deferred watermark");
        assert_eq!(ext_stats2.evicted_partitions, 0);
        assert_eq!(
            runtime
                .olap
                .list_parquet_files("metrics", None)
                .unwrap()
                .len(),
            2
        );

        // Unblock catalog.redb so watermark commits and protection lifts.
        fs::remove_dir(layout.catalog_redb()).expect("remove catalog.redb obstruction");
        fs::rename(&catalog_backup, layout.catalog_redb()).expect("restore catalog binding");
        runtime
            .flush_to_olap()
            .expect("commit deferred watermark after recovery");
        assert_eq!(
            runtime
                .catalog
                .read_watermark("spindle_watermark_id")
                .unwrap(),
            Some(11)
        );
        {
            let staging = runtime.spindle_staging.lock();
            assert!(staging.deferred_range.is_none());
            assert!(staging.uncommitted_partitions.is_empty());
        }

        let stats3 = runtime
            .run_janitor_pass()
            .expect("janitor evicts once watermark committed");
        assert_eq!(stats3.evicted_partitions, 2);
        assert!(
            runtime
                .olap
                .list_parquet_files("metrics", None)
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn daemon_shutdown_drains_queued_ingest_records_into_final_flush() {
        let base = std::env::var_os("TMPDIR")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .filter(|p| !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var("HOME").expect("HOME set"))
                    .join(".cache")
                    .join("fabrico11y-tmp")
            });
        fs::create_dir_all(&base).expect("create non-tmp base");
        let root = tempfile::Builder::new()
            .prefix("daemon-shutdown-drain-")
            .tempdir_in(&base)
            .expect("tempdir");

        // 1. drain_telemetry_queues + flush_to_olap persists buffered metrics/logs/traces to OLAP Parquet.
        let data_dir = root.path().join("drain-data");
        let tmp_dir = root.path().join("drain-tmp");
        fs::create_dir_all(&tmp_dir).expect("create drain tmp dir");
        let catalog = CatalogStore::open(&data_dir).expect("open catalog");
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());
        let janitor = RetentionJanitor::new(olap.clone(), JanitorConfig::default());
        let overflow_dropped = Arc::new(AtomicU64::new(0));
        let guard = SelfPreservationGuard::new(
            data_dir.to_string_lossy().into_owned(),
            tmp_dir.to_string_lossy().into_owned(),
            Arc::clone(&overflow_dropped),
        );
        let runtime = DaemonRuntime {
            start_instant: std::time::Instant::now(),
            config: DaemonConfig {
                data_dir: data_dir.clone(),
                tmp_dir: tmp_dir.clone(),
                spindle_socket: None,
                ..Default::default()
            },
            catalog,
            olap,
            janitor,
            _writer_lock: fabric_store::try_acquire_writer_lock(&layout).expect("writer lock"),
            flush_lock: Mutex::new(()),
            ring: RwLock::new(HotMetricRing::with_capacity_bytes(65_536)),
            latest: RwLock::new(None),
            pending_logs: RwLock::new(Vec::new()),
            pending_traces: RwLock::new(Vec::new()),
            pending_fs_events: RwLock::new(Vec::new()),
            polled_spindle_cursor: AtomicU64::new(0),
            spindle_staging: Mutex::new(SpindleStaging::default()),
            last_spindle_error: RwLock::new(None),
            overflow_dropped: Arc::clone(&overflow_dropped),
            guard,
        };

        let (mq, mut metrics_rx, dropped_m) = BoundedTelemetryQueue::<MetricPoint>::new(8);
        let (lq, mut logs_rx, dropped_l) = BoundedTelemetryQueue::<LogRecord>::new(8);
        let (tq, mut traces_rx, dropped_t) = BoundedTelemetryQueue::<TraceSpanRecord>::new(8);
        let total_dropped = Arc::new(AtomicU64::new(0));
        assert!(mq.try_emit(MetricPoint {
            timestamp_ms: 1_700_000_000_100,
            name: "drain.metric".into(),
            value: 42.0,
            host: "test-host".into(),
            slice: "agent.slice".into(),
            unit: "svc".into(),
            pid: Some(1234),
            labels_json: "{}".into(),
        }));
        assert!(lq.try_emit(LogRecord {
            timestamp_ms: 1_700_000_000_200,
            level: "info".into(),
            source: "ingest".into(),
            unit: "svc".into(),
            message: "tail log".into(),
            attributes_json: "{}".into(),
        }));
        assert!(tq.try_emit(TraceSpanRecord {
            timestamp_ms: 1_700_000_000_300,
            trace_id: "0123456789abcdef0123456789abcdef".into(),
            span_id: "0123456789abcdef".into(),
            parent_span_id: None,
            name: "tail.span".into(),
            service: "svc".into(),
            duration_ms: 3.5,
            status: "ok".into(),
            attributes_json: "{}".into(),
        }));

        let tail_metrics = drain_telemetry_queues(
            &mut metrics_rx,
            &mut logs_rx,
            &mut traces_rx,
            &runtime.pending_logs,
            &runtime.pending_traces,
            &dropped_m,
            &dropped_l,
            &dropped_t,
            &overflow_dropped,
            &total_dropped,
        );
        assert_eq!(tail_metrics.len(), 1);
        assert_eq!(total_dropped.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(
            overflow_dropped.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        runtime.ring.write().push_batch(&tail_metrics);
        runtime
            .flush_to_olap()
            .expect("final flush writes drained records");
        assert_eq!(
            runtime
                .olap
                .list_parquet_files("metrics", None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            runtime.olap.list_parquet_files("logs", None).unwrap().len(),
            1
        );
        assert_eq!(
            runtime
                .olap
                .list_parquet_files("traces", None)
                .unwrap()
                .len(),
            1
        );
        drop(runtime);

        // 2. run_daemon shuts down cleanly via max_ticks even while an idle ingest.sock client remains open.
        let daemon_root = root.path().join("run-daemon");
        let daemon_data = daemon_root.join("data");
        let daemon_tmp = daemon_root.join("tmp");
        let daemon_runtime = daemon_root.join("runtime");
        fs::create_dir_all(&daemon_tmp).expect("create daemon tmp dir");
        fs::create_dir_all(&daemon_runtime).expect("create daemon runtime dir");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&daemon_runtime, fs::Permissions::from_mode(0o700))
                .expect("chmod 700 daemon runtime");
        }
        let ingest_sock = daemon_runtime.join("ingest.sock");
        let daemon_task = tokio::spawn(run_daemon(DaemonConfig {
            data_dir: daemon_data,
            tmp_dir: daemon_tmp,
            interval_ms: 250,
            max_ticks: Some(2),
            spindle_socket: None,
            runtime_dir: Some(daemon_runtime),
            ..Default::default()
        }));

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let idle_client = loop {
            match tokio::net::UnixStream::connect(&ingest_sock).await {
                Ok(stream) => break stream,
                Err(_) if !daemon_task.is_finished() && tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                Err(err) => {
                    panic!("failed connecting idle ingest client before daemon exit: {err}")
                }
            }
        };

        let res = tokio::time::timeout(std::time::Duration::from_secs(15), daemon_task)
            .await
            .expect("run_daemon must shut down cleanly without hanging on idle ingest client")
            .expect("daemon join");
        res.expect("run_daemon exits cleanly");
        drop(idle_client);
        assert!(!ingest_sock.exists(), "ingest.sock removed on shutdown");
    }

    #[test]
    fn crash_before_watermark_rolls_back_spindle_without_deleting_volatile_telemetry() {
        let base = std::env::var_os("TMPDIR")
            .and_then(|s| s.into_string().ok())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .filter(|p| !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var("HOME").expect("HOME set"))
                    .join(".cache")
                    .join("fabrico11y-tmp")
            });
        fs::create_dir_all(&base).expect("create non-tmp base");
        let root = tempfile::Builder::new()
            .prefix("daemon-spindle-volatile-isolation-")
            .tempdir_in(&base)
            .expect("tempdir");
        let data_dir = root.path().join("data");
        let tmp_dir = root.path().join("tmp");
        fs::create_dir_all(&tmp_dir).expect("create tmp dir");

        let catalog = CatalogStore::open(&data_dir).expect("open catalog");
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());
        let janitor = RetentionJanitor::new(olap.clone(), JanitorConfig::default());
        let guard = SelfPreservationGuard::new(
            data_dir.to_string_lossy().into_owned(),
            tmp_dir.to_string_lossy().into_owned(),
            Arc::new(AtomicU64::new(0)),
        );
        let runtime = DaemonRuntime {
            start_instant: std::time::Instant::now(),
            config: DaemonConfig {
                data_dir,
                tmp_dir,
                spindle_socket: None,
                ..Default::default()
            },
            catalog,
            olap,
            janitor,
            _writer_lock: fabric_store::try_acquire_writer_lock(&layout).expect("writer lock"),
            flush_lock: Mutex::new(()),
            ring: RwLock::new(HotMetricRing::with_capacity_bytes(65_536)),
            latest: RwLock::new(None),
            pending_logs: RwLock::new(Vec::new()),
            pending_traces: RwLock::new(Vec::new()),
            pending_fs_events: RwLock::new(Vec::new()),
            polled_spindle_cursor: AtomicU64::new(0),
            spindle_staging: Mutex::new(SpindleStaging::default()),
            last_spindle_error: RwLock::new(None),
            overflow_dropped: Arc::new(AtomicU64::new(0)),
            guard,
        };

        let ts_ms = 1_700_000_000_100i64;
        assert!(enqueue_test_spindle_record(
            1,
            IngestEnvelope::Log(LogRecord {
                timestamp_ms: ts_ms - 1,
                level: "info".into(),
                source: "spindle".into(),
                unit: "spindle.service".into(),
                message: "earlier committed log".into(),
                attributes_json: "{}".into(),
            }),
            &runtime,
        ));
        assert_eq!(runtime.flush_to_olap().expect("commit earlier batch"), 1);
        let committed_logs = runtime.olap.list_parquet_files("logs", None).unwrap();
        assert_eq!(committed_logs.len(), 1);
        assert_eq!(
            runtime
                .catalog
                .read_watermark("spindle_watermark_id")
                .unwrap(),
            Some(1)
        );
        runtime.ring.write().push_batch(&[MetricPoint {
            timestamp_ms: ts_ms,
            name: "volatile.cpu".into(),
            value: 12.5,
            host: "fedora-blade".into(),
            slice: "agent.slice".into(),
            unit: "agent.service".into(),
            pid: Some(1001),
            labels_json: "{}".into(),
        }]);
        runtime.pending_logs.write().push(LogRecord {
            timestamp_ms: ts_ms,
            level: "info".into(),
            source: "journal".into(),
            unit: "agent.service".into(),
            message: "volatile log".into(),
            attributes_json: "{}".into(),
        });
        assert!(enqueue_test_spindle_record(
            2,
            IngestEnvelope::Metric(MetricPoint {
                timestamp_ms: ts_ms,
                name: "spindle.latency".into(),
                value: 3.25,
                host: "fedora-blade".into(),
                slice: "spindle".into(),
                unit: "spindle.service".into(),
                pid: Some(1002),
                labels_json: "{}".into(),
            }),
            &runtime,
        ));
        assert!(enqueue_test_spindle_record(
            3,
            IngestEnvelope::Log(LogRecord {
                timestamp_ms: ts_ms,
                level: "info".into(),
                source: "spindle".into(),
                unit: "spindle.service".into(),
                message: "spindle log".into(),
                attributes_json: "{}".into(),
            }),
            &runtime,
        ));

        // Obstruct catalog.redb so both OLAP plans publish their Parquet files while Spindle's
        // watermark commit fails, simulating a writer crash right before watermark durability.
        let catalog_backup = layout.catalog_redb().with_extension("redb.bak");
        fs::rename(layout.catalog_redb(), &catalog_backup).expect("move catalog aside");
        fs::create_dir_all(layout.catalog_redb()).expect("obstruct catalog.redb");
        assert!(
            runtime.flush_to_olap().is_err(),
            "flush_to_olap must fail when catalog watermark persistence is obstructed"
        );

        let before_m = runtime
            .olap
            .list_parquet_files("metrics", None)
            .expect("list metrics before crash recovery");
        let before_l = runtime
            .olap
            .list_parquet_files("logs", None)
            .expect("list logs before crash recovery");
        assert_eq!(
            before_m.len(),
            2,
            "expected separate volatile + spindle metrics files"
        );
        assert_eq!(
            before_l.len(),
            3,
            "expected separate earlier, volatile, and deferred log files"
        );

        fs::remove_dir(layout.catalog_redb()).expect("remove catalog obstruction");
        fs::rename(&catalog_backup, layout.catalog_redb()).expect("restore catalog binding");
        drop(runtime);

        // Restart rolls back the later Spindle range and keeps the earlier durable
        // prefix and independent host/socket files.
        let recovered_lock =
            fabric_store::try_acquire_writer_lock(&layout).expect("re-acquire writer lock");
        let olap_after = OlapParquetStore::new(layout.clone());
        let after_m = olap_after
            .list_parquet_files("metrics", None)
            .expect("list metrics after recovery");
        let after_l = olap_after
            .list_parquet_files("logs", None)
            .expect("list logs after recovery");
        assert_eq!(
            after_m.len(),
            1,
            "only volatile metrics file should survive"
        );
        assert_eq!(after_l.len(), 2, "earlier and volatile logs should survive");
        assert!(after_l.contains(&committed_logs[0]));
        assert!(before_m.contains(&after_m[0]));
        assert!(after_l.iter().all(|path| before_l.contains(path)));
        let reopened = CatalogStore::open(layout.data_dir()).unwrap();
        assert_eq!(
            reopened.read_watermark("spindle_watermark_id").unwrap(),
            Some(1)
        );
        drop(recovered_lock);
    }

    #[test]
    fn pending_retry_receipt_commits_and_releases_inflight_when_new_batch_fails() {
        let base = std::env::var_os("TMPDIR")
            .and_then(|s| s.into_string().ok())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .filter(|p| !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var("HOME").expect("HOME set"))
                    .join(".cache")
                    .join("fabrico11y-tmp")
            });
        fs::create_dir_all(&base).expect("create non-tmp base");
        let root = tempfile::Builder::new()
            .prefix("daemon-retry-receipt-")
            .tempdir_in(&base)
            .expect("tempdir");
        let data_dir = root.path().join("data");
        let tmp_dir = root.path().join("tmp");
        fs::create_dir_all(&tmp_dir).expect("create tmp dir");

        let catalog = CatalogStore::open(&data_dir).expect("open catalog");
        let layout = catalog.layout().clone();
        let olap = OlapParquetStore::new(layout.clone());
        let janitor = RetentionJanitor::new(olap.clone(), JanitorConfig::default());
        let guard = SelfPreservationGuard::new(
            data_dir.to_string_lossy().into_owned(),
            tmp_dir.to_string_lossy().into_owned(),
            Arc::new(AtomicU64::new(0)),
        );
        let runtime = DaemonRuntime {
            start_instant: std::time::Instant::now(),
            config: DaemonConfig {
                data_dir,
                tmp_dir,
                spindle_socket: None,
                ..Default::default()
            },
            catalog,
            olap,
            janitor,
            _writer_lock: fabric_store::try_acquire_writer_lock(&layout).expect("writer lock"),
            flush_lock: Mutex::new(()),
            ring: RwLock::new(HotMetricRing::with_capacity_bytes(65_536)),
            latest: RwLock::new(None),
            pending_logs: RwLock::new(Vec::new()),
            pending_traces: RwLock::new(Vec::new()),
            pending_fs_events: RwLock::new(Vec::new()),
            polled_spindle_cursor: AtomicU64::new(0),
            spindle_staging: Mutex::new(SpindleStaging::default()),
            last_spindle_error: RwLock::new(None),
            overflow_dropped: Arc::new(AtomicU64::new(0)),
            guard,
        };

        let metrics_domain_dir = layout.olap_domain_dir("metrics");
        fs::create_dir_all(&metrics_domain_dir).expect("create metrics dir");

        runtime
            .catalog
            .set_watermark("spindle_watermark_id", 49)
            .expect("seed retained prefix");

        // 1. Fail Spindle batch id=50 on day1 (2023-07-22) so it parks in pending_plan.
        let day1_ms = 1_690_000_000_000i64;
        assert!(enqueue_test_spindle_record(
            50,
            IngestEnvelope::Metric(MetricPoint {
                timestamp_ms: day1_ms,
                name: "spindle.retry.day1".into(),
                value: 1.0,
                host: "fedora-blade".into(),
                slice: "spindle".into(),
                unit: "spindle.service".into(),
                pid: Some(2001),
                labels_json: "{}".into(),
            }),
            &runtime,
        ));
        let day1_part = metrics_domain_dir.join("dt=2023-07-22");
        fs::write(&day1_part, b"obstruct-day1").expect("obstruct day1");
        assert!(runtime.flush_to_olap().is_err());
        assert!(runtime.spindle_staging.lock().pending_plan.is_some());

        // 2. Unblock day1, enqueue batch id=51 on day2 (2023-11-14) and obstruct day2.
        // Retried day1 batch MUST fold its receipt (releasing inflight accounting and committing wm=50)
        // even though newly staged day2 fails execute_flush_plan.
        fs::remove_file(&day1_part).expect("unblock day1");
        let day2_ms = 1_700_000_000_000i64;
        assert!(enqueue_test_spindle_record(
            51,
            IngestEnvelope::Metric(MetricPoint {
                timestamp_ms: day2_ms,
                name: "spindle.new.day2".into(),
                value: 2.0,
                host: "fedora-blade".into(),
                slice: "spindle".into(),
                unit: "spindle.service".into(),
                pid: Some(2002),
                labels_json: "{}".into(),
            }),
            &runtime,
        ));
        let day2_part = metrics_domain_dir.join("dt=2023-11-14");
        fs::write(&day2_part, b"obstruct-day2").expect("obstruct day2");
        assert!(runtime.flush_to_olap().is_err());

        {
            let staging = runtime.spindle_staging.lock();
            assert_eq!(staging.inflight_items, 0);
            assert_eq!(staging.inflight_bytes, 0);
            assert_eq!(
                staging.pending_plan.as_ref().map(|p| p.1.last_id()),
                Some(51),
                "day2 batch must remain parked for retry"
            );
        }
        assert_eq!(
            runtime
                .catalog
                .read_watermark("spindle_watermark_id")
                .expect("read committed watermark"),
            Some(50)
        );

        // 3. Unblock day2 and confirm retry commits wm=51 cleanly with both Parquet files present.
        fs::remove_file(&day2_part).expect("unblock day2");
        runtime.flush_to_olap().expect("retry day2 flush succeeds");
        assert_eq!(
            runtime
                .catalog
                .read_watermark("spindle_watermark_id")
                .expect("read final watermark"),
            Some(51)
        );
        assert_eq!(runtime.spindle_staging.lock().inflight_items, 0);
        assert_eq!(
            runtime
                .olap
                .list_parquet_files("metrics", None)
                .expect("list final metrics files")
                .len(),
            2
        );
    }
}
