//! Synchronous HTTPS delivery of stored batches to a Fabric Server.
//!
//! The node sends one batch at a time: the oldest unacknowledged one, as the
//! exact bytes stored in its spool. It advances its ACK cursor only from an
//! `ack` answer, which the server sends after its own durable commit
//! ([ADR-0013](../../docs/decisions/ADR-0013-deliver-batches-in-order-with-bounded-dedup.md)).

use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use ureq::tls::{Certificate, RootCerts, TlsConfig};

const MAX_TOKEN_BYTES: u64 = 256;
const MAX_CA_BYTES: u64 = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Where and how a node delivers. All three values are required together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerTarget {
    /// `https://host:port`; batches go to `<url>/v1/batches`.
    pub url: String,
    /// PEM file with the CA certificate(s) that sign the server certificate.
    pub ca: PathBuf,
    /// File holding this node's bearer token.
    pub token_file: PathBuf,
}

impl ServerTarget {
    pub fn validate(&self) -> io::Result<()> {
        let host = self.url.strip_prefix("https://").unwrap_or("");
        if host.is_empty()
            || host.contains('/')
            || self.url.len() > 240
            || !self.ca.is_absolute()
            || !self.token_file.is_absolute()
            || self.ca.as_os_str().len() > 240
            || self.token_file.as_os_str().len() > 240
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "server_url must be https://host:port; server_ca and token_file absolute paths",
            ));
        }
        Ok(())
    }
}

/// The server's answer, or why there is none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivery {
    Ack(u64),
    Conflict(u64),
    Gap(u64),
    /// 401, 403, 400 or 413: the batch will not be accepted as sent.
    Rejected(String),
    /// 503, a transport error or a malformed answer: retry later.
    Retry(String),
}

pub struct Sender {
    agent: ureq::Agent,
    endpoint: String,
    config_endpoint: String,
    authorization: String,
}

/// The configuration a server wants this node to run.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteView {
    pub revision: u64,
    pub paused: bool,
    pub logs: Vec<String>,
    pub metric_interval_s: u64,
}

fn read_bounded(source: impl Read, name: &Path, cap: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    source.take(cap + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > cap {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} exceeds its size cap", name.display()),
        ));
    }
    Ok(bytes)
}

/// Open a credential file. It must be regular, not a symlink and readable only
/// by its owner, or also by its group when root owns it (`0640 root:fabricolly`).
fn open_secret(path: &Path) -> io::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    let root_group = meta.uid() == 0 && meta.mode() & 0o037 == 0;
    if !meta.is_file() || (meta.mode() & 0o077 != 0 && !root_group) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{} must be a regular file readable only by its owner (0600) or, if root-owned, its group (0640)",
                path.display()
            ),
        ));
    }
    Ok(file)
}

/// An HTTPS client that trusts only the CA certificates in `ca`, with the
/// ring provider and a bounded request time. Statuses are returned, not errors.
pub fn agent(ca: &Path) -> io::Result<ureq::Agent> {
    let pem = read_bounded(std::fs::File::open(ca)?, ca, MAX_CA_BYTES)?;
    let ca = Certificate::from_pem(&pem)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
    let tls = TlsConfig::builder()
        .root_certs(RootCerts::Specific(Arc::new(vec![ca])))
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    Ok(ureq::Agent::config_builder()
        .tls_config(tls)
        .http_status_as_error(false)
        .timeout_global(Some(REQUEST_TIMEOUT))
        .build()
        .new_agent())
}

/// Read a bearer token file: trimmed printable ASCII, as `open_secret` allows.
pub fn read_token(path: &Path) -> io::Result<String> {
    let token = String::from_utf8(read_bounded(open_secret(path)?, path, MAX_TOKEN_BYTES)?)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "token is not UTF-8"))?;
    let token = token.trim().to_owned();
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "token must be printable ASCII",
        ));
    }
    Ok(token)
}

impl Sender {
    pub fn new(target: &ServerTarget) -> io::Result<Self> {
        target.validate()?;
        let token = read_token(&target.token_file)?;
        let agent = agent(&target.ca)?;
        Ok(Self {
            agent,
            endpoint: format!("{}/v1/batches", target.url),
            config_endpoint: format!("{}/v1/config", target.url),
            authorization: format!("Bearer {token}"),
        })
    }

    /// Poll for configuration, reporting what this node runs. `Ok(None)`
    /// means the server's view equals `applied_revision`.
    pub fn fetch_config(
        &self,
        applied_revision: u64,
        config_error: Option<&str>,
    ) -> Result<Option<RemoteView>, String> {
        let mut request = self
            .agent
            .get(&self.config_endpoint)
            .header("authorization", &self.authorization)
            .header("x-fabric-applied-revision", applied_revision.to_string())
            .header("if-none-match", format!("\"{applied_revision}\""));
        if let Some(error) = config_error {
            let printable: String = error
                .chars()
                .filter(|c| c.is_ascii_graphic() || *c == ' ')
                .take(240)
                .collect();
            request = request.header("x-fabric-config-error", printable);
        }
        let mut response = request.call().map_err(|e| format!("transport: {e}"))?;
        let status = response.status().as_u16();
        if status == 304 {
            return Ok(None);
        }
        let body = response
            .body_mut()
            .with_config()
            .limit(64 * 1024)
            .read_to_string()
            .map_err(|e| format!("read: {e}"))?;
        if status != 200 {
            return Err(format!("HTTP {status}: {body}"));
        }
        serde_json::from_str(&body)
            .map(Some)
            .map_err(|e| format!("invalid configuration answer: {e}"))
    }

    /// Send one batch's exact bytes and classify the answer.
    pub fn send(&self, bytes: &[u8]) -> Delivery {
        let response = self
            .agent
            .post(&self.endpoint)
            .header("authorization", &self.authorization)
            .header("content-type", "application/x-protobuf")
            .send(bytes);
        let mut response = match response {
            Ok(response) => response,
            Err(error) => return Delivery::Retry(format!("transport: {error}")),
        };
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(4096)
            .read_to_string()
            .unwrap_or_default();
        let parsed: Option<(String, Option<u64>)> =
            serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| {
                    Some((
                        v.get("status")?.as_str()?.to_owned(),
                        v.get("committed_through").and_then(|c| c.as_u64()),
                    ))
                });
        match (status, parsed) {
            (200, Some((kind, Some(through)))) if kind == "ack" => Delivery::Ack(through),
            (409, Some((kind, Some(through)))) if kind == "conflict" => Delivery::Conflict(through),
            (409, Some((kind, Some(through)))) if kind == "gap" => Delivery::Gap(through),
            (401 | 403 | 400 | 413, _) => Delivery::Rejected(format!("HTTP {status}: {body}")),
            _ => Delivery::Retry(format!("HTTP {status}: {body}")),
        }
    }
}
