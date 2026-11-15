//! Converged telemetry daemon, one-shot collector, and `daemon.sock` RPC responder.

use crate::cgroups::walk_cgroups_and_processes;
use crate::channel::BoundedTelemetryQueue;
use crate::fs_watch::FsChurnTracker;
use crate::host::{
    read_gpu_stats, read_hostname, read_memory_snapshot, read_mount_usages, read_psi_metrics,
};
use crate::self_guard::{SelfPreservationGuard, build_doctor_report_from_summary};
use crate::spindle_bridge::serve_ingest_socket;
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
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
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
    HotMetrics(Vec<MetricPoint>),
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

fn runtime_socket_path(layout: &StorageLayout, name: &str) -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
        let dir = PathBuf::from(xdg).join("fabrico11y");
        if validate_non_tmp_path(&dir).is_ok() {
            return dir.join(name);
        }
    }
    layout.state_dir().join(name)
}

/// Queries the resident `fabric daemon` if `daemon.sock` is listening.
#[must_use]
pub fn query_running_daemon(layout: &StorageLayout, req: &DaemonRequest) -> Option<DaemonResponse> {
    let sock_path = default_daemon_socket(layout);
    if !sock_path.exists() {
        return None;
    }
    let mut stream = StdUnixStream::connect(&sock_path).ok()?;
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
    ring: RwLock<HotMetricRing>,
    latest: RwLock<Option<CollectedState>>,
    pending_logs: RwLock<Vec<LogRecord>>,
    pending_traces: RwLock<Vec<TraceSpanRecord>>,
    pending_fs_events: RwLock<Vec<FsEventRecord>>,
    guard: SelfPreservationGuard,
}

impl DaemonRuntime {
    fn flush_to_olap(&self) -> Result<usize, String> {
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
        let _ = self.olap.write_metrics(&metrics)?;
        let _ = self.olap.write_logs(&logs)?;
        let _ = self.olap.write_traces(&traces)?;
        let _ = self.olap.write_fs_events(&fs_events)?;
        let _ = self.olap.write_edges(&edges)?;
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
            DaemonRequest::HotMetrics { since_ms } => {
                DaemonResponse::HotMetrics(self.ring.read().query_since(since_ms))
            }
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

    let catalog = CatalogStore::open(&config.data_dir)?;
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
        ring: RwLock::new(HotMetricRing::with_capacity_bytes(config.ring_budget_bytes)),
        latest: RwLock::new(None),
        pending_logs: RwLock::new(Vec::new()),
        pending_traces: RwLock::new(Vec::new()),
        pending_fs_events: RwLock::new(Vec::new()),
        guard,
    });

    let daemon_sock = default_daemon_socket(&layout);
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
                    if let Ok(mut bytes) = serde_json::to_vec(&reply) {
                        bytes.push(b'\n');
                        let _ = writer.write_all(&bytes).await;
                    }
                }
            });
        }
    });

    let ingest_sock = default_ingest_socket(&layout);
    let ingest_task = tokio::spawn(async move {
        let _ = serve_ingest_socket(&ingest_sock, metrics_q, logs_q, traces_q).await;
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

                let drops = dropped_m.load(std::sync::atomic::Ordering::Relaxed)
                    + dropped_l.load(std::sync::atomic::Ordering::Relaxed)
                    + dropped_t.load(std::sync::atomic::Ordering::Relaxed);
                total_dropped.store(drops, std::sync::atomic::Ordering::Relaxed);

                // Drain bounded queues.
                let mut extra_metrics = Vec::new();
                while let Ok(m) = metrics_rx.try_recv() {
                    extra_metrics.push(m);
                }
                {
                    let mut logs = runtime.pending_logs.write();
                    while let Ok(l) = logs_rx.try_recv() {
                        if logs.len() < 1024 {
                            logs.push(l);
                        }
                    }
                }
                {
                    let mut traces = runtime.pending_traces.write();
                    while let Ok(t) = traces_rx.try_recv() {
                        if traces.len() < 1024 {
                            traces.push(t);
                        }
                    }
                }

                let mut state = run_collect_cycle(&runtime.guard, &mut fs_tracker, &mut vol_detector, &mut sketch);
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

                // Self-shedding check or 60s flush cadence.
                let force_shed = runtime.guard.is_shedding();
                if tick_count == 1 || force_shed || last_flush.elapsed() >= Duration::from_secs(60) {
                    let _ = runtime.flush_to_olap();
                    let _ = runtime.janitor.run_pass(None);
                    last_flush = std::time::Instant::now();
                }

                if let Some(limit) = config.max_ticks && tick_count >= limit {
                    break;
                }
            }
        }
    }

    let _ = runtime.flush_to_olap();
    let _ = runtime.janitor.run_pass(None);
    rpc_task.abort();
    ingest_task.abort();
    let _ = fs::remove_file(&daemon_sock);
    let _ = fs::remove_file(default_ingest_socket(&layout));
    Ok(())
}
