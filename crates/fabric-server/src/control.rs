//! Node enrollment, inventory and desired configuration (ADR-0014).
//!
//! Durable part: one JSON file replaced by synced rename on every
//! administrative change. Observed part (last poll, applied revision,
//! validation error): memory only, refreshed by each node poll. What a
//! request means (revocation is terminal, revisions advance, name and shape
//! limits) is decided by `fabric_core::control`; this module persists it.

use fabric_core::control::{self as kernel, ControlRejection};
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
/// One local credential per server state directory, never the administrator token.
pub const SELF_SPINDLE_NAME: &str = "fabric-server-self";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Active,
    Paused,
    Revoked,
}

impl From<Status> for kernel::Status {
    fn from(status: Status) -> Self {
        match status {
            Status::Active => kernel::Status::Active,
            Status::Paused => kernel::Status::Paused,
            Status::Revoked => kernel::Status::Revoked,
        }
    }
}

impl From<kernel::Status> for Status {
    fn from(status: kernel::Status) -> Self {
        match status {
            kernel::Status::Active => Status::Active,
            kernel::Status::Paused => Status::Paused,
            kernel::Status::Revoked => Status::Revoked,
        }
    }
}

fn rejected(rejection: ControlRejection) -> io::Error {
    invalid(match rejection {
        ControlRejection::Revoked => "node is revoked",
        ControlRejection::RevisionExhausted => "node revision is exhausted",
        ControlRejection::InvalidName => "node name must be 1-64 of [A-Za-z0-9._-]",
        ControlRejection::InvalidDesired => {
            "configuration needs 1-3600 s interval and at most 16 absolute log paths"
        }
    })
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

impl NodeRecord {
    /// Stable enrollment identity, independent of its display label. Existing
    /// credentials are immutable and revoked records/names are never reused.
    /// A future credential-rotation migration must persist this identity first.
    pub fn enrollment_id(&self) -> String {
        sha256_hex(format!("fabric-enrollment-v1:{}", self.token_sha256).as_bytes())
    }
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
    /// A published rename with failed directory sync requires reopen before
    /// memory may authorize or publish another administrative transition.
    poisoned: bool,
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
    kernel::valid_name(name)
}

/// Server-side shape checks. The node validates again with its own local
/// profile before it activates anything.
pub fn check_desired(desired: &DesiredConfig) -> io::Result<()> {
    kernel::check_desired(desired.metric_interval_s, &desired.logs).map_err(rejected)
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
            poisoned: false,
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
            if kernel::authorizes(record.status.into()) {
                control.by_token.insert(hash, record.name.clone());
            }
            control.nodes.insert(record.name.clone(), record);
        }
        Ok(control)
    }

    fn ensure_healthy(&self) -> io::Result<()> {
        if self.poisoned {
            return Err(io::Error::other(
                "control publication uncertain; reopen required",
            ));
        }
        Ok(())
    }

    fn persist(&mut self) -> io::Result<()> {
        self.persist_with(|dir| File::open(dir)?.sync_all())
    }

    fn persist_with(
        &mut self,
        sync_directory: impl FnOnce(&Path) -> io::Result<()>,
    ) -> io::Result<()> {
        self.ensure_healthy()?;
        let stored = Stored {
            version: 1,
            nodes: self.nodes.values().cloned().collect(),
        };
        let text =
            serde_json::to_vec_pretty(&stored).map_err(|e| io::Error::other(e.to_string()))?;
        // Every accepted publication must remain readable by open, including
        // JSON escaping and formatting overhead across the whole inventory.
        if text.len() as u64 > MAX_STATE_BYTES {
            return Err(invalid("control state exceeds its size cap"));
        }
        let staged = self.dir.join("control.json.tmp");
        let mut out = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&staged)?;
        out.write_all(&text)?;
        out.sync_all()?;
        fs::rename(&staged, self.dir.join(STATE))?;
        if let Err(error) = sync_directory(&self.dir) {
            self.poisoned = true;
            return Err(error);
        }
        Ok(())
    }

    /// Apply and persist one transition. Failure restores the previous record;
    /// uncertainty after rename additionally disables authority until reopen.
    fn change(
        &mut self,
        name: &str,
        edit: impl FnOnce(&mut NodeRecord) -> io::Result<()>,
    ) -> io::Result<NodeRecord> {
        self.ensure_healthy()?;
        let before = self
            .nodes
            .get(name)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such node"))?;
        let mut after = before.clone();
        edit(&mut after)?;
        after.revision = kernel::next_revision(before.revision).map_err(rejected)?;
        self.nodes.insert(name.to_owned(), after.clone());
        if let Err(error) = self.persist() {
            self.nodes.insert(name.to_owned(), before);
            return Err(error);
        }
        self.by_token.retain(|_, n| n != name);
        if kernel::authorizes(after.status.into()) {
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
        self.ensure_healthy()?;
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
            kernel::may_set_config(r.status.into()).map_err(rejected)?;
            r.desired = desired;
            Ok(())
        })
    }

    /// Bootstrap a local credential already durably stored by the CLI. A retry
    /// after token publication reuses it; existing/revoked identities are never
    /// replaced or silently reactivated. Uses the ordinary control publication.
    pub fn ensure_self_spindle(&mut self, token: &str, desired: DesiredConfig) -> io::Result<()> {
        self.ensure_healthy()?;
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid("local Spindle token must be 64 hexadecimal bytes"));
        }
        let hash = sha256_hex(token.as_bytes());
        if let Some(existing) = self.nodes.get(SELF_SPINDLE_NAME) {
            if existing.token_sha256 != hash || existing.status == Status::Revoked {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "local Spindle identity differs or is revoked; restore its credential or resolve enrollment explicitly",
                ));
            }
            return Ok(());
        }
        check_desired(&desired)?;
        if self.nodes.len() >= MAX_NODES
            || self
                .nodes
                .values()
                .any(|record| record.token_sha256 == hash)
        {
            return Err(invalid(
                "local Spindle enrollment is full or credential is already assigned",
            ));
        }
        let record = NodeRecord {
            name: SELF_SPINDLE_NAME.into(),
            token_sha256: hash.clone(),
            status: Status::Active,
            revision: 1,
            desired,
            enrolled_unix_s: unix_s(),
        };
        self.nodes.insert(SELF_SPINDLE_NAME.into(), record);
        if let Err(error) = self.persist() {
            self.nodes.remove(SELF_SPINDLE_NAME);
            return Err(error);
        }
        self.by_token
            .insert(parse_hex32(&hash).unwrap(), SELF_SPINDLE_NAME.into());
        Ok(())
    }

    pub fn set_status(&mut self, name: &str, status: Status) -> io::Result<NodeRecord> {
        self.change(name, |r| {
            r.status = kernel::set_status(r.status.into(), status.into())
                .map_err(rejected)?
                .into();
            Ok(())
        })
    }

    /// The node name for a presented bearer token, if it is enrolled and not revoked.
    pub fn authenticate(&self, token: &str) -> Option<String> {
        if self.poisoned {
            return None;
        }
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
        if self.poisoned {
            return None;
        }
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
            paused: kernel::is_paused(record.status.into()),
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

#[cfg(test)]
mod publication_tests {
    use super::*;

    #[test]
    fn a_failed_postrename_sync_stops_authorization_until_reopen() {
        let root = std::env::var_os("FABRIC_SCRATCH_ROOT")
            .expect("run tests through the resource launcher");
        let path = PathBuf::from(root).join(format!("control-sync-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let result = std::panic::catch_unwind(|| {
            let mut control = Control::open(&path).unwrap();
            let (_, token) = control
                .enroll(
                    "edge",
                    DesiredConfig {
                        logs: vec![],
                        metric_interval_s: 15,
                    },
                )
                .unwrap();
            assert_eq!(control.authenticate(&token).as_deref(), Some("edge"));
            let before = control.nodes["edge"].clone();
            control.nodes.get_mut("edge").unwrap().status = Status::Revoked;
            let error = control
                .persist_with(|_| Err(io::Error::other("injected directory sync failure")))
                .unwrap_err();
            assert_eq!(error.to_string(), "injected directory sync failure");
            // Administrative callers restore their previous in-memory record
            // on error. The failed sync cannot turn that rollback into authority.
            control.nodes.insert("edge".into(), before);
            assert!(control.authenticate(&token).is_none());
            assert!(control.poll("edge", 1, None).is_none());
            assert!(control.set_status("edge", Status::Active).is_err());
            assert!(
                control
                    .enroll(
                        "new",
                        DesiredConfig {
                            logs: vec![],
                            metric_interval_s: 15
                        }
                    )
                    .is_err()
            );
            assert!(
                control
                    .ensure_self_spindle(
                        &"aa".repeat(32),
                        DesiredConfig {
                            logs: vec![],
                            metric_interval_s: 15
                        }
                    )
                    .is_err()
            );
            drop(control);
            // A real rename occurred before the injected failure. Reopen
            // reconciles the published state and preserves terminal revocation.
            let mut reopened = Control::open(&path).unwrap();
            assert!(reopened.authenticate(&token).is_none());
            assert_eq!(reopened.inventory()[0].0.status, Status::Revoked);
            assert!(reopened.set_status("edge", Status::Active).is_err());
        });
        fs::remove_dir_all(&path).expect("remove owned control sync scratch");
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}
