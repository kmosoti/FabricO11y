//! Authenticated `fabric-query` client using `FABRIC_CREDENTIAL_FILE` only,
//! with async `tokio::fs` credential checks and optional direct loopback TCP
//! framing over canonical `fabric::canonical` wire types.

use crate::cli_exec::{read_len_frame, resolve_binary, write_len_frame};
use fabric::canonical::{
    AUTH_HANDSHAKE_VERSION, ClientHello, MAX_FRAME, MAX_HANDSHAKE_FRAME, PlanPageRequest,
    PlanScope, Request as CanonicalRequest, Response as CanonicalResponse, ServerChallenge,
    build_client_proof, random_nonce_hex, verify_server_challenge,
};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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

    /// Reads one scoped frozen plan page. A failed selected transport is never retried.
    pub async fn plan_page(&self, request: PlanPageRequest) -> Result<CanonicalResponse, String> {
        self.dispatch_plan(CanonicalRequest::PlanPage { request }, "plan-page")
            .await
    }
    /// Creates one advisory plan subscription; it is not retried after dispatch.
    pub async fn subscribe_plan(&self, scope: PlanScope) -> Result<CanonicalResponse, String> {
        self.dispatch_plan(CanonicalRequest::SubscribePlan { scope }, "subscribe-plan")
            .await
    }
    /// Consumes at most one advisory hint; it is not retried after dispatch.
    pub async fn poll_plan_hint(
        &self,
        scope: PlanScope,
        subscription: String,
    ) -> Result<CanonicalResponse, String> {
        self.dispatch_plan(
            CanonicalRequest::PollPlanHint {
                scope,
                subscription,
            },
            "poll-plan-hint",
        )
        .await
    }
    async fn dispatch_plan(
        &self,
        request: CanonicalRequest,
        command: &str,
    ) -> Result<CanonicalResponse, String> {
        // Bound caller-owned strings before cloning or creating either wire form.
        let raw_len = match &request {
            CanonicalRequest::PlanPage { request } => request
                .scope
                .0
                .len()
                .checked_add(request.cursor.as_ref().map_or(0, String::len)),
            CanonicalRequest::SubscribePlan { scope } => Some(scope.0.len()),
            CanonicalRequest::PollPlanHint {
                scope,
                subscription,
            } => scope.0.len().checked_add(subscription.len()),
            _ => return Err("invalid plan request".into()),
        }
        .ok_or("canonical request exceeds MAX_FRAME")?;
        if raw_len > MAX_FRAME {
            return Err("canonical request exceeds MAX_FRAME".into());
        }
        let expected = match &request {
            CanonicalRequest::PlanPage { request } => PlanExpectation::Page(request.scope.clone()),
            CanonicalRequest::SubscribePlan { .. } => PlanExpectation::Subscribe,
            CanonicalRequest::PollPlanHint { .. } => PlanExpectation::Poll,
            _ => return Err("invalid plan request".into()),
        };
        let direct = self.direct_socket
            && self
                .endpoint
                .trim()
                .parse::<SocketAddr>()
                .is_ok_and(|addr| addr.ip().is_loopback());
        let response = if direct {
            self.exchange_direct(request).await?
        } else {
            // The CLI receives only its strict inner object; still check the
            // canonical frame size that the CLI will ultimately transmit.
            let frame = serde_json::to_vec(&request)
                .map_err(|e| format!("canonical request encode: {e}"))?;
            if frame.len() > MAX_FRAME {
                return Err("canonical request exceeds MAX_FRAME".into());
            }
            let input = match &request {
                CanonicalRequest::PlanPage { request } => serde_json::to_vec(request),
                CanonicalRequest::SubscribePlan { scope } => {
                    serde_json::to_vec(&serde_json::json!({"scope":scope}))
                }
                CanonicalRequest::PollPlanHint {
                    scope,
                    subscription,
                } => serde_json::to_vec(
                    &serde_json::json!({"scope":scope,"subscription":subscription}),
                ),
                _ => return Err("invalid plan request".into()),
            }
            .map_err(|e| format!("plan input encode: {e}"))?;
            if input.len() > MAX_FRAME {
                return Err("canonical request exceeds MAX_FRAME".into());
            }
            self.invoke_plan(command, &input).await?
        };
        validate_plan_response(expected, response)
    }
    async fn invoke_plan(&self, command: &str, input: &[u8]) -> Result<CanonicalResponse, String> {
        let bin = self
            .bin_path
            .as_ref()
            .ok_or("fabric-query binary not found")?;
        if self.endpoint.trim().is_empty() {
            return Err("fabric-query endpoint is empty".into());
        }
        self.validate_credential_file().await?;
        let mut cmd = self.build_command(bin, &[command]);
        cmd.stdin(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("fabric-query spawn failed: {}", e.kind()))?;
        let mut stdin = child.stdin.take().ok_or("fabric-query stdin missing")?;
        let stdout = child.stdout.take().ok_or("fabric-query stdout missing")?;
        let stderr = child.stderr.take().ok_or("fabric-query stderr missing")?;
        let (write, out, err, status) = tokio::time::timeout(QUERY_TIMEOUT, async {
            tokio::join!(
                async move {
                    let result = async {
                        stdin.write_all(input).await?;
                        stdin.shutdown().await
                    }
                    .await;
                    drop(stdin);
                    result
                },
                async {
                    let mut b = Vec::new();
                    stdout
                        .take((MAX_FRAME + 1) as u64)
                        .read_to_end(&mut b)
                        .await
                        .map(|_| b)
                },
                async {
                    let mut b = Vec::new();
                    stderr
                        .take((MAX_FRAME + 1) as u64)
                        .read_to_end(&mut b)
                        .await
                        .map(|_| b)
                },
                child.wait(),
            )
        })
        .await
        .map_err(|_| "fabric-query timeout".to_owned())?;
        write.map_err(|e| format!("fabric-query stdin: {}", e.kind()))?;
        let out = out.map_err(|e| format!("fabric-query stdout: {}", e.kind()))?;
        let err = err.map_err(|e| format!("fabric-query stderr: {}", e.kind()))?;
        let status = status.map_err(|e| format!("fabric-query wait: {}", e.kind()))?;
        if out.len() > MAX_FRAME || err.len() > MAX_FRAME {
            return Err("fabric-query response exceeds MAX_FRAME".into());
        }
        if !status.success() {
            let text = self
                .redact_secret(String::from_utf8_lossy(&err).trim())
                .await;
            return Err(if text.is_empty() {
                format!("fabric-query exit {status}")
            } else {
                text.chars().take(120).collect()
            });
        }
        let response: CanonicalResponse = serde_json::from_slice(&out)
            .map_err(|e| format!("fabric-query response decode: {e}"))?;
        if let CanonicalResponse::Error { error } = response {
            return Err(self.redact_secret(&error).await);
        }
        Ok(response)
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
        let client_nonce = random_nonce_hex()?;
        let hello_bytes = serde_json::to_vec(&ClientHello {
            version: AUTH_HANDSHAKE_VERSION,
            client_nonce: client_nonce.clone(),
        })
        .map_err(|e| format!("canonical handshake encode: {e}"))?;
        let request_bytes = serde_json::to_vec(&request)
            .map_err(|e| redact_token(&format!("canonical request encode: {e}"), &credential))?;
        if request_bytes.len() > MAX_FRAME {
            return Err("canonical request exceeds MAX_FRAME".into());
        }

        let body = tokio::time::timeout(QUERY_TIMEOUT, async {
            let mut stream = TcpStream::connect(addr)
                .await
                .map_err(|e| format!("fabric-server connect: {}", e.kind()))?;
            let _ = stream.set_nodelay(true);
            write_len_frame(&mut stream, &hello_bytes, "fabric-server").await?;
            let challenge_bytes =
                read_len_frame(&mut stream, MAX_HANDSHAKE_FRAME, "fabric-server").await?;
            let challenge: ServerChallenge = serde_json::from_slice(&challenge_bytes)
                .map_err(|e| format!("fabric-server challenge parse: {e}"))?;
            verify_server_challenge(&credential, &client_nonce, &challenge)?;

            let proof = build_client_proof(
                &credential,
                &client_nonce,
                &challenge.server_nonce,
                &request_bytes,
            );
            let proof_bytes =
                serde_json::to_vec(&proof).map_err(|e| format!("canonical proof encode: {e}"))?;
            write_len_frame(&mut stream, &proof_bytes, "fabric-server").await?;
            write_len_frame(&mut stream, &request_bytes, "fabric-server").await?;
            read_len_frame(&mut stream, MAX_FRAME, "fabric-server").await
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
        let _ = self.read_validated_credential().await?;
        Ok(())
    }

    async fn read_validated_credential(&self) -> Result<String, String> {
        let path = self.credential_file.clone();
        tokio::task::spawn_blocking(move || {
            fabric::query::read_credential_fd(&path, false, MAX_CREDENTIAL_FILE_BYTES)
        })
        .await
        .map_err(|e| format!("credential file task error: {e}"))?
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
        if let Ok(token) = self.read_validated_credential().await {
            out = redact_token(&out, &token);
        }
        out
    }
}

enum PlanExpectation {
    Page(PlanScope),
    Subscribe,
    Poll,
}
fn validate_plan_response(
    expected: PlanExpectation,
    response: CanonicalResponse,
) -> Result<CanonicalResponse, String> {
    let valid = match (&expected, &response) {
        (_, CanonicalResponse::PlanCursorFailure { .. }) => true,
        (PlanExpectation::Page(scope), CanonicalResponse::PlanPage { page }) => {
            page.scope == *scope
        }
        (PlanExpectation::Subscribe, CanonicalResponse::PlanSubscribed { .. }) => true,
        (PlanExpectation::Poll, CanonicalResponse::PlanHint { .. }) => true,
        _ => false,
    };
    if valid {
        Ok(response)
    } else {
        Err("fabric-query unexpected plan response or scope".into())
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

    #[tokio::test]
    async fn exchange_direct_rejects_rogue_listener_without_leaking_credential() {
        use std::io::Read;
        use std::net::TcpListener;
        use std::os::unix::fs::PermissionsExt;
        use std::time::Instant;

        struct TempDirGuard(PathBuf);
        impl Drop for TempDirGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .filter(|p| !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
            .unwrap_or_else(|| PathBuf::from("/var/tmp"));
        let unique = format!(
            "fabric-client-auth-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let dir = TempDirGuard(base.join(unique));
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o700)).unwrap();
        let cred_path = dir.0.join("reader.token");
        let secret = "client-direct-secret-token-012345";
        std::fs::write(&cred_path, secret).unwrap();
        std::fs::set_permissions(&cred_path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let peer = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let (mut stream, _) = listener.accept().unwrap();
            let hello_bytes = fabric::canonical::read_frame(&mut stream, deadline).unwrap();
            assert!(!String::from_utf8_lossy(&hello_bytes).contains(secret));
            let hello: ClientHello = serde_json::from_slice(&hello_bytes).unwrap();
            let (_, challenge) =
                fabric::canonical::build_server_challenge(&hello.client_nonce, ["wrong-secret"])
                    .unwrap();
            fabric::canonical::write_frame(
                &mut stream,
                &serde_json::to_vec(&challenge).unwrap(),
                deadline,
            )
            .unwrap();
            let mut rest = [0u8; 64];
            let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
            let n = stream.read(&mut rest).unwrap_or(0);
            assert_eq!(n, 0, "rogue listener received post-challenge data");
        });

        let marker = dir.0.join("fallback-ran");
        let stub = dir.0.join("fallback");
        std::fs::write(&stub, format!("#!/bin/sh\n: > '{}'\n", marker.display())).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
        let client =
            FabricQueryClient::with_bin(Some(stub), addr, &cred_path).with_direct_socket(true);
        let err = client
            .subscribe_plan(PlanScope("team".into()))
            .await
            .unwrap_err();
        peer.join().unwrap();
        assert!(err.contains("mutual authentication"), "{err}");
        assert!(
            !marker.exists(),
            "ambiguous direct dispatch must not run CLI"
        );
    }
    #[tokio::test]
    async fn plan_cli_sends_scope_and_token_only_on_stdin() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .expect("disk-backed TMPDIR");
        assert!(!base.starts_with("/tmp") && !base.starts_with("/dev/shm"));
        let path = base.join(format!(
            "fabric-plan-client-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        struct Guard(PathBuf);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _guard = Guard(path.clone());
        let credential = path.join("token");
        std::fs::write(&credential, "fixture-secret").unwrap();
        std::fs::set_permissions(&credential, std::fs::Permissions::from_mode(0o600)).unwrap();
        let capture = path.join("stdin");
        let script = path.join("query");
        std::fs::write(&script,format!("#!/bin/sh\n[ \"$#\" -eq 1 ] && [ \"$1\" = poll-plan-hint ] || exit 9\ncat > '{}'\nprintf '%s' '{{\"result\":\"plan_hint\",\"pending\":true}}'\n",capture.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let client = FabricQueryClient::with_bin(Some(script), "remote.example:123", credential);
        let response = client
            .poll_plan_hint(PlanScope("team-a".into()), "opaque-token".into())
            .await
            .unwrap();
        assert!(matches!(
            response,
            CanonicalResponse::PlanHint { pending: true }
        ));
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(capture).unwrap()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"scope":"team-a","subscription":"opaque-token"})
        );
        std::fs::write(client.bin_path().unwrap(), "#!/bin/sh\ncat > /dev/null\nprintf '%s' '{\"result\":\"plan_subscribed\",\"subscription\":\"x\",\"frontier\":1}'\n").unwrap();
        assert!(
            client
                .poll_plan_hint(PlanScope("team-a".into()), "opaque-token".into())
                .await
                .is_err()
        );
    }
    #[test]
    fn plan_response_is_bound_to_operation_and_page_scope() {
        use fabric::canonical::{PlanCursorFailure, PlanPage};
        let page = |scope: &str| CanonicalResponse::PlanPage {
            page: PlanPage {
                authority: "receiver".into(),
                scope: PlanScope(scope.into()),
                frontier: 1,
                decisions: vec![],
                next_cursor: None,
            },
        };
        assert!(
            validate_plan_response(PlanExpectation::Page(PlanScope("a".into())), page("a")).is_ok()
        );
        assert!(
            validate_plan_response(PlanExpectation::Page(PlanScope("a".into())), page("b"))
                .is_err()
        );
        assert!(validate_plan_response(PlanExpectation::Poll, page("a")).is_err());
        assert!(
            validate_plan_response(
                PlanExpectation::Subscribe,
                CanonicalResponse::PlanHint { pending: true }
            )
            .is_err()
        );
        assert!(matches!(
            validate_plan_response(
                PlanExpectation::Page(PlanScope("a".into())),
                CanonicalResponse::PlanCursorFailure {
                    reason: PlanCursorFailure::Gap
                }
            ),
            Ok(CanonicalResponse::PlanCursorFailure {
                reason: PlanCursorFailure::Gap
            })
        ));
    }
    #[tokio::test]
    async fn oversized_plan_strings_refuse_before_credentials_or_transport() {
        let client = FabricQueryClient::with_bin(
            Some(PathBuf::from("/nonexistent/query")),
            "127.0.0.1:1",
            "/nonexistent/token",
        )
        .with_direct_socket(true);
        let err = client
            .subscribe_plan(PlanScope("x".repeat(MAX_FRAME + 1)))
            .await
            .unwrap_err();
        assert!(err.contains("MAX_FRAME"));
        let err = client
            .poll_plan_hint(PlanScope("a".into()), "x".repeat(MAX_FRAME))
            .await
            .unwrap_err();
        assert!(err.contains("MAX_FRAME"));
        let request = PlanPageRequest {
            scope: PlanScope("a".into()),
            cursor: Some("x".repeat(MAX_FRAME)),
            limit: 1,
            ttl_secs: 1,
        };
        let err = client.plan_page(request).await.unwrap_err();
        assert!(err.contains("MAX_FRAME"));
    }
}
