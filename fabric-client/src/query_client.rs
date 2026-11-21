//! Authenticated `fabric-query` client using `FABRIC_CREDENTIAL_FILE` only,
//! with async `tokio::fs` credential checks and optional direct loopback TCP
//! framing over canonical `fabric::canonical` wire types.

use crate::cli_exec::{exchange_len_frame, resolve_binary};
use fabric::canonical::{
    Authenticated, MAX_FRAME, Request as CanonicalRequest, Response as CanonicalResponse,
};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::process::Command;

const QUERY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CREDENTIAL_FILE_BYTES: u64 = 4096;

/// Non-blocking client for `fabric-query` / `fabric-server` that passes
/// credentials exclusively via `FABRIC_CREDENTIAL_FILE`, strips
/// `FABRIC_CREDENTIAL` from child environments, kills timed-out subprocesses on
/// drop, and redacts token bytes from error text.
#[derive(Clone)]
pub struct FabricQueryClient {
    bin_path: Option<PathBuf>,
    endpoint: String,
    credential_file: PathBuf,
    direct_socket: bool,
}

impl std::fmt::Debug for FabricQueryClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FabricQueryClient")
            .field("bin_path", &self.bin_path)
            .field("endpoint", &self.endpoint)
            .field("credential_file", &self.credential_file)
            .field("direct_socket", &self.direct_socket)
            .finish()
    }
}

impl FabricQueryClient {
    /// Detects `fabric-query` via `FABRIC_QUERY_BIN`, `$PATH`, or `~/.local/bin/fabric-query`,
    /// preferring direct loopback TCP framing with CLI fallback.
    #[must_use]
    pub fn detect(endpoint: impl Into<String>, credential_file: impl Into<PathBuf>) -> Self {
        Self {
            bin_path: resolve_binary("FABRIC_QUERY_BIN", "fabric-query"),
            endpoint: endpoint.into(),
            credential_file: credential_file.into(),
            direct_socket: true,
        }
    }

    /// Creates a client pinned to an explicit binary path (useful for deterministic testing).
    #[must_use]
    pub fn with_bin(
        bin_path: Option<PathBuf>,
        endpoint: impl Into<String>,
        credential_file: impl Into<PathBuf>,
    ) -> Self {
        Self {
            bin_path,
            endpoint: endpoint.into(),
            credential_file: credential_file.into(),
            direct_socket: false,
        }
    }

    /// Enables or disables direct loopback TCP framing to `fabric-server`.
    #[must_use]
    pub fn with_direct_socket(mut self, enabled: bool) -> Self {
        self.direct_socket = enabled;
        self
    }

    /// Returns `true` when either direct loopback transport or `fabric-query` is available.
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.bin_path.is_some()
            || (self.direct_socket
                && self
                    .endpoint
                    .trim()
                    .parse::<SocketAddr>()
                    .is_ok_and(|addr| addr.ip().is_loopback()))
    }

    /// Returns the resolved `fabric-query` binary path, if any.
    #[must_use]
    pub fn bin_path(&self) -> Option<&Path> {
        self.bin_path.as_deref()
    }

    /// Queries canonical source frontiers via direct loopback TCP or `fabric-query progress`.
    pub async fn progress(&self) -> Result<serde_json::Value, String> {
        self.dispatch(CanonicalRequest::Progress, &["progress"])
            .await
    }

    /// Typed canonical progress response using [`fabric::canonical::Response`].
    pub async fn progress_typed(&self) -> Result<CanonicalResponse, String> {
        serde_json::from_value(self.progress().await?)
            .map_err(|e| format!("fabric-query response decode: {e}"))
    }

    /// Retrieves original canonical JSON for one record via direct TCP or `fabric-query evidence <id>`.
    pub async fn evidence(&self, canonical_id: u64) -> Result<serde_json::Value, String> {
        let id = canonical_id.to_string();
        self.dispatch(
            CanonicalRequest::Evidence { canonical_id },
            &["evidence", &id],
        )
        .await
    }

    /// Typed canonical evidence response using [`fabric::canonical::Response`].
    pub async fn evidence_typed(&self, canonical_id: u64) -> Result<CanonicalResponse, String> {
        serde_json::from_value(self.evidence(canonical_id).await?)
            .map_err(|e| format!("fabric-query response decode: {e}"))
    }

    /// Requests cancellation of an active receiver query via direct TCP or `fabric-query cancel <query_id>`.
    pub async fn cancel(&self, query_id: &str) -> Result<serde_json::Value, String> {
        let clean = query_id.trim();
        if clean.is_empty() {
            return Err("fabric-query query_id is empty".into());
        }
        self.dispatch(
            CanonicalRequest::Cancel {
                query_id: clean.to_owned(),
            },
            &["cancel", clean],
        )
        .await
    }

    /// Typed canonical cancellation response using [`fabric::canonical::Response`].
    pub async fn cancel_typed(&self, query_id: &str) -> Result<CanonicalResponse, String> {
        serde_json::from_value(self.cancel(query_id).await?)
            .map_err(|e| format!("fabric-query response decode: {e}"))
    }

    /// Sends one canonical request directly over loopback TCP using `fabric::canonical` framing.
    pub async fn exchange_direct(
        &self,
        request: CanonicalRequest,
    ) -> Result<CanonicalResponse, String> {
        let endpoint = self.endpoint.trim();
        if endpoint.is_empty() {
            return Err("fabric-query endpoint is empty".into());
        }
        let addr: SocketAddr = endpoint.parse().map_err(|_| {
            "fabric-query direct endpoint must be a loopback socket address".to_owned()
        })?;
        if !addr.ip().is_loopback() {
            return Err("fabric-query direct endpoint must be loopback".into());
        }
        let credential = self.read_validated_credential().await?;
        let payload = serde_json::to_vec(&Authenticated {
            credential: credential.clone(),
            request,
        })
        .map_err(|e| redact_token(&format!("canonical request encode: {e}"), &credential))?;
        if payload.len() > MAX_FRAME {
            return Err("canonical request exceeds MAX_FRAME".into());
        }

        let body = tokio::time::timeout(QUERY_TIMEOUT, async {
            let mut stream = TcpStream::connect(addr)
                .await
                .map_err(|e| format!("fabric-server connect: {}", e.kind()))?;
            let _ = stream.set_nodelay(true);
            exchange_len_frame(&mut stream, &payload, MAX_FRAME, "fabric-server").await
        })
        .await
        .map_err(|_| "fabric-server direct timeout".to_owned())?
        .map_err(|e| redact_token(&e, &credential))?;

        let response: CanonicalResponse = serde_json::from_slice(&body)
            .map_err(|e| redact_token(&format!("fabric-server json parse: {e}"), &credential))?;
        if let CanonicalResponse::Error { error } = response {
            return Err(redact_token(&error, &credential));
        }
        Ok(response)
    }

    async fn dispatch(
        &self,
        request: CanonicalRequest,
        cli_args: &[&str],
    ) -> Result<serde_json::Value, String> {
        if self.direct_socket {
            match self.exchange_direct(request).await {
                Ok(resp) => {
                    return serde_json::to_value(resp)
                        .map_err(|e| format!("fabric-server json encode: {e}"));
                }
                Err(direct_err) => {
                    if self.bin_path.is_none() {
                        return Err(direct_err);
                    }
                }
            }
        }
        self.invoke(cli_args, QUERY_TIMEOUT).await
    }

    async fn validate_credential_file(&self) -> Result<(), String> {
        let meta = tokio::fs::symlink_metadata(&self.credential_file)
            .await
            .map_err(|e| format!("credential file error: {}", e.kind()))?;
        if meta.file_type().is_symlink()
            || !meta.is_file()
            || meta.len() == 0
            || meta.len() > MAX_CREDENTIAL_FILE_BYTES
        {
            return Err("credential file is missing, symlinked, empty, or oversized".into());
        }
        Ok(())
    }

    async fn read_validated_credential(&self) -> Result<String, String> {
        self.validate_credential_file().await?;
        let file = tokio::fs::File::open(&self.credential_file)
            .await
            .map_err(|e| format!("credential file open error: {}", e.kind()))?;
        let mut raw = String::new();
        file.take(MAX_CREDENTIAL_FILE_BYTES + 1)
            .read_to_string(&mut raw)
            .await
            .map_err(|e| format!("credential file read error: {}", e.kind()))?;
        let trimmed = raw.trim();
        if trimmed.is_empty() || raw.len() as u64 > MAX_CREDENTIAL_FILE_BYTES {
            return Err("credential file is empty or oversized".into());
        }
        Ok(trimmed.to_owned())
    }

    pub(crate) async fn invoke(
        &self,
        args: &[&str],
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let Some(bin) = &self.bin_path else {
            return Err("fabric-query binary not found".into());
        };
        if self.endpoint.trim().is_empty() {
            return Err("fabric-query endpoint is empty".into());
        }
        self.validate_credential_file().await?;

        let mut cmd = self.build_command(bin, args);
        let out = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| "fabric-query timeout".to_owned())?
            .map_err(|e| format!("fabric-query spawn failed: {}", e.kind()))?;

        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let sanitized = self.redact_secret(stderr.trim()).await;
            return Err(if sanitized.is_empty() {
                format!("fabric-query exit {}", out.status)
            } else {
                sanitized.chars().take(120).collect()
            });
        }

        match serde_json::from_slice(&out.stdout) {
            Ok(val) => Ok(val),
            Err(e) => Err(self
                .redact_secret(&format!("fabric-query json parse: {e}"))
                .await),
        }
    }

    fn build_command(&self, bin: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(bin);
        cmd.args(args)
            .env("FABRIC_ENDPOINT", self.endpoint.trim())
            .env("FABRIC_CREDENTIAL_FILE", &self.credential_file)
            .env_remove("FABRIC_CREDENTIAL")
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    async fn redact_secret(&self, text: &str) -> String {
        let mut out = text.to_owned();
        if let Ok(file) = tokio::fs::File::open(&self.credential_file).await {
            let mut raw = String::new();
            if file
                .take(MAX_CREDENTIAL_FILE_BYTES + 1)
                .read_to_string(&mut raw)
                .await
                .is_ok()
            {
                let trimmed = raw.trim();
                if !trimmed.is_empty() {
                    out = out.replace(trimmed, "[REDACTED]");
                }
                if !raw.is_empty() {
                    out = out.replace(&raw, "[REDACTED]");
                }
            }
        }
        out
    }
}

fn redact_token(text: &str, token: &str) -> String {
    if token.is_empty() {
        text.to_owned()
    } else {
        text.replace(token, "[REDACTED]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn build_command_sets_credential_file_and_strips_raw_credential_env() {
        let client = FabricQueryClient::with_bin(
            Some(PathBuf::from("/usr/bin/true")),
            "127.0.0.1:17440",
            "/data/fabrico11y/reader.token",
        );
        let cmd = client.build_command(Path::new("/usr/bin/true"), &["progress"]);
        let envs: Vec<(&OsStr, Option<&OsStr>)> = cmd.as_std().get_envs().collect();
        assert!(envs.contains(&(
            OsStr::new("FABRIC_ENDPOINT"),
            Some(OsStr::new("127.0.0.1:17440"))
        )));
        assert!(envs.contains(&(
            OsStr::new("FABRIC_CREDENTIAL_FILE"),
            Some(OsStr::new("/data/fabrico11y/reader.token"))
        )));
        assert!(envs.contains(&(OsStr::new("FABRIC_CREDENTIAL"), None)));
    }
}
