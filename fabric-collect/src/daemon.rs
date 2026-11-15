//! Converged telemetry daemon, one-shot collector, and `daemon.sock` RPC responder.

use crate::cgroups::walk_cgroups_and_processes;
use crate::channel::BoundedTelemetryQueue;
use crate::fs_watch::FsChurnTracker;
use crate::host::{
    read_gpu_stats, read_hostname, read_memory_snapshot, read_mount_usages, read_psi_metrics,
};
use crate::self_guard::{SelfPreservationGuard, build_doctor_report_from_summary};
use crate::spindle_bridge::{IngestEnvelope, poll_spindle_custody_records, serve_ingest_socket};
use crate::systemd::collect_unit_records;
use chrono::Utc;
use fabric_core::{
    BlameReport, BlameResource, DDSketch, DEFAULT_TMP_DIR, DriftContext, DriftReport,
    EwmaMadDetector, FsEventRecord, HostSnapshot, LogRecord, MetricPoint, PillarKind,
    SCHEMA_VERSION, StorageLayout, TraceSpanRecord, evaluate_five_pillars, validate_non_tmp_path,
};
use fabric_graph::{CausalGraph, Direction, LineageSubgraph};
use fabric_store::{
    CatalogStore, DEFAULT_RING_BUDGET_BYTES, HotMetricRing, JanitorConfig, OlapParquetStore,
    RetentionJanitor,
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
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
use tokio::net::UnixListener;

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
        Self {
            data_dir: PathBuf::from(fabric_core::DEFAULT_DATA_DIR),
            tmp_dir: PathBuf::from(DEFAULT_TMP_DIR),
            interval_ms: 2000,
            max_ticks: None,
            ring_budget_bytes: DEFAULT_RING_BUDGET_BYTES,
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
        if validate_non_tmp_path(&path).is_ok() {
            return Some(path);
        }
    }
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(xdg).join("spindle").join("spindle.sock");
        if validate_non_tmp_path(&path).is_ok() {
            return Some(path);
        }
    }
    None
}

fn enqueue_envelope(
    env: IngestEnvelope,
    extra_metrics: &mut Vec<MetricPoint>,
    runtime: &DaemonRuntime,
) -> bool {
    let now_ms = Utc::now().timestamp_millis();
    match env {
        IngestEnvelope::Metric(mut pt) => {
            if extra_metrics.len() >= 4096 || !runtime.ring.read().can_buffer_unflushed() {
                return false;
            }
            if pt.timestamp_ms <= 0 {
                pt.timestamp_ms = now_ms;
            }
            extra_metrics.push(pt);
            true
        }
        IngestEnvelope::Log(mut log) => {
            let mut logs = runtime.pending_logs.write();
            if logs.len() >= 1024 {
                return false;
            }
            if log.timestamp_ms <= 0 {
                log.timestamp_ms = now_ms;
            }
            logs.push(log);
            true
        }
        IngestEnvelope::Trace(mut span) => {
            let mut traces = runtime.pending_traces.write();
            if traces.len() >= 1024 {
                return false;
            }
            if span.timestamp_ms <= 0 {
                span.timestamp_ms = now_ms;
            }
            traces.push(span);
            true
        }
    }
}

fn runtime_socket_path(layout: &StorageLayout, name: &str) -> PathBuf {
    if let Some(rt_env) = std::env::var_os("FABRIC_RUNTIME_DIR")
        && !rt_env.is_empty()
    {
        let dir = PathBuf::from(rt_env);
        if validate_non_tmp_path(&dir).is_ok() {
            return dir.join(name);
        }
    }
    if layout.data_dir() == Path::new(fabric_core::DEFAULT_DATA_DIR)
        && let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR")
    {
        let dir = PathBuf::from(xdg).join("fabrico11y");
        if validate_non_tmp_path(&dir).is_ok() {
            return dir.join(name);
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

/// Performs a single deterministic collection cycle without starting the resident daemon.
pub fn collect_once(data_dir: &Path, tmp_dir: &Path) -> Result<CollectedState, String> {
    validate_non_tmp_path(data_dir).map_err(|e| e.to_string())?;
    validate_non_tmp_path(tmp_dir).map_err(|e| e.to_string())?;
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
    flush_lock: Mutex<()>,
    ring: RwLock<HotMetricRing>,
    latest: RwLock<Option<CollectedState>>,
    pending_logs: RwLock<Vec<LogRecord>>,
    pending_traces: RwLock<Vec<TraceSpanRecord>>,
    pending_fs_events: RwLock<Vec<FsEventRecord>>,
    polled_spindle_cursor: AtomicU64,
    pending_spindle_watermark: RwLock<Option<u64>>,
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
    fn flush_to_olap(&self) -> Result<usize, String> {
        let _flush_guard = self.flush_lock.lock();
        let watermark = *self.pending_spindle_watermark.read();
        let metrics = self.ring.write().drain_unflushed();
        let logs = std::mem::take(&mut *self.pending_logs.write());
        let traces = std::mem::take(&mut *self.pending_traces.write());
        let fs_events = std::mem::take(&mut *self.pending_fs_events.write());
        let edges = self
            .latest
            .read()
            .as_ref()
            .map(|s| s.graph.to_edge_records(s.snapshot.timestamp_unix_ms))
            .unwrap_or_default();

        let total = metrics.len() + logs.len() + traces.len() + fs_events.len() + edges.len();
        let write_res = (|| -> Result<(), String> {
            let _ = self.olap.write_metrics(&metrics)?;
            let _ = self.olap.write_logs(&logs)?;
            let _ = self.olap.write_traces(&traces)?;
            let _ = self.olap.write_fs_events(&fs_events)?;
            let _ = self.olap.write_edges(&edges)?;
            Ok(())
        })();
        if let Err(err) = write_res {
            self.ring.write().restore_unflushed(metrics);
            restore_bounded_queue(&self.pending_logs, logs, 1024, &self.overflow_dropped);
            restore_bounded_queue(&self.pending_traces, traces, 1024, &self.overflow_dropped);
            restore_bounded_queue(
                &self.pending_fs_events,
                fs_events,
                2048,
                &self.overflow_dropped,
            );
            return Err(err);
        }
        if let Some(wm) = watermark {
            let committed = self
                .catalog
                .read_watermark("spindle_watermark_id")?
                .unwrap_or(0);
            let next_wm = admission_kernel::telemetry::watermark_advance(committed, wm, true);
            self.catalog
                .set_watermark("spindle_watermark_id", next_wm)?;
            let mut pending_wm = self.pending_spindle_watermark.write();
            if *pending_wm == Some(wm) {
                *pending_wm = None;
            }
        }
        Ok(total)
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
    validate_non_tmp_path(&config.data_dir).map_err(|e| e.to_string())?;
    validate_non_tmp_path(&config.tmp_dir).map_err(|e| e.to_string())?;
    if let Some(ref sock) = config.spindle_socket {
        validate_non_tmp_path(sock).map_err(|e| e.to_string())?;
    }
    if let Some(ref rt_dir) = config.runtime_dir {
        validate_non_tmp_path(rt_dir).map_err(|e| e.to_string())?;
    }

    let catalog = CatalogStore::open(&config.data_dir)?;
    let initial_spindle_watermark = catalog.read_watermark("spindle_watermark_id")?.unwrap_or(0);
    let layout = catalog.layout().clone();
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
        flush_lock: Mutex::new(()),
        ring: RwLock::new(HotMetricRing::with_capacity_bytes(config.ring_budget_bytes)),
        latest: RwLock::new(None),
        pending_logs: RwLock::new(Vec::new()),
        pending_traces: RwLock::new(Vec::new()),
        pending_fs_events: RwLock::new(Vec::new()),
        polled_spindle_cursor: AtomicU64::new(initial_spindle_watermark),
        pending_spindle_watermark: RwLock::new(None),
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
    let rpc_task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = rpc_listener.accept().await else {
                continue;
            };
            let rt = rpc_runtime.clone();
            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut lines = TokioBufReader::new(reader).lines();
                if let Ok(Some(line)) = lines.next_line().await {
                    let reply = match serde_json::from_str::<DaemonRequest>(line.trim()) {
                        Ok(req) => rt.handle_rpc(req),
                        Err(e) => DaemonResponse::Error {
                            message: e.to_string(),
                        },
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
                        let _ = writer.write_all(&bytes).await;
                        let _ = writer.shutdown().await;
                    }
                }
            });
        }
    });

    let ingest_host = read_hostname();
    let ingest_sock = config
        .runtime_dir
        .as_ref()
        .map_or_else(|| default_ingest_socket(&layout), |d| d.join("ingest.sock"));
    let ingest_sock_cleanup = ingest_sock.clone();
    let serve_host = ingest_host.clone();
    let ingest_task = tokio::spawn(async move {
        let _ = serve_ingest_socket(&ingest_sock, metrics_q, logs_q, traces_q, serve_host).await;
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

                // Drain bounded queues.
                let mut extra_metrics = Vec::new();
                while let Ok(m) = metrics_rx.try_recv() {
                    if extra_metrics.len() < 4096 {
                        extra_metrics.push(m);
                    } else {
                        dropped_m.fetch_add(1, Ordering::Relaxed);
                    }
                }
                {
                    let mut logs = runtime.pending_logs.write();
                    while let Ok(l) = logs_rx.try_recv() {
                        if logs.len() < 1024 {
                            logs.push(l);
                        } else {
                            dropped_l.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
                {
                    let mut traces = runtime.pending_traces.write();
                    while let Ok(t) = traces_rx.try_recv() {
                        if traces.len() < 1024 {
                            traces.push(t);
                        } else {
                            dropped_t.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }

                let mut spindle_backpressure = false;
                let mut newly_accepted_wm: Option<u64> = None;
                if let Some(ref spindle_sock) = config.spindle_socket
                    && spindle_sock.exists()
                {
                    let after_id = runtime.polled_spindle_cursor.load(Ordering::Acquire);
                    let sock = spindle_sock.clone();
                    let host = ingest_host.clone();
                    match tokio::task::spawn_blocking(move || {
                        poll_spindle_custody_records(&sock, after_id, 64, &host)
                    })
                    .await
                    {
                        Ok(Ok(records)) => {
                            *runtime.last_spindle_error.write() = None;
                            let mut accepted_cursor = after_id;
                            for (id, env) in records {
                                if enqueue_envelope(env, &mut extra_metrics, &runtime) {
                                    accepted_cursor = id;
                                } else {
                                    spindle_backpressure = true;
                                    break;
                                }
                            }
                            if accepted_cursor > after_id {
                                runtime
                                    .polled_spindle_cursor
                                    .store(accepted_cursor, Ordering::Release);
                                newly_accepted_wm = Some(accepted_cursor);
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

                let drops = dropped_m.load(Ordering::Relaxed)
                    + dropped_l.load(Ordering::Relaxed)
                    + dropped_t.load(Ordering::Relaxed)
                    + overflow_dropped.load(Ordering::Relaxed);
                total_dropped.store(drops, Ordering::Relaxed);

                let mut state = run_collect_cycle(&runtime.guard, &mut fs_tracker, &mut vol_detector, &mut sketch);
                state.metrics.extend(extra_metrics);

                runtime.ring.write().push_batch(&state.metrics);
                if let Some(wm) = newly_accepted_wm {
                    *runtime.pending_spindle_watermark.write() = Some(wm);
                }
                {
                    let mut fsev = runtime.pending_fs_events.write();
                    if fsev.len() < 2048 {
                        fsev.extend(state.fs_events.iter().cloned());
                    }
                }

                let _ = runtime.catalog.commit_cycle(&state.snapshot, &state.drift, &state.graph);
                *runtime.latest.write() = Some(state);

                // Self-shedding check, Spindle backpressure, or 60s flush cadence.
                let force_shed = runtime.guard.is_shedding();
                if tick_count == 1
                    || force_shed
                    || spindle_backpressure
                    || last_flush.elapsed() >= Duration::from_secs(60)
                {
                    if let Err(err) = runtime.flush_to_olap() {
                        *runtime.last_spindle_error.write() = Some(format!("olap flush failed: {err}"));
                    }
                    let _ = runtime.janitor.run_pass(None);
                    last_flush = std::time::Instant::now();
                }

                if let Some(limit) = config.max_ticks && tick_count >= limit {
                    break;
                }
            }
        }
    }

    let final_flush = runtime.flush_to_olap();
    let _ = runtime.janitor.run_pass(None);
    rpc_task.abort();
    ingest_task.abort();
    let _ = fs::remove_file(&daemon_sock);
    let _ = fs::remove_file(&ingest_sock_cleanup);
    final_flush.map(|_| ())
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
            flush_lock: Mutex::new(()),
            ring: RwLock::new(HotMetricRing::with_capacity_bytes(64 * 1024)),
            latest: RwLock::new(None),
            pending_logs: RwLock::new(Vec::new()),
            pending_traces: RwLock::new(Vec::new()),
            pending_fs_events: RwLock::new(Vec::new()),
            polled_spindle_cursor: AtomicU64::new(5),
            pending_spindle_watermark: RwLock::new(Some(5)),
            last_spindle_error: RwLock::new(None),
            overflow_dropped: Arc::new(AtomicU64::new(0)),
            guard,
        };

        let mut extra_metrics = Vec::new();
        assert!(enqueue_envelope(
            IngestEnvelope::Metric(MetricPoint {
                timestamp_ms: 0,
                name: "test.metric".into(),
                value: 7.0,
                host: "h".into(),
                slice: "spindle".into(),
                unit: "svc".into(),
                pid: None,
                labels_json: r#"{"k":"v"}"#.into(),
            }),
            &mut extra_metrics,
            &runtime,
        ));
        assert!(extra_metrics[0].timestamp_ms > 0);
        runtime.ring.write().push_batch(&extra_metrics);

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
        assert_eq!(*runtime.pending_spindle_watermark.read(), Some(5));

        // Remove obstruction and flush again; watermark must advance only now.
        fs::remove_file(&metrics_dir).expect("remove obstructing file");
        fs::create_dir_all(&metrics_dir).expect("recreate metrics dir");
        assert_eq!(runtime.flush_to_olap().expect("retry flush cleanly"), 1);
        assert_eq!(
            runtime
                .catalog
                .read_watermark("spindle_watermark_id")
                .expect("read watermark after successful flush"),
            Some(5)
        );
        assert_eq!(*runtime.pending_spindle_watermark.read(), None);
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
}
