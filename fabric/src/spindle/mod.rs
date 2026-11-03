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
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub socket: PathBuf,
    pub storage: PathBuf,
    pub quota_bytes: u64,
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
        records: Vec<(u64, crate::observation::Observation)>,
    },
    Stopped,
    Error {
        error: String,
    },
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Health {
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
