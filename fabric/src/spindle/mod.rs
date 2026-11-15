//! Fedora user-service collection and local durable custody.
pub const MAX_REPLY: usize = crate::observation::MAX_FRAME + 8192;
pub mod custody;
mod server;
pub mod sources;
pub mod transport;
pub mod wire;
use serde::{Deserialize, Serialize};
pub use server::Daemon;
use std::path::PathBuf;

/// Notify `systemd` (`Type=notify`) once the service socket is bound and ready.
pub fn notify_systemd_ready() {
    #[cfg(target_os = "linux")]
    {
        use std::os::linux::net::SocketAddrExt;
        use std::os::unix::net::{SocketAddr, UnixDatagram};
        let Some(raw) = std::env::var_os("NOTIFY_SOCKET").filter(|v| !v.is_empty()) else {
            return;
        };
        let Ok(sock) = UnixDatagram::unbound() else {
            return;
        };
        let bytes = raw.as_encoded_bytes();
        let addr = if let Some(abs) = bytes.strip_prefix(b"@") {
            SocketAddr::from_abstract_name(abs).ok()
        } else if bytes.starts_with(b"/") {
            SocketAddr::from_pathname(std::path::Path::new(&raw)).ok()
        } else {
            None
        };
        if let Some(addr) = addr {
            let _ = sock.send_to_addr(b"READY=1\n", &addr);
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub socket: PathBuf,
    pub storage: PathBuf,
    pub quota_bytes: u64,
    #[serde(default)]
    pub forwarding: Option<ForwardConfig>,
    #[serde(default = "connections")]
    pub max_connections: usize,
    #[serde(default = "timeout")]
    pub client_timeout_ms: u64,
    #[serde(default)]
    pub files: Vec<sources::FileConfig>,
    #[serde(default)]
    pub journal: Option<sources::JournalConfig>,
    #[serde(default)]
    pub host: Option<sources::HostConfig>,
}
/// Explicit canonical authority; credentials remain in a private file.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardConfig {
    pub endpoint: String,
    pub credential_file: PathBuf,
    pub authority: String,
    /// Enable only after backfill and shadow comparison have been verified.
    #[serde(default)]
    pub reclaim_confirmed: bool,
}
impl ForwardConfig {
    pub fn validate(&self) -> Result<(), String> {
        let endpoint: std::net::SocketAddr = self
            .endpoint
            .parse()
            .map_err(|_| "canonical endpoint must be an IP socket address")?;
        if !endpoint.ip().is_loopback()
            || self.authority.is_empty()
            || self.authority.len() > 128
            || !self.credential_file.is_absolute()
        {
            return Err("invalid forwarding configuration".into());
        }
        self.credential().map(|_| ())
    }
    pub fn credential(&self) -> Result<String, String> {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::symlink_metadata(&self.credential_file).map_err(|e| e.to_string())?;
        if !m.is_file()
            || m.uid() != rustix::process::getuid().as_raw()
            || m.mode() & 0o077 != 0
            || m.len() > 4096
        {
            return Err("credential must be a private owned regular file".into());
        }
        let credential = std::fs::read_to_string(&self.credential_file)
            .map_err(|e| e.to_string())?
            .trim()
            .to_owned();
        if credential.is_empty() {
            return Err("empty canonical credential".into());
        }
        Ok(credential)
    }
}
fn connections() -> usize {
    16
}
fn timeout() -> u64 {
    500
}
impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if !self.socket.is_absolute() || !self.storage.is_absolute() {
            return Err("socket and storage must be absolute paths".into());
        }
        if let Some(forwarding) = &self.forwarding {
            forwarding.validate()?;
        }
        let mut ids = std::collections::BTreeSet::new();
        for f in &self.files {
            if !ids.insert(&f.id) {
                return Err("duplicate file source ID".into());
            }
        }
        if self.max_connections == 0
            || self.max_connections > 128
            || self.client_timeout_ms == 0
            || self.client_timeout_ms > 5000
            || self.files.len() > 32
        {
            return Err("invalid collector limits".into());
        }
        for f in &self.files {
            f.validate()?
        }
        if let Some(j) = &self.journal {
            j.validate()?
        }
        if let Some(h) = &self.host {
            h.validate()?
        }
        Ok(())
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Status,
    Read { after: u64, limit: usize },
    Stop,
    Lease { producer: String },
    FabricProgress,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reply {
    Receipt {
        receipt: custody::Receipt,
    },
    Status {
        status: Health,
    },
    Records {
        records: Vec<(u64, crate::observation::StoredObservation)>,
    },
    Stopped,
    Lease {
        generation: String,
        expires_unix_s: u64,
    },
    FabricProgress {
        authority: Option<String>,
        origin: String,
        through: u64,
    },
    Error {
        error: String,
    },
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Health {
    pub origin: String,
    pub canonical_through: u64,
    pub reclaimed_through: u64,
    pub forwarding_error: Option<String>,
    pub ingestion: u64,
    pub refusals: u64,
    pub errors: u64,
    pub retries: u64,
    pub records: u64,
    pub highest_record_id: u64,
    pub custody_bytes: u64,
    pub active_connections: usize,
    pub queue_bytes: usize,
    pub last_commit_latency_ns: u64,
    pub uptime_ms: u64,
    pub source_status: Vec<sources::SourceStatus>,
    pub missed_samples: u64,
    pub process_resident_bytes: u64,
}
