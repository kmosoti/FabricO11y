//! Read-only inspection of one origin-qualified Spindle receipt.

use fabric::canonical::{self, ProjectionReadyRequest, ProjectionReason, ProjectionStatus};
use fabric::observation::{Data, Identity, decode_versioned};
use fabric::spindle::{READ_ORIGIN_MISMATCH, custody::Receipt, transport};
use fabric_core::{QueryDomain, SpindleProvenance, parse_duration_ms};
use fabric_store::{CatalogStore, SpindleProjectionFrontier, validated_storage_layout};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

const INPUT_LIMIT: u64 = 4096;
const ONE_CHECK: Duration = Duration::from_secs(1);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Target {
    Local,
    Canonical,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Where the exact observation must be queryable.
    #[arg(long, value_enum, default_value = "local")]
    target: Target,
    /// Expected Fabric Server authority for canonical projection.
    #[arg(long, required_if_eq("target", "canonical"))]
    authority: Option<String>,
    /// Override the canonical receiver endpoint.
    #[arg(long)]
    server_endpoint: Option<String>,
    /// Override the private canonical reader credential file.
    #[arg(long)]
    reader_credential_file: Option<PathBuf>,
    /// Origin associated with the receipt at submission time.
    #[arg(long)]
    origin: String,
    /// JSON receipt or receipt reply file; use '-' to read stdin.
    #[arg(long)]
    receipt: PathBuf,
    /// Poll for this duration, up to five minutes (for example, 30s).
    #[arg(long, value_parser = wait_duration)]
    wait: Option<Duration>,
    /// Override the configured local Spindle socket.
    #[arg(long, env = "SPINDLE_SOCKET")]
    spindle_socket: Option<PathBuf>,
}

fn wait_duration(raw: &str) -> Result<Duration, String> {
    let millis = parse_duration_ms(raw)?;
    if raw.trim().starts_with('-') || millis > 300_000 {
        return Err("wait must be between 0ms and 5m".into());
    }
    Ok(Duration::from_millis(millis as u64))
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Visible,
    Pending,
    Unavailable,
}

#[derive(Clone, Serialize)]
struct Report {
    status: Status,
    reason: &'static str,
    origin: String,
    record_id: Option<u64>,
    identity: Option<Identity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    authority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_authority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    custody_frontier: Option<u64>,
    projection_frontier: Option<SpindleProjectionFrontier>,
}

impl Report {
    fn finish(mut self, status: Status, reason: &'static str) -> Self {
        self.status = status;
        self.reason = reason;
        self
    }
}

enum Update {
    Progress(Report),
    Done(Report),
    Invalid(String),
}

#[derive(Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
enum ReceiptReply {
    Receipt { receipt: Receipt },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ReceiptInput {
    Receipt(Receipt),
    Reply(ReceiptReply),
}

fn read_receipt(path: &Path) -> Result<Receipt, String> {
    let mut bytes = Vec::new();
    if path == Path::new("-") {
        io::stdin()
            .lock()
            .take(INPUT_LIMIT + 1)
            .read_to_end(&mut bytes)
    } else {
        File::open(path).and_then(|file| file.take(INPUT_LIMIT + 1).read_to_end(&mut bytes))
    }
    .map_err(|err| format!("read receipt: {err}"))?;
    if bytes.len() as u64 > INPUT_LIMIT {
        return Err("receipt exceeds 4096 bytes".into());
    }
    let receipt =
        match serde_json::from_slice(&bytes).map_err(|err| format!("invalid receipt: {err}"))? {
            ReceiptInput::Receipt(receipt)
            | ReceiptInput::Reply(ReceiptReply::Receipt { receipt }) => receipt,
        };
    if receipt.id == 0
        || receipt.id > i64::MAX as u64
        || receipt.identity.sequence == 0
        || !valid_name(&receipt.identity.producer)
        || !valid_name(&receipt.identity.generation)
    {
        return Err("invalid receipt record ID or identity".into());
    }
    Ok(receipt)
}

fn valid_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.contains('\0')
}

fn socket(args: &Args) -> Result<PathBuf, String> {
    if let Some(path) = super::non_empty_path(args.spindle_socket.clone()) {
        return fabric_store::validate_non_tmp_path_on_disk(&path).map_err(|err| err.to_string());
    }
    let configured = if let Some(dir) = super::fabrico11y_config_dir() {
        super::read_optional_json::<super::SpindleFileConfigHint>(&dir.join("spindle.json"))?.socket
    } else {
        None
    };
    let path = super::non_empty_path(configured)
        .or_else(fabric_collect::default_spindle_socket)
        .ok_or("no Spindle socket configured")?;
    fabric_store::validate_non_tmp_path_on_disk(&path).map_err(|err| err.to_string())
}

fn inspect(
    args: &Args,
    data_dir: &Path,
    deadline: Instant,
    sender: &Sender<Update>,
    mut report: Report,
) -> Option<Report> {
    let Ok(receipt) = read_receipt(&args.receipt).inspect_err(|err| {
        let _ = sender.send(Update::Invalid(err.clone()));
    }) else {
        return None;
    };
    report.record_id = Some(receipt.id);
    report.identity = Some(receipt.identity.clone());
    let _ = sender.send(Update::Progress(report.clone()));

    Some(match args.target {
        Target::Local => inspect_local(args, data_dir, deadline, sender, report, receipt),
        Target::Canonical => inspect_canonical(args, deadline, sender, report, receipt),
    })
}

fn inspect_local(
    args: &Args,
    data_dir: &Path,
    deadline: Instant,
    sender: &Sender<Update>,
    mut report: Report,
    receipt: Receipt,
) -> Report {
    let layout = match validated_storage_layout(data_dir) {
        Ok(layout) => layout,
        Err(_) => return report.finish(Status::Unavailable, "invalid_projection_path"),
    };
    report.projection_frontier =
        CatalogStore::read_spindle_projection_frontier(&layout, deadline).ok();
    let _ = sender.send(Update::Progress(report.clone()));
    let socket = match socket(args) {
        Ok(socket) => socket,
        Err(_) => return report.finish(Status::Unavailable, "spindle_socket_unavailable"),
    };
    let records =
        match transport::read_raw_for_origin(&socket, &args.origin, receipt.id - 1, deadline) {
            Ok(records) => records,
            Err(err) if err == READ_ORIGIN_MISMATCH => {
                return report.finish(Status::Unavailable, "origin_mismatch");
            }
            Err(_) => {
                return report.finish(Status::Unavailable, "custody_evidence_unavailable");
            }
        };
    let Some((id, raw)) = records.first().filter(|record| record.0 == receipt.id) else {
        return report.finish(Status::Unavailable, "record_unavailable");
    };
    let Ok(observation) = decode_versioned(raw) else {
        return report.finish(Status::Unavailable, "custody_evidence_unavailable");
    };
    if observation.identity() != &receipt.identity {
        return report.finish(Status::Unavailable, "identity_mismatch");
    }
    let key = SpindleProvenance {
        origin: args.origin.clone(),
        record_id: *id,
        producer: receipt.identity.producer,
        generation: receipt.identity.generation,
        sequence: receipt.identity.sequence,
    };
    let domain = match observation.data() {
        Data::Metric { .. } => QueryDomain::Metrics,
        Data::ServiceRequest { .. } | Data::AgentRun { .. } | Data::ToolExecution { .. } => {
            QueryDomain::Traces
        }
        Data::Log { .. } | Data::JsonLog { .. } | Data::Event { .. } | Data::Gap { .. } => {
            QueryDomain::Logs
        }
    };
    let timestamp_ms = (observation.timestamp_ns() / 1_000_000) as i64;

    loop {
        if Instant::now() >= deadline {
            return report;
        }
        let frontier = match CatalogStore::read_spindle_projection_frontier(&layout, deadline) {
            Ok(frontier) => frontier,
            Err(_) if report.status == Status::Pending && Instant::now() >= deadline => {
                return report;
            }
            Err(_) => return report.finish(Status::Unavailable, "frontier_unavailable"),
        };
        report.projection_frontier = Some(frontier.clone());
        if frontier
            .origin
            .as_deref()
            .is_some_and(|origin| origin != args.origin)
        {
            return report.finish(Status::Unavailable, "projection_origin_mismatch");
        }
        if receipt.id <= frontier.legacy_through
            || (frontier.origin.is_none() && frontier.record_id > 0)
        {
            return report.finish(Status::Unavailable, "legacy_projection");
        }
        if frontier.record_id >= receipt.id {
            report = report.finish(Status::Unavailable, "projection_evidence_unavailable");
            let _ = sender.send(Update::Progress(report.clone()));
            let matched = tokio::runtime::Builder::new_current_thread()
                .build()
                .map(|runtime| {
                    runtime.block_on(fabric_query::probe_spindle_projection(
                        &layout,
                        domain,
                        timestamp_ms,
                        &key,
                    ))
                });
            return match matched {
                Ok(Ok(true)) => report.finish(Status::Visible, "committed_row"),
                Ok(Ok(false)) => report.finish(Status::Unavailable, "projected_row_unavailable"),
                _ => report,
            };
        }
        report = report.finish(Status::Pending, "awaiting_projection");
        let _ = sender.send(Update::Progress(report.clone()));
        if !args.wait.is_some_and(|wait| !wait.is_zero()) {
            return report;
        }
        std::thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

fn canonical_connection(args: &Args) -> Result<fabric::query::Client, &'static str> {
    let config_dir = super::fabrico11y_config_dir();
    let mut endpoint = super::non_empty_str(
        args.server_endpoint
            .clone()
            .or_else(|| std::env::var("FABRIC_SERVER_ENDPOINT").ok()),
    );
    let mut credential = super::non_empty_path(
        args.reader_credential_file
            .clone()
            .or_else(|| std::env::var_os("FABRIC_READER_CREDENTIAL_FILE").map(PathBuf::from)),
    );
    if let Some(dir) = config_dir {
        if endpoint.is_none() {
            endpoint = super::read_optional_json::<super::SpindleFileConfigHint>(
                &dir.join("spindle.json"),
            )
            .ok()
            .and_then(|spindle| spindle.forwarding)
            .and_then(|forwarding| super::non_empty_str(forwarding.endpoint));
        }
        if endpoint.is_none() || credential.is_none() {
            let server =
                super::read_optional_json::<super::ServerFileConfigHint>(&dir.join("server.json"))
                    .map_err(|_| "canonical_configuration_unavailable")?;
            endpoint = endpoint.or_else(|| super::non_empty_str(server.listen));
            credential =
                credential.or_else(|| super::non_empty_path(server.reader_credential_file));
        }
    }
    let endpoint =
        super::normalize_canonical_endpoint(endpoint.ok_or("canonical_endpoint_unavailable")?)
            .map_err(|_| "canonical_endpoint_unavailable")?;
    let credential_path = credential.ok_or("canonical_credential_unavailable")?;
    fabric_store::validate_non_tmp_path_on_disk(&credential_path)
        .map_err(|_| "canonical_credential_unavailable")?;
    let credential = fabric::query::read_credential_file(&credential_path)
        .map_err(|_| "canonical_credential_unavailable")?;
    Ok(fabric::query::Client::new(endpoint, credential))
}

fn inspect_canonical(
    args: &Args,
    deadline: Instant,
    sender: &Sender<Update>,
    mut report: Report,
    receipt: Receipt,
) -> Report {
    let client = match canonical_connection(args) {
        Ok(client) => client,
        Err(reason) => return report.finish(Status::Unavailable, reason),
    };
    let Some(authority) = args.authority.clone() else {
        return report.finish(Status::Unavailable, "authority_unavailable");
    };
    let mut request = ProjectionReadyRequest {
        authority,
        origin: args.origin.clone(),
        source_id: receipt.id,
        identity: receipt.identity,
        timeout_ms: 1,
        // A local polling deadline keeps the canonical worker free between checks.
        wait: false,
    };
    while Instant::now() < deadline {
        request.timeout_ms = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .clamp(1, canonical::MAX_PROJECTION_READY_TIMEOUT_MS.into())
            as u32;
        let result = client.exchange(
            &canonical::Request::ProjectionReady {
                request: request.clone(),
            },
            deadline,
        );
        let readiness = match result {
            Ok(canonical::Response::ProjectionReady { readiness }) => readiness,
            Ok(canonical::Response::Error { error }) if error == "read credential required" => {
                return report.finish(Status::Unavailable, "canonical_reader_required");
            }
            _ if report.status == Status::Pending && Instant::now() >= deadline => return report,
            _ => return report.finish(Status::Unavailable, "canonical_evidence_unavailable"),
        };
        if readiness
            .authority
            .as_ref()
            .is_some_and(|authority| authority != &request.authority)
            || (readiness.status != ProjectionStatus::Unavailable && readiness.authority.is_none())
        {
            return report.finish(Status::Unavailable, "authority_mismatch");
        }
        let valid_frontiers = match readiness.status {
            ProjectionStatus::Visible => readiness
                .custody_frontier
                .zip(readiness.projection_frontier)
                .is_some_and(|(custody, projection)| {
                    custody >= request.source_id && projection >= request.source_id
                }),
            ProjectionStatus::Pending => readiness
                .custody_frontier
                .zip(readiness.projection_frontier)
                .is_some_and(|(custody, projection)| {
                    custody >= request.source_id && projection < request.source_id
                }),
            ProjectionStatus::Unavailable => true,
        };
        if !valid_frontiers {
            return report.finish(Status::Unavailable, "canonical_evidence_unavailable");
        }
        report.authority = readiness.authority;
        report.custody_frontier = readiness.custody_frontier;
        report.projection_frontier =
            readiness
                .projection_frontier
                .map(|record_id| SpindleProjectionFrontier {
                    origin: Some(args.origin.clone()),
                    legacy_through: 0,
                    record_id,
                });
        let status = match readiness.status {
            ProjectionStatus::Visible => Status::Visible,
            ProjectionStatus::Pending => Status::Pending,
            ProjectionStatus::Unavailable => Status::Unavailable,
        };
        report = report.finish(status, canonical_reason(readiness.reason));
        if status != Status::Pending || !args.wait.is_some_and(|wait| !wait.is_zero()) {
            return report;
        }
        let _ = sender.send(Update::Progress(report.clone()));
        std::thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
    }
    report
}

fn canonical_reason(reason: ProjectionReason) -> &'static str {
    match reason {
        ProjectionReason::CommittedRow => "committed_row",
        ProjectionReason::AwaitingProjection => "awaiting_projection",
        ProjectionReason::AuthorityMismatch => "authority_mismatch",
        ProjectionReason::UnknownOrigin => "origin_mismatch",
        ProjectionReason::RecordUnavailable => "record_unavailable",
        ProjectionReason::IdentityMismatch => "identity_mismatch",
        ProjectionReason::CustodyEvidenceUnavailable => "custody_evidence_unavailable",
        ProjectionReason::ProjectionEvidenceUnavailable => "projection_evidence_unavailable",
        ProjectionReason::ProjectedRowUnavailable => "projected_row_unavailable",
        ProjectionReason::BackendUnsupported => "canonical_backend_unsupported",
    }
}

pub(crate) fn run(args: Args, data_dir: PathBuf, unsupported_format: bool) -> i32 {
    if unsupported_format || !valid_name(&args.origin) {
        eprintln!(
            "fabric: projection-ready requires JSON output and a nonempty origin of at most 128 bytes"
        );
        return 2;
    }
    match args.target {
        Target::Canonical if !args.authority.as_deref().is_some_and(valid_name) => {
            eprintln!(
                "fabric: the canonical target requires a nonempty authority of at most 128 bytes"
            );
            return 2;
        }
        Target::Local
            if args.authority.is_some()
                || args.server_endpoint.is_some()
                || args.reader_credential_file.is_some() =>
        {
            eprintln!(
                "fabric: authority, server endpoint and reader credential require --target canonical"
            );
            return 2;
        }
        _ => {}
    }
    let deadline = Instant::now()
        + args
            .wait
            .filter(|wait| !wait.is_zero())
            .unwrap_or(ONE_CHECK);
    let mut report = Report {
        status: Status::Unavailable,
        reason: "evidence_deadline",
        origin: args.origin.clone(),
        record_id: None,
        identity: None,
        authority: None,
        expected_authority: args.authority.clone(),
        custody_frontier: None,
        projection_frontier: None,
    };
    let initial = report.clone();
    let (sender, receiver) = mpsc::channel();
    if std::thread::Builder::new()
        .name("projection-inspection".into())
        .spawn(move || {
            if let Some(final_report) = inspect(&args, &data_dir, deadline, &sender, initial) {
                let _ = sender.send(Update::Done(final_report));
            }
        })
        .is_err()
    {
        report.reason = "inspection_worker_unavailable";
    } else {
        while let Ok(update) =
            receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            match update {
                Update::Progress(progress) => report = progress,
                Update::Done(final_report) => {
                    if Instant::now() <= deadline || final_report.status != Status::Visible {
                        report = final_report;
                    }
                    break;
                }
                Update::Invalid(err) => {
                    eprintln!("fabric: {err}");
                    return 2;
                }
            }
        }
    }
    if let Err(err) = super::emit_json("projection-ready", &report) {
        eprintln!("fabric: {err}");
        return 1;
    }
    match report.status {
        Status::Visible => 0,
        Status::Pending => 3,
        Status::Unavailable => 4,
    }
}
