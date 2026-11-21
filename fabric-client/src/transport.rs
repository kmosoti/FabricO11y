//! Persistent Unix domain socket transport for `FabricClient` with connection
//! reuse, bounded backoff, and canonical `fabric::spindle` wire framing.

use crate::cli_exec::exchange_len_frame;
use crate::schema::{
    FabricBlame, FabricCustody, FabricDoctor, FabricDrift, FabricLineage, FabricMetricRow,
    FabricSnapshot,
};
use fabric::spindle::{
    Health as SpindleHealth, MAX_REPLY, Reply as SpindleReply, Request as SpindleRequest,
};
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

const MAX_DAEMON_REPLY_BYTES: usize = 4 * 1024 * 1024;
const CONNECT_BACKOFF_MS: [u64; 3] = [10, 25, 50];

/// Transport strategy used by [`crate::FabricClient`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FabricTransport {
    /// Invoke `fabric ... --json` subprocesses per request.
    #[default]
    Cli,
    /// Reuse a persistent Unix domain socket to `daemon.sock` (and optional
    /// `spindle.sock`), skipping CLI subprocess forks when reachable.
    DirectSocket {
        daemon_socket: PathBuf,
        spindle_socket: Option<PathBuf>,
        fallback_to_cli: bool,
    },
}

/// Reusable async UDS connection to `daemon.sock`.
#[derive(Debug, Clone, Default)]
pub(crate) struct DaemonSocketPool {
    conn: Arc<Mutex<Option<BufReader<UnixStream>>>>,
}

impl DaemonSocketPool {
    pub(crate) async fn exchange<T: DeserializeOwned>(
        &self,
        socket_path: &Path,
        request_json: &str,
        expected_kind: &str,
        timeout: Duration,
    ) -> Result<T, String> {
        let val = self
            .exchange_value(socket_path, request_json, expected_kind, timeout)
            .await?;
        serde_json::from_value(val).map_err(|e| format!("daemon rpc decode ({expected_kind}): {e}"))
    }

    pub(crate) async fn exchange_hot_metrics(
        &self,
        socket_path: &Path,
        since_ms: i64,
        timeout: Duration,
    ) -> Result<Vec<FabricMetricRow>, String> {
        let req = serde_json::json!({ "op": "hot_metrics", "since_ms": since_ms }).to_string();
        let val = self
            .exchange_value(socket_path, &req, "hot_metrics", timeout)
            .await?;
        let points = val
            .get("points")
            .cloned()
            .ok_or_else(|| "daemon rpc hot_metrics missing points".to_owned())?;
        serde_json::from_value(points).map_err(|e| format!("daemon rpc hot_metrics decode: {e}"))
    }

    async fn exchange_value(
        &self,
        socket_path: &Path,
        request_json: &str,
        expected_kind: &str,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let mut guard = self.conn.lock().await;
        let mut last_err = String::new();
        for attempt in 0..2 {
            if guard.is_none() {
                match connect_with_backoff(socket_path, timeout).await {
                    Ok(stream) => *guard = Some(BufReader::with_capacity(8192, stream)),
                    Err(e) => return Err(e),
                }
            }
            let reader = guard.as_mut().expect("connected reader");
            match round_trip_line(reader, request_json, timeout).await {
                Ok(line) => {
                    let val: serde_json::Value = serde_json::from_str(line.trim())
                        .map_err(|e| format!("daemon rpc json parse: {e}"))?;
                    match val.get("kind").and_then(|k| k.as_str()) {
                        Some("error") => {
                            let msg = val
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("daemon error");
                            return Err(msg.to_owned());
                        }
                        Some(kind) if kind == expected_kind => return Ok(val),
                        other => {
                            return Err(format!(
                                "unexpected daemon rpc reply kind {other:?} (expected {expected_kind:?})"
                            ));
                        }
                    }
                }
                Err(e) => {
                    *guard = None;
                    last_err = e;
                    if attempt == 0 {
                        tokio::time::sleep(Duration::from_millis(CONNECT_BACKOFF_MS[0])).await;
                    }
                }
            }
        }
        Err(last_err)
    }

    pub(crate) async fn snapshot(
        &self,
        sock: &Path,
        timeout: Duration,
    ) -> Result<FabricSnapshot, String> {
        self.exchange(sock, r#"{"op":"snapshot"}"#, "snapshot", timeout)
            .await
    }

    pub(crate) async fn doctor(
        &self,
        sock: &Path,
        timeout: Duration,
    ) -> Result<FabricDoctor, String> {
        self.exchange(sock, r#"{"op":"doctor"}"#, "doctor", timeout)
            .await
    }

    pub(crate) async fn blame(
        &self,
        sock: &Path,
        resource: &str,
        timeout: Duration,
    ) -> Result<FabricBlame, String> {
        let req = serde_json::json!({ "op": "blame", "resource": resource }).to_string();
        self.exchange(sock, &req, "blame", timeout).await
    }

    pub(crate) async fn drift(
        &self,
        sock: &Path,
        timeout: Duration,
    ) -> Result<FabricDrift, String> {
        self.exchange(sock, r#"{"op":"drift","pillar":null}"#, "drift", timeout)
            .await
    }

    pub(crate) async fn lineage(
        &self,
        sock: &Path,
        target: &str,
        timeout: Duration,
    ) -> Result<FabricLineage, String> {
        let req = serde_json::json!({
            "op": "lineage",
            "target": target,
            "direction": "downstream",
            "max_depth": 2
        })
        .to_string();
        self.exchange(sock, &req, "lineage", timeout).await
    }
}

async fn connect_with_backoff(path: &Path, timeout: Duration) -> Result<UnixStream, String> {
    let mut last_err = String::new();
    for (idx, delay_ms) in CONNECT_BACKOFF_MS.iter().enumerate() {
        match tokio::time::timeout(timeout, UnixStream::connect(path)).await {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(e)) => {
                let transient = matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::ConnectionAborted
                );
                last_err = format!("daemon socket connect ({}): {}", path.display(), e.kind());
                if !transient || idx + 1 == CONNECT_BACKOFF_MS.len() {
                    return Err(last_err);
                }
            }
            Err(_) => {
                return Err(format!(
                    "daemon socket connect timeout ({})",
                    path.display()
                ));
            }
        }
        tokio::time::sleep(Duration::from_millis(*delay_ms)).await;
    }
    Err(last_err)
}

async fn round_trip_line(
    reader: &mut BufReader<UnixStream>,
    request_json: &str,
    timeout: Duration,
) -> Result<String, String> {
    tokio::time::timeout(timeout, async {
        let mut payload = Vec::with_capacity(request_json.len() + 1);
        payload.extend_from_slice(request_json.as_bytes());
        payload.push(b'\n');
        reader
            .get_mut()
            .write_all(&payload)
            .await
            .map_err(|e| format!("daemon socket write: {}", e.kind()))?;
        reader
            .get_mut()
            .flush()
            .await
            .map_err(|e| format!("daemon socket flush: {}", e.kind()))?;

        let mut buf = Vec::new();
        loop {
            let chunk = reader
                .fill_buf()
                .await
                .map_err(|e| format!("daemon socket read: {}", e.kind()))?;
            if chunk.is_empty() {
                if buf.is_empty() {
                    return Err("daemon socket closed before reply".into());
                }
                break;
            }
            if let Some(pos) = chunk.iter().position(|&b| b == b'\n') {
                if buf.len().saturating_add(pos) > MAX_DAEMON_REPLY_BYTES {
                    return Err("daemon socket reply exceeded size limit".into());
                }
                buf.extend_from_slice(&chunk[..pos]);
                reader.consume(pos + 1);
                break;
            }
            let len = chunk.len();
            if buf.len().saturating_add(len) > MAX_DAEMON_REPLY_BYTES {
                return Err("daemon socket reply exceeded size limit".into());
            }
            buf.extend_from_slice(chunk);
            reader.consume(len);
        }
        String::from_utf8(buf).map_err(|e| format!("daemon socket utf8: {e}"))
    })
    .await
    .map_err(|_| "daemon socket timeout".to_owned())?
}

/// Queries Spindle directly over its length-prefixed UDS using canonical
/// [`fabric::spindle::Request::Status`] and [`fabric::spindle::Reply::Status`].
pub(crate) async fn query_spindle_custody(
    spindle_socket: &Path,
    timeout: Duration,
) -> Result<FabricCustody, String> {
    let req_bytes =
        serde_json::to_vec(&SpindleRequest::Status).map_err(|e| format!("spindle encode: {e}"))?;
    let reply_bytes = tokio::time::timeout(timeout, async {
        let mut stream = UnixStream::connect(spindle_socket)
            .await
            .map_err(|e| format!("spindle connect: {}", e.kind()))?;
        exchange_len_frame(&mut stream, &req_bytes, MAX_REPLY, "spindle").await
    })
    .await
    .map_err(|_| "spindle socket timeout".to_owned())??;

    match serde_json::from_slice::<SpindleReply>(&reply_bytes)
        .map_err(|e| format!("spindle decode: {e}"))?
    {
        SpindleReply::Status { status } => Ok(spindle_health_to_custody(spindle_socket, status)),
        SpindleReply::Error { error } => Err(error),
        _ => Err("unexpected spindle status reply".into()),
    }
}

fn spindle_health_to_custody(socket: &Path, status: SpindleHealth) -> FabricCustody {
    FabricCustody {
        spindle_socket: socket.display().to_string(),
        spindle_online: true,
        spindle_origin: (!status.origin.is_empty()).then_some(status.origin),
        spindle_error: None,
        records: status.records,
        highest_record_id: status.highest_record_id,
        canonical_through: status.canonical_through,
        reclaimed_through: status.reclaimed_through,
        forwarding_error: status.forwarding_error,
        server_endpoint: None,
        server_online: false,
        server_authority: None,
        server_error: None,
        server_frontiers: BTreeMap::new(),
        collector_spindle_watermark_id: None,
        catalog_error: None,
    }
}

/// Discovers an existing local `daemon.sock` (and optional `spindle.sock`) for
/// zero-fork [`FabricTransport::DirectSocket`] collection with CLI fallback.
pub(crate) fn discover_direct_socket(data_dir: Option<&Path>) -> Option<FabricTransport> {
    let daemon_socket = if let Some(raw) = std::env::var_os("FABRIC_DAEMON_SOCKET") {
        let p = PathBuf::from(raw);
        p.exists().then_some(p)
    } else if let Some(dir) = data_dir {
        let p = dir.join("state").join("daemon.sock");
        p.exists().then_some(p)
    } else {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(|xdg| PathBuf::from(xdg).join("fabrico11y").join("daemon.sock"))
            .filter(|p| p.exists())
    }?;

    let spindle_socket = std::env::var_os("SPINDLE_SOCKET")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_RUNTIME_DIR")
                .map(|xdg| PathBuf::from(xdg).join("spindle").join("spindle.sock"))
        })
        .filter(|p| p.exists());

    Some(FabricTransport::DirectSocket {
        daemon_socket,
        spindle_socket,
        fallback_to_cli: true,
    })
}
