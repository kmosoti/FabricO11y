//! `fabric` — Deterministic CLI for FabricO11y host and agent telemetry.

use arrow::array::{BooleanArray, StringArray, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use chrono::Utc;
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use fabric::canonical::{self, Request as CanonicalRequest, Response as CanonicalResponse};
use fabric::spindle::{Reply as SpindleReply, Request as SpindleRequest, transport};
use fabric_collect::{
    CollectedState, DaemonRequest, DaemonResponse, IngestEnvelope, SelfPreservationGuard,
    collect_once, configured_host_export_node, default_spindle_socket, is_forwarded_host_signal,
    poll_spindle_projection_records, query_running_daemon, run_daemon, run_spindle_only_daemon,
};
use fabric_core::arrow_schemas::{
    blame_to_record_batch, doctor_to_record_batch, drift_to_record_batch,
    encode_batches_to_ipc_stream, snapshot_to_record_batch,
};
use fabric_core::{
    BlameReport, BlameResource, CliEnvelope, DEFAULT_TMP_DIR, DoctorReport, DriftReport,
    HostSnapshot, PillarKind, StorageLayout,
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

mod device_memory;
mod guide;
mod live_cursor;
mod production_ready;
mod projection_ready;
mod receipt_batch;
mod settings;
mod table;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Json,
    Arrow,
    Table,
}

#[derive(Debug, Parser)]
#[command(
    name = "fabric",
    about = "Linux observability CLI for hosts and autonomous agents",
    version
)]
struct Cli {
    /// Non-tmpfs directory for redb OLTP state and ZSTD Parquet partitions.
    #[arg(long, env = "FABRIC_DATA_DIR", global = true)]
    data_dir: Option<PathBuf>,

    /// Non-tmpfs temporary scratch directory.
    #[arg(long, env = "FABRIC_TMP_DIR", global = true)]
    tmp_dir: Option<PathBuf>,

    /// Versioned Fabric TOML settings. Defaults to the user configuration directory.
    #[arg(long, env = "FABRIC_CONFIG_FILE", global = true)]
    config: Option<PathBuf>,

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
    /// Look up features, tested scenarios, and scoped proof references offline.
    Guide(guide::GuideArgs),
    /// Inspect effective configuration as TOML or JSON without starting a daemon.
    Settings(DaemonArgs),
    /// Capture or inspect the latest live host, slice, cgroup, unit, and storage snapshot.
    Snapshot(SnapshotArgs),
    /// Read an ordered page from the local live metric cursor (JSON only).
    LiveMetrics(live_cursor::Args),
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
    /// Inspect a bounded list of receipts at one canonical source barrier.
    ReceiptBatchReady(receipt_batch::Args),
    /// Check current local services and, optionally, canonical progress for this origin.
    ProductionReady(production_ready::Args),
    /// Inspect selected cgroup v2 device-memory usage and limits.
    DeviceMemory(device_memory::Args),
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
    /// Approximate percentiles over query rows use p50:col, p90:col, p95:col, or p99:col.
    #[arg(long, value_delimiter = ',')]
    agg: Vec<String>,
    /// Maximum rows to return.
    #[arg(long, default_value_t = 200)]
    limit: usize,
}

#[derive(Debug, Clone, Args)]
struct SnapshotArgs {
    /// Emit only host identity, capture time, and collection counts (JSON only).
    #[arg(long, requires = "json", conflicts_with_all = ["arrow", "table"])]
    summary: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SnapshotSummary {
    timestamp_unix_ms: i64,
    hostname: String,
    counts: SnapshotCounts,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SnapshotCounts {
    processes: usize,
    cgroups: usize,
    fs_churn: usize,
    units: usize,
}

fn summarize_snapshot(snapshot: &HostSnapshot) -> SnapshotSummary {
    SnapshotSummary {
        timestamp_unix_ms: snapshot.timestamp_unix_ms,
        hostname: snapshot.hostname.clone(),
        counts: SnapshotCounts {
            processes: snapshot.processes.len(),
            cgroups: snapshot.cgroups.len(),
            fs_churn: snapshot.fs_churn.len(),
            units: snapshot.units.len(),
        },
    }
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
    /// Path to Spindle Unix socket (defaults to the Spindle settings file or XDG_RUNTIME_DIR/spindle/spindle.sock).
    #[arg(long, env = "SPINDLE_SOCKET")]
    spindle_socket: Option<PathBuf>,
    /// Canonical fabric-server host:port (defaults to Spindle forwarding or server listen settings).
    #[arg(long, env = "FABRIC_SERVER_ENDPOINT")]
    server_endpoint: Option<String>,
    /// Reader credential file used to query canonical progress from fabric-server.
    #[arg(long, env = "FABRIC_READER_CREDENTIAL_FILE")]
    reader_credential_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct DaemonArgs {
    /// Collection tick interval in milliseconds.
    #[arg(long)]
    interval_ms: Option<u64>,
    /// Maximum hot metric ring payload in bytes.
    #[arg(long)]
    ring_budget_bytes: Option<usize>,
    /// Maximum retained local Parquet data in bytes.
    #[arg(long)]
    max_disk_bytes: Option<u64>,
    /// Run a single collection & persistence cycle and exit.
    #[arg(long)]
    once: bool,
    /// Project from a dedicated Spindle socket without collecting host or ingest data.
    #[arg(long, conflicts_with = "once")]
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
        eprintln!("fabric: {}", fabric_core::sanitize_terminal_text(&err));
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
    let matches = Cli::command().get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    let format = if cli.json {
        OutputFormat::Json
    } else if cli.arrow {
        OutputFormat::Arrow
    } else if cli.table || std::io::stdout().is_terminal() {
        OutputFormat::Table
    } else {
        OutputFormat::Json
    };

    if let Commands::Guide(args) = &cli.command {
        return guide::run(args, format);
    }
    if let Some(result) = dispatch_device_memory(&cli, device_memory::run) {
        return result;
    }
    if let Commands::ProductionReady(args) = &cli.command {
        std::process::exit(production_ready::run(&cli, &matches, args, format).await);
    }
    let (profile_path, profile) = settings::load_profile(cli.config.as_deref())?;
    let (data_dir, _) = settings::data_dir(&cli, &matches, &profile);
    if let Commands::ProjectionReady(args) = cli.command {
        std::process::exit(projection_ready::run(
            args,
            data_dir,
            cli.arrow || cli.table,
        ));
    }
    if let Commands::ReceiptBatchReady(args) = cli.command {
        std::process::exit(receipt_batch::run(args, cli.arrow || cli.table));
    }
    if let Commands::Settings(args) = &cli.command {
        let effective = settings::effective(&cli, &matches, &profile, args)?;
        return settings::print(&cli, args, effective, profile_path.as_deref());
    }
    let (tmp_dir, _) = settings::tmp_dir(&cli, &matches, &profile)?;
    let layout = validated_storage_layout(&data_dir)?;

    match &cli.command {
        Commands::Guide(_)
        | Commands::ProjectionReady(_)
        | Commands::ReceiptBatchReady(_)
        | Commands::ProductionReady(_)
        | Commands::DeviceMemory(_)
        | Commands::Settings(_) => {
            unreachable!("commands dispatched before storage access")
        }
        Commands::LiveMetrics(args) => live_cursor::run(&layout, args, format).await,
        Commands::Snapshot(args) => {
            let snap = fetch_snapshot(&layout, &tmp_dir)?;
            match format {
                OutputFormat::Json if args.summary => {
                    emit_json("snapshot", &summarize_snapshot(&snap))
                }
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
            let mut daemon_config = settings::effective(&cli, &matches, &profile, args)?.config;
            daemon_config.data_dir = layout.data_dir().to_path_buf();
            if args.once {
                let export_node = configured_host_export_node()?;
                let catalog = CatalogStore::open(layout.data_dir())?;
                let _writer_lock = fabric_store::try_acquire_writer_lock(catalog.layout())?;
                let state = collect_once(catalog.layout().data_dir(), &tmp_dir)?;
                let olap = fabric_store::OlapParquetStore::new(catalog.layout().clone());
                let spindle_sock = daemon_config.spindle_socket.clone();
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

fn dispatch_device_memory(
    cli: &Cli,
    run: impl FnOnce(&device_memory::Args, bool) -> Result<(), String>,
) -> Option<Result<(), String>> {
    match &cli.command {
        Commands::DeviceMemory(args) => Some(run(args, cli.arrow || cli.table)),
        _ => None,
    }
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

fn print_snapshot_table(s: &HostSnapshot) {
    let _ = table::render_snapshot_table(&mut std::io::stdout(), s);
}

fn print_query_table(domain: &str, rows: &[serde_json::Value]) {
    let _ = table::render_query_table(&mut std::io::stdout(), domain, rows);
}

fn print_lineage_table(sub: &LineageSubgraph) {
    let _ = table::render_lineage_table(&mut std::io::stdout(), sub);
}

fn print_blame_table(report: &BlameReport) {
    let _ = table::render_blame_table(&mut std::io::stdout(), report);
}

fn print_drift_table(report: &DriftReport) {
    let _ = table::render_drift_table(&mut std::io::stdout(), report);
}

fn print_doctor_table(report: &DoctorReport) {
    let _ = table::render_doctor_table(&mut std::io::stdout(), report);
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct SpindleFileConfigHint {
    socket: Option<PathBuf>,
    forwarding: Option<SpindleForwardHint>,
    quota_bytes: Option<u64>,
    max_connections: Option<usize>,
    client_timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct SpindleForwardHint {
    endpoint: Option<String>,
    authority: Option<String>,
    credential_file: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct ServerFileConfigHint {
    listen: Option<String>,
    reader_credential_file: Option<PathBuf>,
    quota_bytes: Option<u64>,
    projection_quota_bytes: Option<u64>,
    max_connections: Option<usize>,
    nodes: Option<Vec<NodeCredentialHint>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct NodeCredentialHint {
    origin: String,
    credential_file: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CustodyStatusReport {
    pub(crate) spindle_socket: Option<String>,
    pub(crate) spindle_online: bool,
    pub(crate) spindle_origin: Option<String>,
    pub(crate) spindle_error: Option<String>,
    pub(crate) records: u64,
    pub(crate) highest_record_id: u64,
    pub(crate) canonical_through: u64,
    pub(crate) reclaimed_through: u64,
    pub(crate) forwarding_error: Option<String>,
    pub(crate) server_endpoint: Option<String>,
    pub(crate) server_online: bool,
    pub(crate) server_authority: Option<String>,
    pub(crate) server_error: Option<String>,
    pub(crate) server_frontiers: BTreeMap<String, u64>,
    pub(crate) collector_spindle_watermark_id: Option<u64>,
    pub(crate) catalog_error: Option<String>,
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

fn read_service_hint<T: serde::de::DeserializeOwned + Default>(
    directory: &Path,
    stem: &str,
) -> Result<T, String> {
    Ok(fabric::config::discover(directory, stem)?
        .map(|(_, settings)| settings)
        .unwrap_or_default())
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
        Some(d) => match read_service_hint::<SpindleFileConfigHint>(d, "spindle") {
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
        Some(d) => match read_service_hint::<ServerFileConfigHint>(d, "server") {
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
                    "reader_credential_file not configured (pass --reader-credential-file or set the server settings file)"
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
    let _ = table::render_custody_table(&mut std::io::stdout(), report);
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

    #[test]
    fn device_memory_dispatches_before_profile_and_storage_setup() {
        let cli = Cli::try_parse_from([
            "fabric",
            "--config",
            "/missing/profile.toml",
            "device-memory",
            "--cgroup",
            "/user.slice/job.scope",
        ])
        .unwrap();
        let mut called = false;
        let result = dispatch_device_memory(&cli, |args, unsupported_format| {
            called = true;
            assert_eq!(args.cgroup, "/user.slice/job.scope");
            assert!(!unsupported_format);
            Ok(())
        });
        assert!(called);
        assert!(matches!(result, Some(Ok(()))));

        let table_cli = Cli::try_parse_from([
            "fabric",
            "--table",
            "device-memory",
            "--cgroup",
            "/user.slice/job.scope",
        ])
        .unwrap();
        let result = dispatch_device_memory(&table_cli, |_, unsupported_format| {
            assert!(unsupported_format);
            Ok(())
        });
        assert!(matches!(result, Some(Ok(()))));
    }

    #[test]
    fn snapshot_summary_requires_json_and_defaults_to_full_snapshot() {
        let summary = Cli::try_parse_from(["fabric", "snapshot", "--summary", "--json"])
            .expect("summary JSON arguments parse");
        assert!(matches!(
            summary.command,
            Commands::Snapshot(SnapshotArgs { summary: true })
        ));

        assert!(Cli::try_parse_from(["fabric", "snapshot", "--summary"]).is_err());
        for incompatible in ["--arrow", "--table"] {
            assert!(
                Cli::try_parse_from(["fabric", "snapshot", "--summary", incompatible]).is_err()
            );
            assert!(
                Cli::try_parse_from(["fabric", incompatible, "snapshot", "--summary"]).is_err()
            );
        }
        let default = Cli::try_parse_from(["fabric", "snapshot"]).expect("default snapshot parses");
        assert!(matches!(
            default.command,
            Commands::Snapshot(SnapshotArgs { summary: false })
        ));
    }

    #[test]
    fn snapshot_summary_uses_vector_counts_and_omits_detail_rows() {
        let snapshot = HostSnapshot {
            timestamp_unix_ms: 1234,
            hostname: "node-a".to_owned(),
            processes: vec![Default::default(), Default::default()],
            cgroups: vec![Default::default()],
            fs_churn: vec![Default::default(), Default::default(), Default::default()],
            units: vec![
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
            ],
            ..Default::default()
        };

        let summary = summarize_snapshot(&snapshot);
        assert_eq!(summary.timestamp_unix_ms, 1234);
        assert_eq!(summary.hostname, "node-a");
        assert_eq!(summary.counts.processes, 2);
        assert_eq!(summary.counts.cgroups, 1);
        assert_eq!(summary.counts.fs_churn, 3);
        assert_eq!(summary.counts.units, 4);

        let json = serde_json::to_value(summary).unwrap();
        assert_eq!(json.as_object().unwrap().len(), 3);
        assert!(json.get("processes").is_none());
        assert!(json.get("memory").is_none());
        assert!(json.get("self_health").is_none());
        assert_eq!(json["counts"].as_object().unwrap().len(), 4);
    }
}
