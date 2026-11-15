//! `fabric` — Deterministic CLI sensory plane for FabricO11y host, agent, and OTLP telemetry.

use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use fabric_collect::{
    CollectedState, DaemonConfig, DaemonRequest, DaemonResponse, SelfPreservationGuard,
    collect_once, query_running_daemon, run_daemon,
};
use fabric_core::arrow_schemas::{
    blame_to_record_batch, doctor_to_record_batch, drift_to_record_batch,
    encode_batches_to_ipc_stream, snapshot_to_record_batch,
};
use fabric_core::{
    BlameReport, BlameResource, CliEnvelope, DEFAULT_DATA_DIR, DEFAULT_TMP_DIR, DoctorReport,
    DriftReport, HostSnapshot, PillarKind, StorageLayout, validate_non_tmp_path,
};
use fabric_graph::{Direction, LineageSubgraph, lineage_to_record_batch};
use fabric_query::unified::{
    QueryDomain, UnifiedQuerySpec, execute_unified_query, parse_duration_ms,
};
use fabric_store::CatalogStore;
use serde::Serialize;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

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
    #[arg(long, env = "TMPDIR", default_value = DEFAULT_TMP_DIR, global = true)]
    tmp_dir: PathBuf,

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

#[derive(Debug, Args)]
struct DaemonArgs {
    /// Collection tick interval in milliseconds.
    #[arg(long, default_value_t = 2000)]
    interval_ms: u64,
    /// Run a single collection & persistence cycle and exit.
    #[arg(long)]
    once: bool,
    /// Stop automatically after N ticks.
    #[arg(long)]
    max_ticks: Option<u64>,
}

#[derive(Debug, Serialize)]
struct QueryJsonPayload {
    domain: String,
    row_count: usize,
    rows: Vec<serde_json::Value>,
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("fabric: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let format = if cli.json {
        OutputFormat::Json
    } else if cli.arrow {
        OutputFormat::Arrow
    } else if cli.table || std::io::stdout().is_terminal() {
        OutputFormat::Table
    } else {
        OutputFormat::Json
    };

    let tmp_dir = if cli.tmp_dir.starts_with("/tmp") || cli.tmp_dir.starts_with("/dev/shm") {
        PathBuf::from(DEFAULT_TMP_DIR)
    } else {
        cli.tmp_dir.clone()
    };
    validate_non_tmp_path(&cli.data_dir).map_err(|e| e.to_string())?;
    validate_non_tmp_path(&tmp_dir).map_err(|e| e.to_string())?;
    let layout = StorageLayout::new(&cli.data_dir).map_err(|e| e.to_string())?;

    match &cli.command {
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
                Some(raw) => Some(Utc::now().timestamp_millis() - parse_duration_ms(raw)?),
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
        Commands::Daemon(args) => {
            if args.once {
                let state = collect_once(layout.data_dir(), &tmp_dir)?;
                let catalog = CatalogStore::open(layout.data_dir())?;
                catalog.commit_cycle(&state.snapshot, &state.drift, &state.graph)?;
                let olap = fabric_store::OlapParquetStore::new(catalog.layout().clone());
                let _ = olap.write_metrics(&state.metrics)?;
                let _ = olap.write_fs_events(&state.fs_events)?;
                let _ = olap.write_edges(
                    &state
                        .graph
                        .to_edge_records(state.snapshot.timestamp_unix_ms),
                )?;
                return emit_json("daemon_once", &state.snapshot.self_health);
            }
            run_daemon(DaemonConfig {
                data_dir: layout.data_dir().to_path_buf(),
                tmp_dir,
                interval_ms: args.interval_ms,
                max_ticks: args.max_ticks,
                ..Default::default()
            })
            .await
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
            Some(DaemonResponse::HotMetrics(pts)) => pts,
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
