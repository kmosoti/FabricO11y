use super::{OutputFormat, emit_json};
use clap::Args;
use fabric::query::{LiveCursorRequest, LivePage};
use fabric_collect::{DaemonRequest, DaemonResponse, default_daemon_socket};
use fabric_core::StorageLayout;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Debug, Args)]
pub(super) struct Args {
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
    let page = tokio::time::timeout(Duration::from_millis(750), exchange(layout, &request))
        .await
        .map_err(|_| "daemon live cursor timed out".to_owned())??;
    validate_page(&request, &page)?;
    emit_json("live-metrics", &page)
}

async fn exchange(layout: &StorageLayout, request: &LiveCursorRequest) -> Result<LivePage, String> {
    let mut stream = UnixStream::connect(default_daemon_socket(layout))
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

fn validate_page(request: &LiveCursorRequest, page: &LivePage) -> Result<(), String> {
    if page.version != 1
        || page.origin.is_empty()
        || page.origin.len() > 128
        || page.origin.chars().any(char::is_control)
        || page.epoch.len() != 64
        || !page
            .epoch
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || request.origin.as_ref().is_some_and(|v| v != &page.origin)
        || request.epoch.as_ref().is_some_and(|v| v != &page.epoch)
        || page.entries.len() > request.count
        || page.epoch_start_after > page.observed_through
        || page.highest_lost > page.observed_through
        || page
            .oldest_retained
            .is_some_and(|v| v > page.observed_through)
        || page.gap != (request.after < page.highest_lost || request.after < page.epoch_start_after)
        || page.gap != page.gap_reason.is_some()
        || (page.highest_lost == 0) != page.highest_lost_reason.is_none()
        || page.next_after < request.after
        || page.next_after > page.observed_through
        || page.next_after
            != page
                .entries
                .last()
                .map_or(page.observed_through, |entry| entry.record_id)
        || page
            .entries
            .iter()
            .any(|entry| entry.record_id <= request.after || entry.record_id > page.next_after)
        || page
            .entries
            .windows(2)
            .any(|entries| entries[0].record_id >= entries[1].record_id)
    {
        return Err("invalid daemon live cursor response".into());
    }
    Ok(())
}
