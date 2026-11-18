//! Read-only readiness for the resident local services and optional canonical origin.

use super::{Cli, OutputFormat};
use arrow::array::{ArrayRef, BooleanArray, StringArray, UInt64Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use clap::ArgMatches;
use fabric::canonical::{self, Request as CanonicalRequest, Response as CanonicalResponse};
use fabric::spindle::{Reply, Request, transport};
use fabric_collect::{default_spindle_socket, query_running_doctor};
use fabric_core::arrow_schemas::encode_batches_to_ipc_stream;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SCOPE: &str = "Checks the current resident collector and local Spindle. Canonical progress is checked only when requested. Use fabric projection-ready for an exact receipt and fabric guide for proof scope.";
const PROBE_TIMEOUT: Duration = Duration::from_millis(750);

#[derive(Debug, clap::Args)]
pub(super) struct Args {
    /// Require authenticated canonical progress for the local Spindle origin.
    #[arg(long)]
    require_canonical: bool,
    /// Override the local Spindle socket.
    #[arg(long, env = "SPINDLE_SOCKET")]
    spindle_socket: Option<PathBuf>,
    /// Override the canonical receiver endpoint.
    #[arg(long, env = "FABRIC_SERVER_ENDPOINT")]
    server_endpoint: Option<String>,
    /// Override the private canonical reader credential file.
    #[arg(long, env = "FABRIC_READER_CREDENTIAL_FILE")]
    reader_credential_file: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Ready,
    NotReady,
    Unavailable,
    NotRequested,
}

impl Status {
    fn text(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NotReady => "not_ready",
            Self::Unavailable => "unavailable",
            Self::NotRequested => "not_requested",
        }
    }
}

#[derive(Serialize)]
struct Probe {
    status: Status,
    reason: &'static str,
}

impl Probe {
    fn new(status: Status, reason: &'static str) -> Self {
        Self { status, reason }
    }
}

#[derive(Serialize)]
struct Report {
    version: u32,
    status: Status,
    require_canonical: bool,
    collector: Probe,
    spindle: Probe,
    canonical: Probe,
    spindle_origin: Option<String>,
    expected_authority: Option<String>,
    canonical_authority: Option<String>,
    canonical_frontier: Option<u64>,
    scope: &'static str,
}

impl Report {
    fn new(require_canonical: bool) -> Self {
        Self {
            version: 1,
            status: Status::Unavailable,
            require_canonical,
            collector: Probe::new(Status::Unavailable, "collector_not_checked"),
            spindle: Probe::new(Status::Unavailable, "spindle_not_checked"),
            canonical: if require_canonical {
                Probe::new(Status::Unavailable, "canonical_not_checked")
            } else {
                Probe::new(Status::NotRequested, "canonical_not_requested")
            },
            spindle_origin: None,
            expected_authority: None,
            canonical_authority: None,
            canonical_frontier: None,
            scope: SCOPE,
        }
    }

    fn finish(&mut self) {
        let statuses = [
            self.collector.status,
            self.spindle.status,
            self.canonical.status,
        ];
        self.status = if statuses.contains(&Status::Unavailable) {
            Status::Unavailable
        } else if statuses.contains(&Status::NotReady) {
            Status::NotReady
        } else {
            Status::Ready
        };
    }

    fn exit_code(&self) -> i32 {
        match self.status {
            Status::Ready => 0,
            Status::NotReady => 2,
            Status::Unavailable | Status::NotRequested => 3,
        }
    }
}

// Only the socket is relevant to local readiness. Ignoring unrelated forwarding
// fields prevents malformed or disconnected canonical settings from affecting it.
#[derive(Default, Deserialize)]
struct SocketHint {
    socket: Option<PathBuf>,
}

fn valid_identity(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.contains('\0')
}

fn spindle_socket(
    args: &Args,
    profile: &super::settings::Profile,
) -> Result<PathBuf, &'static str> {
    let mut socket = super::non_empty_path(args.spindle_socket.clone())
        .or_else(|| super::non_empty_path(profile.spindle_socket()));
    if socket.is_none()
        && let Some(dir) = super::fabrico11y_config_dir()
    {
        let hint: SocketHint = super::read_service_hint(&dir, "spindle")
            .map_err(|_| "spindle_configuration_unavailable")?;
        socket = super::non_empty_path(hint.socket);
    }
    let socket = socket
        .or_else(default_spindle_socket)
        .ok_or("spindle_socket_unavailable")?;
    fabric_store::validate_non_tmp_path_on_disk(&socket).map_err(|_| "spindle_socket_invalid")
}

fn canonical_connection(args: &Args) -> Result<(String, String), &'static str> {
    let mut endpoint = super::non_empty_str(args.server_endpoint.clone());
    let mut credential = super::non_empty_path(args.reader_credential_file.clone());
    if let Some(dir) = super::fabrico11y_config_dir() {
        if endpoint.is_none() {
            let spindle: super::SpindleFileConfigHint =
                super::read_service_hint(&dir, "spindle")
                    .map_err(|_| "canonical_configuration_unavailable")?;
            endpoint = spindle
                .forwarding
                .and_then(|f| super::non_empty_str(f.endpoint));
        }
        if endpoint.is_none() || credential.is_none() {
            let server: super::ServerFileConfigHint = super::read_service_hint(&dir, "server")
                .map_err(|_| "canonical_configuration_unavailable")?;
            endpoint = endpoint.or_else(|| super::non_empty_str(server.listen));
            credential =
                credential.or_else(|| super::non_empty_path(server.reader_credential_file));
        }
    }
    let endpoint =
        super::normalize_canonical_endpoint(endpoint.ok_or("canonical_endpoint_unavailable")?)
            .map_err(|_| "canonical_endpoint_invalid")?;
    let credential_path = credential.ok_or("canonical_credential_unavailable")?;
    let credential = fabric::query::read_credential_file(&credential_path)
        .map_err(|_| "canonical_credential_unavailable")?;
    Ok((endpoint, credential))
}

async fn inspect(cli: &Cli, matches: &ArgMatches, args: &Args) -> Report {
    let mut report = Report::new(args.require_canonical);
    let profile = match super::settings::load_profile(cli.config.as_deref()) {
        Ok((_, profile)) => profile,
        Err(_) => {
            report.collector =
                Probe::new(Status::Unavailable, "collector_configuration_unavailable");
            report.spindle = Probe::new(Status::Unavailable, "spindle_configuration_unavailable");
            if args.require_canonical {
                report.canonical = Probe::new(Status::Unavailable, "local_origin_unavailable");
            }
            report.finish();
            return report;
        }
    };
    let (data_dir, _) = super::settings::data_dir(cli, matches, &profile);
    report.collector = match fabric_store::validated_storage_layout(&data_dir) {
        Ok(layout) => match query_running_doctor(&layout).await {
            Some(doctor) if !doctor.daemon_running => {
                Probe::new(Status::Unavailable, "collector_not_resident")
            }
            Some(doctor) if doctor.healthy => Probe::new(Status::Ready, "collector_healthy"),
            Some(_) => Probe::new(Status::NotReady, "collector_unhealthy"),
            _ => Probe::new(Status::Unavailable, "collector_unavailable"),
        },
        Err(_) => Probe::new(Status::Unavailable, "collector_data_dir_invalid"),
    };
    report.spindle = match spindle_socket(args, &profile) {
        Ok(socket) => {
            let request = if args.require_canonical {
                Request::FabricProgress
            } else {
                Request::Status
            };
            match transport::request(&socket, &request, Instant::now() + PROBE_TIMEOUT) {
                Ok(Reply::Status { status })
                    if !args.require_canonical && valid_identity(&status.origin) =>
                {
                    report.spindle_origin = Some(status.origin);
                    Probe::new(Status::Ready, "spindle_reachable")
                }
                Ok(Reply::Status { .. }) if !args.require_canonical => {
                    Probe::new(Status::Unavailable, "spindle_origin_invalid")
                }
                Ok(Reply::FabricProgress {
                    authority, origin, ..
                }) if args.require_canonical && valid_identity(&origin) => {
                    report.spindle_origin = Some(origin);
                    match authority {
                        Some(authority) if valid_identity(&authority) => {
                            report.expected_authority = Some(authority)
                        }
                        Some(_) => {
                            report.canonical =
                                Probe::new(Status::NotReady, "spindle_authority_invalid")
                        }
                        None => {
                            report.canonical =
                                Probe::new(Status::NotReady, "spindle_authority_unconfigured")
                        }
                    }
                    Probe::new(Status::Ready, "spindle_reachable")
                }
                Ok(Reply::FabricProgress { .. }) if args.require_canonical => {
                    Probe::new(Status::Unavailable, "spindle_origin_invalid")
                }
                _ => Probe::new(Status::Unavailable, "spindle_unavailable"),
            }
        }
        Err(reason) => Probe::new(Status::Unavailable, reason),
    };
    if args.require_canonical {
        report.canonical = match (
            report.spindle_origin.as_deref(),
            report.expected_authority.as_deref(),
        ) {
            (None, _) => Probe::new(Status::Unavailable, "local_origin_unavailable"),
            (Some(_), None) => report.canonical,
            (Some(origin), Some(expected)) => match canonical_connection(args) {
                Err(reason) => Probe::new(Status::Unavailable, reason),
                Ok((endpoint, credential)) => match canonical::exchange(
                    &endpoint,
                    &credential,
                    &CanonicalRequest::Progress,
                    Instant::now() + PROBE_TIMEOUT,
                ) {
                    Ok(CanonicalResponse::Progress {
                        authority,
                        frontiers,
                    }) if valid_identity(&authority) => {
                        let matches_authority = authority == expected;
                        report.canonical_authority = Some(authority);
                        match (matches_authority, frontiers.get(origin)) {
                            (false, _) => {
                                Probe::new(Status::NotReady, "canonical_authority_mismatch")
                            }
                            (true, Some(frontier)) => {
                                report.canonical_frontier = Some(*frontier);
                                Probe::new(Status::Ready, "canonical_origin_present")
                            }
                            (true, None) => {
                                Probe::new(Status::NotReady, "canonical_origin_missing")
                            }
                        }
                    }
                    Ok(CanonicalResponse::Progress { .. }) => {
                        Probe::new(Status::Unavailable, "canonical_authority_invalid")
                    }
                    _ => Probe::new(Status::Unavailable, "canonical_progress_unavailable"),
                },
            },
        };
    }
    report.finish();
    report
}

fn batch(report: &Report) -> Result<RecordBatch, String> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("version", DataType::UInt64, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("require_canonical", DataType::Boolean, false),
        Field::new("collector_status", DataType::Utf8, false),
        Field::new("collector_reason", DataType::Utf8, false),
        Field::new("spindle_status", DataType::Utf8, false),
        Field::new("spindle_reason", DataType::Utf8, false),
        Field::new("canonical_status", DataType::Utf8, false),
        Field::new("canonical_reason", DataType::Utf8, false),
        Field::new("spindle_origin", DataType::Utf8, true),
        Field::new("expected_authority", DataType::Utf8, true),
        Field::new("canonical_authority", DataType::Utf8, true),
        Field::new("canonical_frontier", DataType::UInt64, true),
        Field::new("scope", DataType::Utf8, false),
    ]));
    let text = |s: &str| -> ArrayRef { Arc::new(StringArray::from(vec![s])) };
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(UInt64Array::from(vec![u64::from(report.version)])),
            text(report.status.text()),
            Arc::new(BooleanArray::from(vec![report.require_canonical])),
            text(report.collector.status.text()),
            text(report.collector.reason),
            text(report.spindle.status.text()),
            text(report.spindle.reason),
            text(report.canonical.status.text()),
            text(report.canonical.reason),
            Arc::new(StringArray::from(vec![report.spindle_origin.as_deref()])),
            Arc::new(StringArray::from(vec![
                report.expected_authority.as_deref(),
            ])),
            Arc::new(StringArray::from(vec![
                report.canonical_authority.as_deref(),
            ])),
            Arc::new(UInt64Array::from(vec![report.canonical_frontier])),
            text(report.scope),
        ],
    )
    .map_err(|error| error.to_string())
}

pub(super) async fn run(cli: &Cli, matches: &ArgMatches, args: &Args, format: OutputFormat) -> i32 {
    let report = inspect(cli, matches, args).await;
    let output = match format {
        OutputFormat::Json => super::emit_json("production-ready", &report),
        OutputFormat::Arrow => batch(&report).and_then(|batch| {
            let bytes = encode_batches_to_ipc_stream(&batch.schema(), &[batch])?;
            super::write_stdout_bytes(&bytes)
        }),
        OutputFormat::Table => {
            println!("production-ready  {}", report.status.text());
            for (name, probe) in [
                ("collector", &report.collector),
                ("spindle", &report.spindle),
                ("canonical", &report.canonical),
            ] {
                println!("{name:<12}  {:<13}  {}", probe.status.text(), probe.reason);
            }
            if let Some(origin) = &report.spindle_origin {
                println!("origin       {origin}");
            }
            if let Some(authority) = &report.canonical_authority {
                println!("authority    {authority}");
            }
            if let Some(authority) = &report.expected_authority {
                println!("expected     {authority}");
            }
            if let Some(frontier) = report.canonical_frontier {
                println!("frontier     {frontier}");
            }
            println!("scope        {}", report.scope);
            Ok(())
        }
    };
    if let Err(error) = output {
        eprintln!("fabric: output unavailable: {error}");
        return 3;
    }
    report.exit_code()
}
