//! `fabric` — Deterministic CLI sensory plane for FabricO11y host, agent, and OTLP telemetry.

use arrow::array::{BooleanArray, StringArray, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use fabric::canonical::{self, Request as CanonicalRequest, Response as CanonicalResponse};
use fabric::spindle::{Reply as SpindleReply, Request as SpindleRequest, transport};
use fabric_collect::{
    CollectedState, DaemonConfig, DaemonRequest, DaemonResponse, IngestEnvelope,
    SelfPreservationGuard, collect_once, configured_host_export_node, default_spindle_socket,
    is_forwarded_host_signal, poll_spindle_projection_records, query_running_daemon, run_daemon,
    run_spindle_only_daemon,
};
use fabric_core::arrow_schemas::{
    blame_to_record_batch, doctor_to_record_batch, drift_to_record_batch,
    encode_batches_to_ipc_stream, snapshot_to_record_batch,
};
use fabric_core::{
    BlameReport, BlameResource, CliEnvelope, DEFAULT_DATA_DIR, DEFAULT_TMP_DIR, DoctorReport,
    DriftReport, HostSnapshot, PillarKind, StorageLayout,
};
use fabric_graph::{Direction, LineageSubgraph, lineage_to_record_batch};
use fabric_query::unified::{
    QueryDomain, UnifiedQuerySpec, execute_unified_query, parse_duration_ms,
};
use fabric_store::{CatalogStore, validate_non_tmp_path_on_disk, validated_storage_layout};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

mod projection_ready;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Json,
    Arrow,
    Table,
}

#[derive(Debug, Parser)]
#[command(
    name = "fabric",
    about = "Converged Linux host, autonomous agent, and OTLP observability CLI",
    version
)]
struct Cli {
    /// Non-tmpfs directory for redb OLTP state and ZSTD Parquet partitions.
    #[arg(long, env = "FABRIC_DATA_DIR", default_value = DEFAULT_DATA_DIR, global = true)]
    data_dir: PathBuf,

    /// Non-tmpfs temporary scratch directory.
    #[arg(long, env = "FABRIC_TMP_DIR", global = true)]
    tmp_dir: Option<PathBuf>,

    /// Emit structured versioned JSON (default when stdout is not a TTY).
    #[arg(long, global = true, conflicts_with_all = ["arrow", "table"])]
    json: bool,

    /// Emit raw Arrow IPC stream bytes to stdout.
    #[arg(long, global = true, conflicts_with_all = ["json", "table"])]
    arrow: bool,

    /// Emit compact aligned terminal tables (default on TTY).
    #[arg(long, global = true, conflicts_with_all = ["json", "arrow"])]
    table: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Capture or inspect the latest live host, slice, cgroup, unit, and storage snapshot.
    Snapshot,
    /// Query historical ZSTD Parquet + hot RAM ring/live state via DataFusion.
    Query(QueryArgs),
    /// Traverse causal upstream/downstream lineage around a path, unit, slice, PID, or agent.
    Lineage(LineageArgs),
    /// Rank contributors driving pressure on swap, memory, tmpfs, disk:<mount>, or cpu.
    Blame(BlameArgs),
    /// Evaluate the Five Data Observability Pillars (freshness, volume, schema, distribution, lineage).
    Drift(DriftArgs),
    /// Verify self-preservation invariants (RSS < 100 MiB, open FDs < 64, zero /tmp FDs, slice).
    Doctor,
    /// Inspect unified Spindle custody store, Server canonical frontiers, and collector watermark.
    Custody(CustodyArgs),
    /// Check whether an exact Spindle receipt is queryable from committed projection.
    ProjectionReady(projection_ready::Args),
    /// Run the converged telemetry daemon (inside fabrico11y.slice).
    Daemon(DaemonArgs),
}

#[derive(Debug, Args)]
struct QueryArgs {
    /// Domain: metrics, logs, traces, units, fs, or agents.
    domain: String,
    /// Lookback window relative to now (e.g., 5m, 1h, 7d).
    #[arg(long)]
    since: Option<String>,
    /// Filter predicates (e.g., slice=agent.slice, value>100, path~synth).
    #[arg(long = "where")]
    where_clauses: Vec<String>,
    /// Group-by columns separated by commas.
    #[arg(long, value_delimiter = ',')]
    group_by: Vec<String>,
    /// Aggregations separated by commas (count, sum:col, avg:col, min:col, max:col).
    #[arg(long, value_delimiter = ',')]
    agg: Vec<String>,
    /// Maximum rows to return.
    #[arg(long, default_value_t = 200)]
    limit: usize,
}

#[derive(Debug, Args)]
struct LineageArgs {
    /// Target identifier, filesystem path, unit name, slice, PID, or agent name.
    target: String,
    /// Traversal direction: upstream, downstream, or both.
    #[arg(long, default_value = "both")]
    direction: String,
    /// Maximum BFS depth.
    #[arg(long, default_value_t = 4)]
    depth: usize,
}

#[derive(Debug, Args)]
struct BlameArgs {
    /// Contended resource: swap, memory, tmpfs, cpu, or disk:<mount> (e.g. disk:/data).
    resource: String,
}

#[derive(Debug, Args)]
struct DriftArgs {
    /// Filter by pillar: freshness, volume, schema, distribution, or lineage.
    #[arg(long)]
    pillar: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct CustodyArgs {
    /// Path to Spindle Unix socket (defaults to spindle.json or XDG_RUNTIME_DIR/spindle/spindle.sock).
    #[arg(long, env = "SPINDLE_SOCKET")]
    spindle_socket: Option<PathBuf>,
    /// HTTP endpoint of fabric-server (defaults to spindle.json forwarding.endpoint or server.json listen).
    #[arg(long, env = "FABRIC_SERVER_ENDPOINT")]
    server_endpoint: Option<String>,
    /// Reader credential file used to query canonical progress from fabric-server.
    #[arg(long, env = "FABRIC_READER_CREDENTIAL_FILE")]
    reader_credential_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct DaemonArgs {
    /// Collection tick interval in milliseconds.
    #[arg(long, default_value_t = 2000)]
    interval_ms: u64,
    /// Run a single collection & persistence cycle and exit.
    #[arg(long)]
    once: bool,
    /// Project from a dedicated Spindle socket without collecting host or ingest data.
    #[arg(long, requires = "spindle_socket", conflicts_with = "once")]
    spindle_only: bool,
    /// Stop automatically after N ticks.
    #[arg(long)]
    max_ticks: Option<u64>,
    /// Optional Spindle Unix socket path to poll alongside host collection.
    #[arg(long, env = "SPINDLE_SOCKET")]
    spindle_socket: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct QueryJsonPayload {
    domain: String,
    row_count: usize,
    rows: Vec<serde_json::Value>,
}

#[tokio::main(worker_threads = 4)]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("fabric: {err}");
        std::process::exit(1);
    }
}

fn resolve_cli_tmp_dir(explicit: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(dir) = explicit {
        return validate_non_tmp_path_on_disk(dir).map_err(|e| e.to_string());
    }
    if let Some(raw) = std::env::var_os("TMPDIR")
        && !raw.is_empty()
        && let Ok(valid) = validate_non_tmp_path_on_disk(Path::new(&raw))
    {
        return Ok(valid);
    }
    validate_non_tmp_path_on_disk(Path::new(DEFAULT_TMP_DIR)).map_err(|e| e.to_string())
}

async fn run() -> Result<(), String> {
    let cli = Cli::parse();
    if let Commands::ProjectionReady(args) = cli.command {
        std::process::exit(projection_ready::run(
            args,
            cli.data_dir,
            cli.arrow || cli.table,
        ));
    }
    let format = if cli.json {
        OutputFormat::Json
    } else if cli.arrow {
        OutputFormat::Arrow
    } else if cli.table || std::io::stdout().is_terminal() {
        OutputFormat::Table
    } else {
        OutputFormat::Json
    };

    let tmp_dir = resolve_cli_tmp_dir(cli.tmp_dir.as_deref())?;
    let layout = validated_storage_layout(&cli.data_dir)?;

    match &cli.command {
        Commands::ProjectionReady(_) => unreachable!(),
        Commands::Snapshot => {
            let snap = fetch_snapshot(&layout, &tmp_dir)?;
            match format {
                OutputFormat::Json => emit_json("snapshot", &snap),
                OutputFormat::Arrow => {
                    let batch = snapshot_to_record_batch(&snap)?;
                    let bytes = encode_batches_to_ipc_stream(&batch.schema(), &[batch])?;
                    write_stdout_bytes(&bytes)
                }
                OutputFormat::Table => {
                    print_snapshot_table(&snap);
                    Ok(())
                }
            }
        }
        Commands::Query(args) => {
            let domain = args.domain.parse::<QueryDomain>()?;
            let since_ms = match &args.since {
                Some(raw) => Some(
                    Utc::now()
                        .timestamp_millis()
                        .saturating_sub(parse_duration_ms(raw)?),
                ),
                None => None,
            };
            let spec = UnifiedQuerySpec {
                domain,
                since_ms,
                where_clauses: args.where_clauses.clone(),
                group_by: args.group_by.clone(),
                agg_specs: args.agg.clone(),
                limit: args.limit,
            };
            let (live_metrics, live_snap) = fetch_query_live_context(&layout, &tmp_dir, &spec)?;
            let res =
                execute_unified_query(layout.data_dir(), &spec, &live_metrics, live_snap.as_ref())
                    .await?;
            match format {
                OutputFormat::Json => {
                    let payload = QueryJsonPayload {
                        domain: res.domain,
                        row_count: res.row_count,
                        rows: res.rows,
                    };
                    emit_json("query", &payload)
                }
                OutputFormat::Arrow => {
                    let bytes = encode_batches_to_ipc_stream(&res.schema, &res.batches)?;
                    write_stdout_bytes(&bytes)
                }
                OutputFormat::Table => {
                    print_query_table(&res.domain, &res.rows);
                    Ok(())
                }
            }
        }
        Commands::Lineage(args) => {
            let dir = args.direction.parse::<Direction>()?;
            let sub = fetch_lineage(&layout, &tmp_dir, &args.target, dir, args.depth)?;
            match format {
                OutputFormat::Json => emit_json("lineage", &sub),
                OutputFormat::Arrow => {
                    let batch = lineage_to_record_batch(&sub)?;
                    let bytes = encode_batches_to_ipc_stream(&batch.schema(), &[batch])?;
                    write_stdout_bytes(&bytes)
                }
                OutputFormat::Table => {
                    print_lineage_table(&sub);
                    Ok(())
                }
            }
        }
        Commands::Blame(args) => {
            let res = args.resource.parse::<BlameResource>()?;
            let report = fetch_blame(&layout, &tmp_dir, &res)?;
            match format {
                OutputFormat::Json => emit_json("blame", &report),
                OutputFormat::Arrow => {
                    let batch = blame_to_record_batch(&report)?;
                    let bytes = encode_batches_to_ipc_stream(&batch.schema(), &[batch])?;
                    write_stdout_bytes(&bytes)
                }
                OutputFormat::Table => {
                    print_blame_table(&report);
                    Ok(())
                }
            }
        }
        Commands::Drift(args) => {
            let pillar = match &args.pillar {
                Some(p) => Some(p.parse::<PillarKind>()?),
                None => None,
            };
            let report = fetch_drift(&layout, &tmp_dir, pillar)?;
            match format {
                OutputFormat::Json => emit_json("drift", &report),
                OutputFormat::Arrow => {
                    let batch = drift_to_record_batch(&report)?;
                    let bytes = encode_batches_to_ipc_stream(&batch.schema(), &[batch])?;
                    write_stdout_bytes(&bytes)
                }
                OutputFormat::Table => {
                    print_drift_table(&report);
                    Ok(())
                }
            }
        }
        Commands::Doctor => {
            let report = fetch_doctor(&layout, &tmp_dir);
            match format {
                OutputFormat::Json => emit_json("doctor", &report),
                OutputFormat::Arrow => {
                    let batch = doctor_to_record_batch(&report)?;
                    let bytes = encode_batches_to_ipc_stream(&batch.schema(), &[batch])?;
                    write_stdout_bytes(&bytes)
                }
                OutputFormat::Table => {
                    print_doctor_table(&report);
                    Ok(())
                }
            }
        }
        Commands::Custody(args) => {
            let layout_for_task = layout.clone();
            let args_cloned = args.clone();
            let report = tokio::task::spawn_blocking(move || {
                fetch_custody_status(&layout_for_task, &args_cloned)
            })
            .await
            .map_err(|e| e.to_string())??;
            match format {
                OutputFormat::Json => emit_json("custody", &report),
                OutputFormat::Arrow => {
                    let batch = custody_to_record_batch(&report)?;
                    let bytes = encode_batches_to_ipc_stream(&batch.schema(), &[batch])?;
                    write_stdout_bytes(&bytes)
                }
                OutputFormat::Table => {
                    print_custody_table(&report);
                    Ok(())
                }
            }
        }
        Commands::Daemon(args) => {
            let cli_spindle_sock = non_empty_path(args.spindle_socket.clone());
            if args.spindle_only && cli_spindle_sock.is_none() {
                return Err("Spindle-only projection requires a nonempty Spindle socket".into());
            }
            if let Some(ref sock) = cli_spindle_sock {
                validate_non_tmp_path_on_disk(sock).map_err(|e| e.to_string())?;
            }
            if args.once {
                let export_node = configured_host_export_node()?;
                let catalog = CatalogStore::open(layout.data_dir())?;
                let _writer_lock = fabric_store::try_acquire_writer_lock(catalog.layout())?;
                let state = collect_once(catalog.layout().data_dir(), &tmp_dir)?;
                let olap = fabric_store::OlapParquetStore::new(catalog.layout().clone());
                let spindle_sock = cli_spindle_sock.or_else(default_spindle_socket);
                let mut spindle_range = None;
                let mut spindle_error = None;
                let mut spindle_metrics = Vec::new();
                let mut extra_logs = Vec::new();
                let mut extra_traces = Vec::new();
                if let Some(ref sock) = spindle_sock {
                    validate_non_tmp_path_on_disk(sock).map_err(|e| e.to_string())?;
                    if sock.exists() {
                        let after_id = catalog.read_watermark("spindle_watermark_id")?.unwrap_or(0);
                        let sock_owned = sock.clone();
                        let host_owned = state.snapshot.hostname.clone();
                        let catalog_owned = catalog.clone();
                        let poll = tokio::task::spawn_blocking(move || {
                            poll_spindle_projection_records(
                                &sock_owned,
                                &catalog_owned,
                                after_id,
                                64,
                                &host_owned,
                            )
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|result| result);
                        let records = match poll {
                            Ok(poll) => {
                                spindle_error = poll.tail_error;
                                poll.records
                            }
                            Err(error) => {
                                spindle_error = Some(error);
                                Vec::new()
                            }
                        };
                        let now_ms = Utc::now().timestamp_millis();
                        for (provenance, env) in records {
                            fabric_collect::SpindleProjectionRange::include(
                                &mut spindle_range,
                                &provenance,
                            )?;
                            if is_forwarded_host_signal(&provenance, &env, export_node.as_deref()) {
                                continue;
                            }
                            match env {
                                IngestEnvelope::Metric(mut pt) => {
                                    if pt.timestamp_ms <= 0 {
                                        pt.timestamp_ms = now_ms;
                                    }
                                    spindle_metrics.push((Some(provenance), pt));
                                }
                                IngestEnvelope::Log(mut log) => {
                                    if log.timestamp_ms <= 0 {
                                        log.timestamp_ms = now_ms;
                                    }
                                    extra_logs.push((Some(provenance), log));
                                }
                                IngestEnvelope::Trace(mut span) => {
                                    if span.timestamp_ms <= 0 {
                                        span.timestamp_ms = now_ms;
                                    }
                                    extra_traces.push((Some(provenance), span));
                                }
                            }
                        }
                    }
                }
                catalog.commit_cycle(&state.snapshot, &state.drift, &state.graph)?;
                let edges = state
                    .graph
                    .to_edge_records(state.snapshot.timestamp_unix_ms);
                let mut volatile_plan =
                    olap.plan_flush(&state.metrics, &[], &[], &state.fs_events, &edges)?;
                olap.execute_flush_plan(&mut volatile_plan)?;
                let committed_wm = if let Some(range) = spindle_range {
                    let mut spindle_plan = olap
                        .plan_spindle_flush(&spindle_metrics, &extra_logs, &extra_traces)?
                        .with_spindle_watermark(Some(range.last_id()));
                    olap.execute_flush_plan(&mut spindle_plan)?;
                    Some(range.commit_published(&catalog)?)
                } else {
                    None
                };
                fabric_store::clear_committed_uncommitted(
                    catalog.layout(),
                    committed_wm,
                    &std::collections::BTreeSet::new(),
                )?;
                if let Some(error) = spindle_error {
                    return Err(error);
                }
                return emit_json("daemon_once", &state.snapshot.self_health);
            }
            let mut daemon_config = DaemonConfig {
                data_dir: layout.data_dir().to_path_buf(),
                tmp_dir,
                interval_ms: args.interval_ms,
                max_ticks: args.max_ticks,
                ..Default::default()
            };
            if let Some(spindle_sock) = cli_spindle_sock {
                daemon_config.spindle_socket = Some(spindle_sock);
            }
            if args.spindle_only {
                run_spindle_only_daemon(daemon_config).await
            } else {
                run_daemon(daemon_config).await
            }
        }
    }
}

fn fetch_state_or_collect(
    layout: &StorageLayout,
    tmp_dir: &Path,
) -> Result<CollectedState, String> {
    if let Ok(catalog) = CatalogStore::open(layout.data_dir())
        && let Ok(Some(live)) = catalog.load_live_state()
        && (Utc::now().timestamp_millis() - live.updated_at_unix_ms) <= 5_000
    {
        return Ok(CollectedState {
            snapshot: live.snapshot,
            drift: live.drift,
            graph: live.graph,
            metrics: Vec::new(),
            fs_events: Vec::new(),
        });
    }
    collect_once(layout.data_dir(), tmp_dir)
}

fn fetch_snapshot(layout: &StorageLayout, tmp_dir: &Path) -> Result<HostSnapshot, String> {
    if let Some(DaemonResponse::Snapshot(snap)) =
        query_running_daemon(layout, &DaemonRequest::Snapshot)
    {
        return Ok(*snap);
    }
    Ok(fetch_state_or_collect(layout, tmp_dir)?.snapshot)
}

fn fetch_lineage(
    layout: &StorageLayout,
    tmp_dir: &Path,
    target: &str,
    direction: Direction,
    depth: usize,
) -> Result<LineageSubgraph, String> {
    let dir_str = match direction {
        Direction::Upstream => "upstream",
        Direction::Downstream => "downstream",
        Direction::Both => "both",
    };
    if let Some(DaemonResponse::Lineage(sub)) = query_running_daemon(
        layout,
        &DaemonRequest::Lineage {
            target: target.to_owned(),
            direction: dir_str.to_owned(),
            max_depth: depth,
        },
    ) {
        return Ok(sub);
    }
    let state = fetch_state_or_collect(layout, tmp_dir)?;
    Ok(state.graph.lineage(target, direction, depth))
}

fn fetch_blame(
    layout: &StorageLayout,
    tmp_dir: &Path,
    resource: &BlameResource,
) -> Result<BlameReport, String> {
    if let Some(DaemonResponse::Blame(report)) = query_running_daemon(
        layout,
        &DaemonRequest::Blame {
            resource: resource.to_string(),
        },
    ) {
        return Ok(report);
    }
    let state = fetch_state_or_collect(layout, tmp_dir)?;
    Ok(state.graph.blame(resource, &state.snapshot))
}

fn fetch_drift(
    layout: &StorageLayout,
    tmp_dir: &Path,
    pillar: Option<PillarKind>,
) -> Result<DriftReport, String> {
    if let Some(DaemonResponse::Drift(report)) = query_running_daemon(
        layout,
        &DaemonRequest::Drift {
            pillar: pillar.map(|p| p.to_string()),
        },
    ) {
        return Ok(report);
    }
    let state = fetch_state_or_collect(layout, tmp_dir)?;
    let mut drift = state.drift;
    if let Some(p) = pillar {
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
    Ok(drift)
}

fn fetch_doctor(layout: &StorageLayout, tmp_dir: &Path) -> DoctorReport {
    if let Some(DaemonResponse::Doctor(report)) =
        query_running_daemon(layout, &DaemonRequest::Doctor)
    {
        return report;
    }
    let guard = SelfPreservationGuard::new(
        layout.data_dir().to_string_lossy().into_owned(),
        tmp_dir.to_string_lossy().into_owned(),
        Arc::new(AtomicU64::new(0)),
    );
    guard.doctor_report(false)
}

fn fetch_query_live_context(
    layout: &StorageLayout,
    tmp_dir: &Path,
    spec: &UnifiedQuerySpec,
) -> Result<(Vec<fabric_core::MetricPoint>, Option<HostSnapshot>), String> {
    let live_metrics = if spec.domain == QueryDomain::Metrics {
        match query_running_daemon(
            layout,
            &DaemonRequest::HotMetrics {
                since_ms: spec.since_ms.unwrap_or(0),
            },
        ) {
            Some(DaemonResponse::HotMetrics { points }) => points,
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };
    let live_snapshot = if matches!(
        spec.domain,
        QueryDomain::Units | QueryDomain::Agents | QueryDomain::Fs
    ) {
        Some(fetch_snapshot(layout, tmp_dir)?)
    } else {
        None
    };
    Ok((live_metrics, live_snapshot))
}

fn emit_json<T: Serialize>(command: &str, payload: &T) -> Result<(), String> {
    let envelope = CliEnvelope::new(Utc::now().timestamp_millis(), command, payload);
    let out = serde_json::to_string_pretty(&envelope).map_err(|e| e.to_string())?;
    println!("{out}");
    Ok(())
}

fn write_stdout_bytes(bytes: &[u8]) -> Result<(), String> {
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(bytes).map_err(|e| e.to_string())?;
    stdout.flush().map_err(|e| e.to_string())
}

fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", (bytes as f64) / (1024.0 * 1024.0))
}

fn print_snapshot_table(s: &HostSnapshot) {
    println!(
        "HOST {} | mem_avail={} | swap_used={} | psi(mem={:.2}, io={:.2}) | self_rss={}",
        s.hostname,
        mib(s.memory.mem_available_bytes),
        mib(s.memory.swap_used_bytes),
        s.psi.memory_some.avg10,
        s.psi.io_some.avg10,
        mib(s.self_health.rss_bytes),
    );
    println!(
        "{:<20} {:>12} {:>12} {:>8}",
        "SLICE", "MEM_CUR", "MEM_HIGH", "PIDS"
    );
    for sl in &s.slices {
        let high = sl.memory_high_bytes.map_or_else(|| "max".into(), mib);
        println!(
            "{:<20} {:>12} {:>12} {:>8}",
            sl.slice_name,
            mib(sl.memory_current_bytes),
            high,
            sl.pids_current
        );
    }
    println!();
    println!(
        "{:<12} {:<10} {:>12} {:>12} {:>8}",
        "MOUNT", "FSTYPE", "USED", "AVAIL", "USED%"
    );
    for m in &s.mounts {
        println!(
            "{:<12} {:<10} {:>12} {:>12} {:>7.1}%",
            m.mount_point,
            m.fs_type,
            mib(m.used_bytes),
            mib(m.available_bytes),
            m.used_pct
        );
    }
}

fn print_query_table(domain: &str, rows: &[serde_json::Value]) {
    println!("QUERY domain={domain} rows={}", rows.len());
    let Some(first) = rows.first().and_then(|v| v.as_object()) else {
        return;
    };
    let cols: Vec<&String> = first.keys().take(7).collect();
    for c in &cols {
        print!("{:<20} ", truncate(c, 20));
    }
    println!();
    for row in rows.iter().take(50) {
        if let Some(obj) = row.as_object() {
            for c in &cols {
                let s = match obj.get(*c) {
                    Some(serde_json::Value::String(v)) => v.clone(),
                    Some(v) => v.to_string(),
                    None => String::new(),
                };
                print!("{:<20} ", truncate(&s, 20));
            }
            println!();
        }
    }
}

fn print_lineage_table(sub: &LineageSubgraph) {
    println!(
        "LINEAGE target={} matched={:?} nodes={} edges={}",
        sub.target,
        sub.matched_roots,
        sub.nodes.len(),
        sub.edges.len()
    );
    println!("{:<40} {:<16} {:<30}", "NODE_ID", "KIND", "LABEL");
    for n in &sub.nodes {
        println!(
            "{:<40} {:<16} {:<30}",
            truncate(&n.id, 40),
            n.kind,
            truncate(&n.label, 30)
        );
    }
    for e in &sub.edges {
        println!("  {} --[{}]--> {}", e.src, e.kind, e.dst);
    }
}

fn print_blame_table(report: &BlameReport) {
    println!(
        "BLAME resource={} | {}",
        report.resource, report.pressure_summary
    );
    println!(
        "{:<4} {:<26} {:>12} {:>7} {:<14} {:<28}",
        "RANK", "LABEL", "VALUE", "SHARE", "SLICE", "UNIT/PATH"
    );
    for e in &report.entries {
        let val = if e.unit_label == "bytes" {
            mib(e.contribution_value)
        } else {
            format!("{} {}", e.contribution_value, e.unit_label)
        };
        let extra = e
            .target_path
            .as_deref()
            .or(e.unit.as_deref())
            .unwrap_or("-");
        println!(
            "{:<4} {:<26} {:>12} {:>6.1}% {:<14} {:<28}",
            e.rank,
            truncate(&e.label, 26),
            val,
            e.share_pct,
            truncate(&e.slice, 14),
            truncate(extra, 28)
        );
    }
}

fn print_drift_table(report: &DriftReport) {
    println!(
        "DRIFT findings={} critical={} warn={} info={}",
        report.total_findings, report.critical_count, report.warn_count, report.info_count
    );
    println!(
        "{:<14} {:<10} {:<28} SUMMARY",
        "PILLAR", "SEVERITY", "TARGET"
    );
    for f in &report.findings {
        println!(
            "{:<14} {:<10} {:<28} {}",
            f.pillar,
            f.severity,
            truncate(&f.target, 28),
            f.summary
        );
    }
}

fn print_doctor_table(report: &DoctorReport) {
    println!(
        "DOCTOR healthy={} daemon_running={} pid={} rss={} fds={} tmp_fds={} slice={}",
        report.healthy,
        report.daemon_running,
        report.pid,
        mib(report.rss_bytes),
        report.open_fds,
        report.tmp_files_created,
        report.cgroup_path
    );
    println!("{:<22} {:<8} {:<32} THRESHOLD", "CHECK", "STATUS", "ACTUAL");
    for c in &report.checks {
        println!(
            "{:<22} {:<8} {:<32} {}",
            c.name,
            if c.passed { "PASS" } else { "FAIL" },
            truncate(&c.actual, 32),
            c.threshold
        );
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else if max > 3 {
        let mut out: String = s.chars().take(max - 3).collect();
        out.push_str("...");
        out
    } else {
        s.chars().take(max).collect()
    }
}

#[derive(Debug, Deserialize, Default)]
struct SpindleFileConfigHint {
    socket: Option<PathBuf>,
    forwarding: Option<SpindleForwardHint>,
}

#[derive(Debug, Deserialize, Default)]
struct SpindleForwardHint {
    endpoint: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ServerFileConfigHint {
    listen: Option<String>,
    reader_credential_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
struct CustodyStatusReport {
    spindle_socket: Option<String>,
    spindle_online: bool,
    spindle_origin: Option<String>,
    spindle_error: Option<String>,
    records: u64,
    highest_record_id: u64,
    canonical_through: u64,
    reclaimed_through: u64,
    forwarding_error: Option<String>,
    server_endpoint: Option<String>,
    server_online: bool,
    server_authority: Option<String>,
    server_error: Option<String>,
    server_frontiers: BTreeMap<String, u64>,
    collector_spindle_watermark_id: Option<u64>,
    catalog_error: Option<String>,
}

fn non_empty_path(path: Option<PathBuf>) -> Option<PathBuf> {
    path.filter(|p| !p.as_os_str().is_empty())
}

fn non_empty_str(raw: Option<String>) -> Option<String> {
    raw.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

fn fabrico11y_config_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("fabrico11y"));
    }
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".config").join("fabrico11y"))
}

fn read_optional_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> Result<T, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str::<T>(&raw)
            .map_err(|e| format!("invalid JSON in {}: {e}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(err) => Err(format!("failed reading {}: {err}", path.display())),
    }
}

fn normalize_canonical_endpoint(raw: String) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.starts_with("https://") {
        return Err(format!(
            "canonical exchange speaks plain HTTP; https endpoint is unsupported: {trimmed}"
        ));
    }
    let stripped = trimmed
        .strip_prefix("http://")
        .unwrap_or(trimmed)
        .trim_end_matches('/');
    if stripped.is_empty() {
        return Err("empty fabric-server endpoint".into());
    }
    Ok(stripped.to_owned())
}

fn fetch_custody_status(
    layout: &StorageLayout,
    args: &CustodyArgs,
) -> Result<CustodyStatusReport, String> {
    let config_dir = fabrico11y_config_dir();
    let mut spindle_error: Option<String> = None;
    let spindle_hint = match config_dir.as_ref() {
        Some(d) => match read_optional_json::<SpindleFileConfigHint>(&d.join("spindle.json")) {
            Ok(h) => h,
            Err(err) => {
                spindle_error = Some(err);
                SpindleFileConfigHint::default()
            }
        },
        None => SpindleFileConfigHint::default(),
    };
    let mut server_error: Option<String> = None;
    let server_hint = match config_dir.as_ref() {
        Some(d) => match read_optional_json::<ServerFileConfigHint>(&d.join("server.json")) {
            Ok(h) => h,
            Err(err) => {
                server_error = Some(err);
                ServerFileConfigHint::default()
            }
        },
        None => ServerFileConfigHint::default(),
    };

    let spindle_sock = non_empty_path(args.spindle_socket.clone())
        .or_else(|| non_empty_path(spindle_hint.socket))
        .or_else(default_spindle_socket);
    if let Some(ref sock) = spindle_sock {
        validate_non_tmp_path_on_disk(sock).map_err(|e| e.to_string())?;
    }

    let raw_endpoint = non_empty_str(args.server_endpoint.clone())
        .or_else(|| non_empty_str(spindle_hint.forwarding.and_then(|f| f.endpoint)))
        .or_else(|| non_empty_str(server_hint.listen));
    let server_endpoint = raw_endpoint.map(normalize_canonical_endpoint).transpose()?;
    let cred_file = non_empty_path(args.reader_credential_file.clone())
        .or_else(|| non_empty_path(server_hint.reader_credential_file));

    let (collector_spindle_watermark_id, catalog_error) = if layout.data_dir().exists() {
        match CatalogStore::open(layout.data_dir()) {
            Ok(c) => match c.read_watermark("spindle_watermark_id") {
                Ok(wm) => (wm, None),
                Err(err) => (None, Some(err)),
            },
            Err(err) => (None, Some(err)),
        }
    } else {
        (
            None,
            Some(format!(
                "data directory does not exist: {}",
                layout.data_dir().display()
            )),
        )
    };

    let mut report = CustodyStatusReport {
        spindle_socket: spindle_sock.as_ref().map(|p| p.display().to_string()),
        spindle_online: false,
        spindle_origin: None,
        spindle_error,
        records: 0,
        highest_record_id: 0,
        canonical_through: 0,
        reclaimed_through: 0,
        forwarding_error: None,
        server_endpoint: server_endpoint.clone(),
        server_online: false,
        server_authority: None,
        server_error,
        server_frontiers: BTreeMap::new(),
        collector_spindle_watermark_id,
        catalog_error,
    };

    if let Some(ref sock) = spindle_sock {
        if !sock.exists() {
            if report.spindle_error.is_none() {
                report.spindle_error = Some(format!("spindle socket missing: {}", sock.display()));
            }
        } else {
            match transport::request(
                sock,
                &SpindleRequest::Status,
                Instant::now() + Duration::from_millis(750),
            ) {
                Ok(SpindleReply::Status { status }) => {
                    report.spindle_online = true;
                    report.spindle_origin = Some(status.origin);
                    report.records = status.records;
                    report.highest_record_id = status.highest_record_id;
                    report.canonical_through = status.canonical_through;
                    report.reclaimed_through = status.reclaimed_through;
                    report.forwarding_error = status.forwarding_error;
                }
                Ok(other) => {
                    report.spindle_error = Some(format!("unexpected spindle reply: {other:?}"));
                }
                Err(err) => {
                    report.spindle_error = Some(err);
                }
            }
        }
    }

    if let Some(ref endpoint) = server_endpoint {
        match cred_file {
            Some(ref cred_path) => match std::fs::read_to_string(cred_path) {
                Ok(cred) => match canonical::exchange(
                    endpoint,
                    cred.trim(),
                    &CanonicalRequest::Progress,
                    Instant::now() + Duration::from_millis(750),
                ) {
                    Ok(CanonicalResponse::Progress {
                        authority,
                        frontiers,
                    }) => {
                        report.server_online = true;
                        report.server_authority = Some(authority);
                        report.server_frontiers = frontiers;
                    }
                    Ok(other) => {
                        report.server_error =
                            Some(format!("unexpected fabric-server response: {other:?}"));
                    }
                    Err(err) => {
                        report.server_error = Some(err);
                    }
                },
                Err(err) => {
                    report.server_error = Some(format!(
                        "failed reading credential {}: {err}",
                        cred_path.display()
                    ));
                }
            },
            None if report.server_error.is_none() => {
                report.server_error = Some(
                    "reader_credential_file not configured (pass --reader-credential-file or set server.json)"
                        .into(),
                );
            }
            None => {}
        }
    }

    Ok(report)
}

fn custody_to_record_batch(report: &CustodyStatusReport) -> Result<RecordBatch, String> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("spindle_socket", DataType::Utf8, false),
        Field::new("spindle_online", DataType::Boolean, false),
        Field::new("spindle_origin", DataType::Utf8, false),
        Field::new("spindle_error", DataType::Utf8, false),
        Field::new("records", DataType::UInt64, false),
        Field::new("highest_record_id", DataType::UInt64, false),
        Field::new("canonical_through", DataType::UInt64, false),
        Field::new("reclaimed_through", DataType::UInt64, false),
        Field::new("forwarding_error", DataType::Utf8, false),
        Field::new("server_endpoint", DataType::Utf8, false),
        Field::new("server_online", DataType::Boolean, false),
        Field::new("server_authority", DataType::Utf8, false),
        Field::new("server_error", DataType::Utf8, false),
        Field::new("server_frontiers_json", DataType::Utf8, false),
        Field::new("collector_spindle_watermark_id", DataType::UInt64, true),
        Field::new("catalog_error", DataType::Utf8, false),
    ]));
    let frontiers_json =
        serde_json::to_string(&report.server_frontiers).map_err(|e| e.to_string())?;
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(vec![
                report.spindle_socket.as_deref().unwrap_or(""),
            ])),
            Arc::new(BooleanArray::from(vec![report.spindle_online])),
            Arc::new(StringArray::from(vec![
                report.spindle_origin.as_deref().unwrap_or(""),
            ])),
            Arc::new(StringArray::from(vec![
                report.spindle_error.as_deref().unwrap_or(""),
            ])),
            Arc::new(UInt64Array::from(vec![report.records])),
            Arc::new(UInt64Array::from(vec![report.highest_record_id])),
            Arc::new(UInt64Array::from(vec![report.canonical_through])),
            Arc::new(UInt64Array::from(vec![report.reclaimed_through])),
            Arc::new(StringArray::from(vec![
                report.forwarding_error.as_deref().unwrap_or(""),
            ])),
            Arc::new(StringArray::from(vec![
                report.server_endpoint.as_deref().unwrap_or(""),
            ])),
            Arc::new(BooleanArray::from(vec![report.server_online])),
            Arc::new(StringArray::from(vec![
                report.server_authority.as_deref().unwrap_or(""),
            ])),
            Arc::new(StringArray::from(vec![
                report.server_error.as_deref().unwrap_or(""),
            ])),
            Arc::new(StringArray::from(vec![frontiers_json.as_str()])),
            Arc::new(UInt64Array::from(vec![
                report.collector_spindle_watermark_id,
            ])),
            Arc::new(StringArray::from(vec![
                report.catalog_error.as_deref().unwrap_or(""),
            ])),
        ],
    )
    .map_err(|e| e.to_string())
}

fn print_custody_table(report: &CustodyStatusReport) {
    println!(
        "SPINDLE online={} socket={} origin={} records={} highest={} canonical={} reclaimed={}",
        report.spindle_online,
        report.spindle_socket.as_deref().unwrap_or("-"),
        report.spindle_origin.as_deref().unwrap_or("-"),
        report.records,
        report.highest_record_id,
        report.canonical_through,
        report.reclaimed_through,
    );
    if let Some(err) = &report.spindle_error {
        println!("  spindle_error={err}");
    }
    if let Some(err) = &report.forwarding_error {
        println!("  forwarding_error={err}");
    }
    println!(
        "SERVER  online={} endpoint={} authority={}",
        report.server_online,
        report.server_endpoint.as_deref().unwrap_or("-"),
        report.server_authority.as_deref().unwrap_or("-"),
    );
    if let Some(err) = &report.server_error {
        println!("  server_error={err}");
    }
    for (origin, frontier) in &report.server_frontiers {
        println!("  frontier origin={origin} through={frontier}");
    }
    println!(
        "COLLECTOR spindle_watermark_id={}",
        report
            .collector_spindle_watermark_id
            .map_or_else(|| "none".to_owned(), |id| id.to_string())
    );
    if let Some(err) = &report.catalog_error {
        println!("  catalog_error={err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_tmp_dir_under_ramfs_is_rejected_not_rewritten() {
        assert!(resolve_cli_tmp_dir(Some(Path::new("/tmp/bad"))).is_err());
        assert!(resolve_cli_tmp_dir(Some(Path::new("/dev/shm/bad"))).is_err());
        assert_eq!(
            resolve_cli_tmp_dir(Some(Path::new("/data/tmp"))).unwrap(),
            PathBuf::from("/data/tmp")
        );
        assert!(resolve_cli_tmp_dir(None).is_ok());
    }
}
