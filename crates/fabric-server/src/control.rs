//! Node enrollment, inventory and desired configuration (ADR-0014).
//!
//! Durable part: one JSON file replaced by synced rename on every
//! administrative change. Observed part (last poll, applied revision,
//! validation error): memory only, refreshed by each node poll.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const STATE: &str = "control.json";
const MAX_STATE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_NODES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Active,
    Paused,
    Revoked,
}

/// What an administrator wants a node to collect.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesiredConfig {
    pub logs: Vec<String>,
    pub metric_interval_s: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeRecord {
    pub name: String,
    pub token_sha256: String,
    pub status: Status,
    pub revision: u64,
    pub desired: DesiredConfig,
    pub enrolled_unix_s: u64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    nodes: Vec<NodeRecord>,
}

/// What the latest poll from a node reported.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Observed {
    pub last_poll_unix_s: u64,
    pub applied_revision: u64,
    pub config_error: Option<String>,
}

/// The answer to a node's configuration poll.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NodeView {
    pub revision: u64,
    pub paused: bool,
    pub logs: Vec<String>,
    pub metric_interval_s: u64,
}

pub struct Control {
    dir: PathBuf,
    nodes: BTreeMap<String, NodeRecord>,
    by_token: HashMap<[u8; 32], String>,
    observed: HashMap<String, Observed>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn unix_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn parse_hex32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0_u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(out)
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// Server-side shape checks. The node validates again with its own local
/// profile before it activates anything.
pub fn check_desired(desired: &DesiredConfig) -> io::Result<()> {
    if desired.metric_interval_s == 0
        || desired.metric_interval_s > 3600
        || desired.logs.len() > 16
        || desired
            .logs
            .iter()
            .any(|p| !p.starts_with('/') || p.len() > 240 || p.contains('\n') || p.contains('\0'))
    {
        return Err(invalid(
            "configuration needs 1-3600 s interval and at most 16 absolute log paths",
        ));
    }
    Ok(())
}

impl Control {
    pub fn open(state_dir: &Path) -> io::Result<Self> {
        let dir = state_dir.to_path_buf();
        fs::create_dir_all(&dir)?;
        let stored: Stored = match File::open(dir.join(STATE)) {
            Ok(file) => {
                let mut text = String::new();
                file.take(MAX_STATE_BYTES + 1).read_to_string(&mut text)?;
                if text.len() as u64 > MAX_STATE_BYTES {
                    return Err(invalid("control state exceeds its size cap"));
                }
                serde_json::from_str(&text)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Stored {
                version: 1,
                nodes: Vec::new(),
            },
            Err(error) => return Err(error),
        };
        if stored.version != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported control state version",
            ));
        }
        let mut control = Self {
            dir,
            nodes: BTreeMap::new(),
            by_token: HashMap::new(),
            observed: HashMap::new(),
        };
        for record in stored.nodes {
            let hash = parse_hex32(&record.token_sha256).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid stored token hash")
            })?;
            if !valid_name(&record.name) || control.nodes.contains_key(&record.name) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid or duplicate stored node name",
                ));
            }
            if record.status != Status::Revoked {
                control.by_token.insert(hash, record.name.clone());
            }
            control.nodes.insert(record.name.clone(), record);
        }
        Ok(control)
    }

    fn persist(&self) -> io::Result<()> {
        let stored = Stored {
            version: 1,
            nodes: self.nodes.values().cloned().collect(),
        };
        let text =
            serde_json::to_vec_pretty(&stored).map_err(|e| io::Error::other(e.to_string()))?;
        let staged = self.dir.join("control.json.tmp");
        let mut out = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&staged)?;
        out.write_all(&text)?;
        out.sync_all()?;
        fs::rename(&staged, self.dir.join(STATE))?;
        File::open(&self.dir)?.sync_all()
    }

    /// Apply a change to one record and persist it; roll back on failure so
    /// memory never claims a state that is not durable.
    fn change(
        &mut self,
        name: &str,
        edit: impl FnOnce(&mut NodeRecord) -> io::Result<()>,
    ) -> io::Result<NodeRecord> {
        let before = self
            .nodes
            .get(name)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such node"))?;
        let mut after = before.clone();
        edit(&mut after)?;
        after.revision = before.revision + 1;
        self.nodes.insert(name.to_owned(), after.clone());
        if let Err(error) = self.persist() {
            self.nodes.insert(name.to_owned(), before);
            return Err(error);
        }
        self.by_token.retain(|_, n| n != name);
        if after.status != Status::Revoked {
            self.by_token
                .insert(parse_hex32(&after.token_sha256).unwrap(), name.to_owned());
        }
        Ok(after)
    }

    /// Enroll a new node; the returned token is shown once and stored only as a hash.
    pub fn enroll(
        &mut self,
        name: &str,
        desired: DesiredConfig,
    ) -> io::Result<(NodeRecord, String)> {
        if !valid_name(name) {
            return Err(invalid("node name must be 1-64 of [A-Za-z0-9._-]"));
        }
        check_desired(&desired)?;
        if self.nodes.contains_key(name) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "node already enrolled",
            ));
        }
        if self.nodes.len() >= MAX_NODES {
            return Err(invalid("node inventory is full"));
        }
        let mut secret = [0_u8; 32];
        File::open("/dev/urandom")?.read_exact(&mut secret)?;
        let token: String = secret.iter().map(|b| format!("{b:02x}")).collect();
        let hash_hex = sha256_hex(token.as_bytes());
        let record = NodeRecord {
            name: name.to_owned(),
            token_sha256: hash_hex.clone(),
            status: Status::Active,
            revision: 1,
            desired,
            enrolled_unix_s: unix_s(),
        };
        self.nodes.insert(name.to_owned(), record.clone());
        if let Err(error) = self.persist() {
            self.nodes.remove(name);
            return Err(error);
        }
        self.by_token
            .insert(parse_hex32(&hash_hex).unwrap(), name.to_owned());
        Ok((record, token))
    }

    pub fn set_config(&mut self, name: &str, desired: DesiredConfig) -> io::Result<NodeRecord> {
        check_desired(&desired)?;
        self.change(name, |r| {
            if r.status == Status::Revoked {
                return Err(invalid("node is revoked"));
            }
            r.desired = desired;
            Ok(())
        })
    }

    pub fn set_status(&mut self, name: &str, status: Status) -> io::Result<NodeRecord> {
        self.change(name, |r| {
            if r.status == Status::Revoked {
                return Err(invalid("node is revoked"));
            }
            r.status = status;
            Ok(())
        })
    }

    /// The node name for a presented bearer token, if it is enrolled and not revoked.
    pub fn authenticate(&self, token: &str) -> Option<String> {
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        self.by_token.get(&digest).cloned()
    }

    /// Record a poll and return the node's current view.
    pub fn poll(
        &mut self,
        name: &str,
        applied_revision: u64,
        error: Option<String>,
    ) -> Option<NodeView> {
        let record = self.nodes.get(name)?;
        self.observed.insert(
            name.to_owned(),
            Observed {
                last_poll_unix_s: unix_s(),
                applied_revision,
                config_error: error,
            },
        );
        Some(NodeView {
            revision: record.revision,
            paused: record.status == Status::Paused,
            logs: record.desired.logs.clone(),
            metric_interval_s: record.desired.metric_interval_s,
        })
    }

    pub fn inventory(&self) -> Vec<(NodeRecord, Observed)> {
        self.nodes
            .values()
            .map(|r| {
                (
                    r.clone(),
                    self.observed.get(&r.name).cloned().unwrap_or_default(),
                )
            })
            .collect()
    }
}
