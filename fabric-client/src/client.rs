//! Async non-blocking `FabricO11y` client with persistent `DirectSocket` UDS
//! reuse and deterministic `fabric` CLI fallback.

pub use crate::query_client::FabricQueryClient;
pub use crate::schema::{EXPECTED_SCHEMA_VERSION, parse_envelope};
pub use crate::transport::FabricTransport;

use crate::cli_exec::{
    assemble_bundle, collect_via_cli, query_mem_history_cli, resolve_binary, run_json,
};
use crate::schema::{FabricBlame, FabricBundle, FabricCustody, FabricDoctor, FabricLineage};
use crate::transport::{DaemonSocketPool, discover_direct_socket, query_spindle_custody};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FAST_TIMEOUT: Duration = Duration::from_secs(3);
const QUERY_TIMEOUT: Duration = Duration::from_secs(9);

/// Non-blocking `FabricO11y` client supporting persistent UDS `DirectSocket`
/// transport (zero process forks) alongside deterministic `fabric --json` CLI fallback.
#[derive(Debug, Clone)]
pub struct FabricClient {
    bin_path: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    transport: FabricTransport,
    pool: DaemonSocketPool,
}

impl FabricClient {
    /// Detects the local `daemon.sock` (preferring `DirectSocket` with CLI fallback)
    /// and resolves `fabric` via `FABRIC_BIN`, `$PATH`, or `~/.local/bin/fabric`.
    #[must_use]
    pub fn detect(data_dir: Option<PathBuf>) -> Self {
        let transport = discover_direct_socket(data_dir.as_deref()).unwrap_or(FabricTransport::Cli);
        Self {
            bin_path: resolve_binary("FABRIC_BIN", "fabric"),
            data_dir,
            transport,
            pool: DaemonSocketPool::default(),
        }
    }

    /// Creates a CLI-mode client pinned to an explicit binary path (for deterministic testing).
    #[must_use]
    pub fn with_bin(bin_path: Option<PathBuf>, data_dir: Option<PathBuf>) -> Self {
        Self {
            bin_path,
            data_dir,
            transport: FabricTransport::Cli,
            pool: DaemonSocketPool::default(),
        }
    }

    /// Creates a `DirectSocket` client targeting `daemon.sock` (and optional `spindle.sock`).
    #[must_use]
    pub fn direct_socket(
        daemon_socket: impl Into<PathBuf>,
        spindle_socket: Option<PathBuf>,
    ) -> Self {
        Self {
            bin_path: resolve_binary("FABRIC_BIN", "fabric"),
            data_dir: None,
            transport: FabricTransport::DirectSocket {
                daemon_socket: daemon_socket.into(),
                spindle_socket,
                fallback_to_cli: true,
            },
            pool: DaemonSocketPool::default(),
        }
    }

    /// Overrides the transport strategy on this client.
    #[must_use]
    pub fn with_transport(mut self, transport: FabricTransport) -> Self {
        self.transport = transport;
        self
    }

    /// Returns the active [`FabricTransport`].
    #[must_use]
    pub fn transport(&self) -> &FabricTransport {
        &self.transport
    }

    /// Returns `true` when either a direct socket is configured or `fabric` was resolved.
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.bin_path.is_some() || matches!(self.transport, FabricTransport::DirectSocket { .. })
    }

    /// Returns the resolved `fabric` binary path, if any.
    #[must_use]
    pub fn bin_path(&self) -> Option<&Path> {
        self.bin_path.as_deref()
    }

    /// Collects `snapshot`, `custody`, `blame`, `drift`, and `doctor`.
    /// Preserves partial failures in `warnings` whenever `online == true`.
    pub async fn collect(&self) -> FabricBundle {
        self.collect_inner(true).await
    }

    /// Lightweight background poll running only `snapshot` and `custody`.
    pub async fn collect_fast(&self) -> FabricBundle {
        self.collect_inner(false).await
    }

    async fn collect_inner(&self, extended: bool) -> FabricBundle {
        if let FabricTransport::DirectSocket {
            daemon_socket,
            spindle_socket,
            fallback_to_cli,
        } = &self.transport
        {
            let direct = self
                .collect_via_socket(daemon_socket, spindle_socket.as_deref(), extended)
                .await;
            if direct.online || !fallback_to_cli || self.bin_path.is_none() {
                return direct;
            }
        }
        collect_via_cli(
            self.bin_path.as_deref(),
            &self.data_dir,
            extended,
            FAST_TIMEOUT,
        )
        .await
    }

    async fn collect_via_socket(
        &self,
        daemon_socket: &Path,
        spindle_socket: Option<&Path>,
        extended: bool,
    ) -> FabricBundle {
        let snap_res = self.pool.snapshot(daemon_socket, FAST_TIMEOUT).await;
        let cust_res = match spindle_socket {
            Some(spindle) => query_spindle_custody(spindle, FAST_TIMEOUT).await,
            None => match &self.bin_path {
                Some(bin) => {
                    run_json::<FabricCustody>(
                        bin,
                        &self.data_dir,
                        &["custody", "--json"],
                        FAST_TIMEOUT,
                    )
                    .await
                }
                None => Err("spindle socket not configured".into()),
            },
        };
        let (blame_res, drift_res, doc_res) = if extended {
            (
                self.pool.blame(daemon_socket, "memory", FAST_TIMEOUT).await,
                self.pool.drift(daemon_socket, FAST_TIMEOUT).await,
                self.pool.doctor(daemon_socket, FAST_TIMEOUT).await,
            )
        } else {
            (
                Err("skipped on fast poll".into()),
                Err("skipped on fast poll".into()),
                Err("skipped on fast poll".into()),
            )
        };
        assemble_bundle(
            daemon_socket.display().to_string(),
            extended,
            snap_res,
            cust_res,
            blame_res,
            drift_res,
            doc_res,
        )
    }

    /// Reads the opt-in local live cursor. This never falls back to timestamp history.
    pub async fn try_query_live_metrics(
        &self,
        request: &fabric_query_contract::LiveCursorRequest,
    ) -> Result<fabric_query_contract::LivePage, String> {
        match &self.transport {
            FabricTransport::DirectSocket { daemon_socket, .. } => tokio::time::timeout(
                QUERY_TIMEOUT,
                self.pool.live_cursor(daemon_socket, request, QUERY_TIMEOUT),
            )
            .await
            .map_err(|_| "daemon live cursor timed out".to_owned())?,
            FabricTransport::Cli => Err("live cursor requires direct daemon socket".into()),
        }
    }

    /// Queries metric history (`host.mem.available_bytes`), preferring hot UDS metrics when available.
    pub async fn try_query_mem_history(&self) -> Result<Vec<(u64, f64)>, String> {
        if let FabricTransport::DirectSocket {
            daemon_socket,
            fallback_to_cli,
            ..
        } = &self.transport
        {
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64);
            let since_ms = now_ms.saturating_sub(5 * 60 * 1000);
            match self
                .pool
                .exchange_hot_metrics(daemon_socket, since_ms, FAST_TIMEOUT)
                .await
            {
                Ok(rows) => {
                    let mut series: Vec<(u64, f64)> = rows
                        .into_iter()
                        .filter(|r| r.name == "host.mem.available_bytes")
                        .map(|r| (r.timestamp_ms, r.value))
                        .collect();
                    series.sort_by_key(|&(ts, _)| ts);
                    if series.len() > 24 {
                        series = series.split_off(series.len() - 24);
                    }
                    if !series.is_empty() || !fallback_to_cli || self.bin_path.is_none() {
                        return Ok(series);
                    }
                }
                Err(err) if !fallback_to_cli || self.bin_path.is_none() => return Err(err),
                Err(_) => {}
            }
        }
        let bin = self
            .bin_path
            .as_deref()
            .ok_or_else(|| "fabric binary not found".to_owned())?;
        query_mem_history_cli(bin, &self.data_dir, QUERY_TIMEOUT).await
    }

    /// Best-effort wrapper over [`FabricClient::try_query_mem_history`].
    pub async fn query_mem_history(&self) -> Vec<(u64, f64)> {
        self.try_query_mem_history().await.unwrap_or_default()
    }

    /// Collects blame for `"memory"`, `"cpu"`, `"swap"`, `"tmpfs"`, or `"disk:<mount>"`.
    pub async fn collect_blame(&self, resource: &str) -> Result<FabricBlame, String> {
        let res = resource.trim();
        let valid = matches!(res, "memory" | "cpu" | "swap" | "tmpfs")
            || res.strip_prefix("disk:").is_some_and(|m| !m.is_empty());
        if !valid {
            return Err(format!("unsupported blame resource {resource:?}"));
        }
        if let FabricTransport::DirectSocket {
            daemon_socket,
            fallback_to_cli,
            ..
        } = &self.transport
        {
            match self.pool.blame(daemon_socket, res, FAST_TIMEOUT).await {
                Ok(b) => return Ok(b),
                Err(err) if !fallback_to_cli || self.bin_path.is_none() => return Err(err),
                Err(_) => {}
            }
        }
        let bin = self
            .bin_path
            .as_deref()
            .ok_or_else(|| "fabric binary not found".to_owned())?;
        run_json::<FabricBlame>(bin, &self.data_dir, &["blame", res, "--json"], FAST_TIMEOUT).await
    }

    /// Collects downstream lineage (depth 2) for `target`.
    pub async fn collect_lineage(&self, target: &str) -> Result<FabricLineage, String> {
        let clean = target.trim();
        if clean.is_empty() {
            return Err("lineage target must not be empty".into());
        }
        if let FabricTransport::DirectSocket {
            daemon_socket,
            fallback_to_cli,
            ..
        } = &self.transport
        {
            match self.pool.lineage(daemon_socket, clean, FAST_TIMEOUT).await {
                Ok(lin) => return Ok(lin),
                Err(err) if !fallback_to_cli || self.bin_path.is_none() => return Err(err),
                Err(_) => {}
            }
        }
        let bin = self
            .bin_path
            .as_deref()
            .ok_or_else(|| "fabric binary not found".to_owned())?;
        run_json::<FabricLineage>(
            bin,
            &self.data_dir,
            &[
                "lineage",
                clean,
                "--direction",
                "downstream",
                "--depth",
                "2",
                "--json",
            ],
            FAST_TIMEOUT,
        )
        .await
    }

    /// Collects self-preservation `doctor` report.
    pub async fn collect_doctor(&self) -> Result<FabricDoctor, String> {
        if let FabricTransport::DirectSocket {
            daemon_socket,
            fallback_to_cli,
            ..
        } = &self.transport
        {
            match self.pool.doctor(daemon_socket, FAST_TIMEOUT).await {
                Ok(doc) => return Ok(doc),
                Err(err) if !fallback_to_cli || self.bin_path.is_none() => return Err(err),
                Err(_) => {}
            }
        }
        let bin = self
            .bin_path
            .as_deref()
            .ok_or_else(|| "fabric binary not found".to_owned())?;
        run_json::<FabricDoctor>(bin, &self.data_dir, &["doctor", "--json"], FAST_TIMEOUT).await
    }
}
