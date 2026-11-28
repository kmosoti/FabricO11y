use super::{OutputFormat, emit_json};
use clap::Args;
use fabric::query::{LiveCursorRequest, LivePage};
use fabric_collect::{DaemonRequest, DaemonResponse, default_daemon_socket};
use fabric_core::StorageLayout;
use fabric_store::validate_non_tmp_path_on_disk;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Debug, Args)]
pub(super) struct Args {
    /// Existing daemon socket; defaults to the regular daemon socket path.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Origin from the previous page; omit both origin and epoch for a fresh cursor.
    #[arg(long, requires = "epoch")]
    origin: Option<String>,
    /// Process epoch from the previous page.
    #[arg(long, requires = "origin")]
    epoch: Option<String>,
    /// Resume after this source record ID.
    #[arg(long, default_value_t = 0)]
    after: u64,
    /// Maximum entries (1 through 64).
    #[arg(long, default_value_t = 64)]
    count: usize,
    /// Maximum encoded daemon reply bytes including newline (1024 through 32768).
    #[arg(long, default_value_t = 32768)]
    max_bytes: usize,
}

pub(super) async fn run(
    layout: &StorageLayout,
    args: &Args,
    format: OutputFormat,
) -> Result<(), String> {
    if format != OutputFormat::Json {
        return Err("live-metrics requires JSON output; pass --json".into());
    }
    let request = LiveCursorRequest {
        origin: args.origin.clone(),
        epoch: args.epoch.clone(),
        after: args.after,
        count: args.count,
        max_bytes: args.max_bytes,
    };
    request.validate().map_err(str::to_owned)?;
    let socket = match &args.socket {
        Some(path) => {
            let path = validate_non_tmp_path_on_disk(path).map_err(|e| e.to_string())?;
            if !std::fs::metadata(&path)
                .map_err(|e| format!("daemon socket: {}", e.kind()))?
                .file_type()
                .is_socket()
            {
                return Err("daemon socket path is not a socket".into());
            }
            path
        }
        None => default_daemon_socket(layout),
    };
    let page = tokio::time::timeout(Duration::from_millis(750), exchange(&socket, &request))
        .await
        .map_err(|_| "daemon live cursor timed out".to_owned())??;
    page.validate_for(&request).map_err(str::to_owned)?;
    emit_json("live-metrics", &page)
}

async fn exchange(socket: &Path, request: &LiveCursorRequest) -> Result<LivePage, String> {
    let mut stream = UnixStream::connect(socket)
        .await
        .map_err(|e| format!("daemon socket connect: {}", e.kind()))?;
    let wire = DaemonRequest::LiveMetricsV1 {
        origin: request.origin.clone(),
        epoch: request.epoch.clone(),
        after: request.after,
        count: request.count,
        max_bytes: request.max_bytes,
    };
    let mut encoded = serde_json::to_vec(&wire).map_err(|e| format!("daemon request: {e}"))?;
    encoded.push(b'\n');
    stream
        .write_all(&encoded)
        .await
        .map_err(|e| format!("daemon socket write: {}", e.kind()))?;
    let mut bytes = Vec::new();
    BufReader::new(stream)
        .take((request.max_bytes + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .await
        .map_err(|e| format!("daemon socket read: {}", e.kind()))?;
    if bytes.len() > request.max_bytes || bytes.last() != Some(&b'\n') {
        return Err("invalid or oversized daemon cursor reply".into());
    }
    let reply: DaemonResponse =
        serde_json::from_slice(&bytes).map_err(|e| format!("daemon cursor decode: {e}"))?;
    match reply {
        DaemonResponse::LiveMetricsV1(page) => Ok(page),
        DaemonResponse::Error { message } => Err(message),
        _ => Err("unexpected daemon cursor reply".into()),
    }
}
