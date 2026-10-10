//! Local passkey authentication and explicit scoped authority (ADR-0027).
//!
//! Callers hold their access mutex through authorization and control publication.
//! Sessions and one-use ceremony state never leave this server except opaque IDs.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use subtle::ConstantTimeEq;
use webauthn_rs::fake::{FakePasskeyDistribution, WebauthnFakeCredentialGenerator};
use webauthn_rs::prelude::*;

const STATE_CAP: usize = 16 * 1024 * 1024;
const PRINCIPAL_CAP: usize = 1024;
const SESSION_CAP: usize = 4096;
const CEREMONY_CAP: usize = 1024;
const AUDIT_CAP: usize = 4096;
const CHALLENGE_TTL: u64 = 300;
const FRESH_TTL: u64 = 300;
const IDLE_TTL: u64 = 1800;
const SESSION_TTL: u64 = 28800;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AccessConfig {
    pub origin: String,
    pub rp_id: String,
    pub audience: String,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    TelemetryRead,
    InventoryRead,
    NodeConfigure,
    NodePause,
    NodeResume,
    NodeEnroll,
    NodeRevoke,
    IdentityManage,
    GrantManage,
    AuditRead,
}
impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::TelemetryRead => "telemetry.read",
            Self::InventoryRead => "inventory.read",
            Self::NodeConfigure => "node.configure",
            Self::NodePause => "node.pause",
            Self::NodeResume => "node.resume",
            Self::NodeEnroll => "node.enroll",
            Self::NodeRevoke => "node.revoke",
            Self::IdentityManage => "identity.manage",
            Self::GrantManage => "grant.manage",
            Self::AuditRead => "audit.read",
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Signal {
    Metrics,
    Logs,
    Traces,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub actions: BTreeSet<Action>,
    /// Explicit installation-wide grants are reserved for the owner.
    pub installation_wide: bool,
    pub enrollments: BTreeSet<String>,
    pub signals: BTreeSet<Signal>,
    pub max_query_window_s: u64,
    pub max_query_rows: usize,
    pub allowed_log_paths: BTreeSet<String>,
    pub min_interval_s: u64,
    pub max_interval_s: u64,
    pub enrollment_namespace: Option<String>,
    pub max_enrollments: usize,
}
impl Scope {
    pub fn owner() -> Self {
        Self {
            actions: [
                Action::TelemetryRead,
                Action::InventoryRead,
                Action::NodeConfigure,
                Action::NodePause,
                Action::NodeResume,
                Action::NodeEnroll,
                Action::NodeRevoke,
                Action::IdentityManage,
                Action::GrantManage,
                Action::AuditRead,
            ]
            .into(),
            installation_wide: true,
            enrollments: BTreeSet::new(),
            signals: [Signal::Metrics, Signal::Logs, Signal::Traces].into(),
            max_query_window_s: 86400,
            max_query_rows: 10000,
            allowed_log_paths: BTreeSet::new(),
            min_interval_s: 1,
            max_interval_s: 3600,
            enrollment_namespace: None,
            max_enrollments: 4096,
        }
    }
    pub fn allows(&self, action: Action, enrollment: Option<&str>, signal: Option<Signal>) -> bool {
        self.actions.contains(&action)
            && enrollment.is_none_or(|id| self.installation_wide || self.enrollments.contains(id))
            && signal.is_none_or(|s| self.signals.contains(&s))
    }
    pub fn contains(&self, other: &Self) -> bool {
        other.actions.is_subset(&self.actions)
            && (!other.installation_wide || self.installation_wide)
            && (self.installation_wide || other.enrollments.is_subset(&self.enrollments))
            && other.signals.is_subset(&self.signals)
            && other.max_query_window_s <= self.max_query_window_s
            && other.max_query_rows <= self.max_query_rows
            && (self.installation_wide
                || other.allowed_log_paths.is_subset(&self.allowed_log_paths))
            && other.min_interval_s >= self.min_interval_s
            && other.max_interval_s <= self.max_interval_s
            && other.max_enrollments <= self.max_enrollments
            && (self.installation_wide || other.enrollment_namespace == self.enrollment_namespace)
    }
    fn validate(&self) -> io::Result<()> {
        if self.enrollments.len() > 4096
            || self.allowed_log_paths.len() > 16
            || self
                .enrollments
                .iter()
                .any(|s| s.is_empty() || s.len() > 128)
            || self
                .allowed_log_paths
                .iter()
                .any(|s| !s.starts_with('/') || s.len() > 4096)
            || self.max_query_rows == 0
            || self.max_query_rows > 10000
            || self.max_query_window_s == 0
            || self.max_query_window_s > 86400
            || self.min_interval_s == 0
            || self.max_interval_s > 3600
            || self.min_interval_s > self.max_interval_s
            || self.max_enrollments > 4096
            || self
                .enrollment_namespace
                .as_ref()
                .is_some_and(|s| s.len() > 64)
        {
            return Err(denied());
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PrincipalKind {
    Human,
    Workload,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Principal {
    id: String,
    name: String,
    kind: PrincipalKind,
    disabled: bool,
    epoch: u64,
    scope: Scope,
    passkeys: Vec<Passkey>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkloadCredential {
    id: String,
    principal: String,
    hash: String,
    audience: String,
    issued: u64,
    expires: u64,
    epoch: u64,
    scope: Scope,
    on_behalf_of: Option<String>,
    parent_epoch: Option<u64>,
    parent_session: Option<String>,
    parent_credential: Option<String>,
    policy: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditEvent {
    pub time: u64,
    pub request_id: String,
    pub actor: Option<String>,
    pub on_behalf_of: Option<String>,
    pub action: String,
    pub resource: Option<String>,
    pub policy_version: u64,
    pub outcome: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Invitation {
    principal: String,
    hash: String,
    expires: u64,
    epoch: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    config: AccessConfig,
    installation_epoch: String,
    fake_credential_key: String,
    policy: u64,
    last_time: u64,
    bootstrap_hash: Option<String>,
    bootstrap_expires: u64,
    recovery_principal: Option<String>,
    principals: BTreeMap<String, Principal>,
    invitations: BTreeMap<String, Invitation>,
    credentials: BTreeMap<String, WorkloadCredential>,
    audit: Vec<AuditEvent>,
    audit_dropped: u64,
    audit_next_id: u64,
    pending_control: Option<ControlIntent>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlIntent {
    id: String,
    action: Action,
    resource: String,
    actor: String,
    on_behalf_of: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryIntent {
    version: u32,
    old_epoch: String,
    old_state_hash: String,
    next: Stored,
    secret: String,
}

struct Session {
    principal: String,
    epoch: u64,
    policy: u64,
    csrf_hash: String,
    csrf_token: String,
    issued: u64,
    seen: u64,
    verified: u64,
}
enum Ceremony {
    Registration {
        principal: String,
        name: String,
        bootstrap: bool,
        epoch: u64,
        expires: u64,
        state: PasskeyRegistration,
    },
    Login {
        principal: String,
        epoch: u64,
        expires: u64,
        state: PasskeyAuthentication,
        credential_state: String,
    },
}
#[derive(Clone, Debug, Serialize)]
pub struct CeremonyStart {
    pub ceremony_id: String,
    pub principal_id: String,
    pub public_key: serde_json::Value,
}
#[derive(Serialize)]
pub struct SessionIssued {
    #[serde(skip_serializing)]
    pub session_token: String,
    pub csrf_token: String,
    pub principal_id: String,
    pub expires_unix_s: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Authority {
    pub principal_id: String,
    pub kind: PrincipalKind,
    pub scope: Scope,
    pub policy_version: u64,
    pub on_behalf_of: Option<String>,
    #[serde(skip)]
    session_hash: Option<String>,
    #[serde(skip)]
    credential_id: Option<String>,
    #[serde(skip)]
    epoch: u64,
}
#[derive(Serialize)]
pub struct CredentialIssued {
    pub principal_id: String,
    pub credential_id: String,
    pub token: String,
    pub expires_unix_s: u64,
}
#[derive(Serialize)]
pub struct PrincipalView {
    pub id: String,
    pub name: String,
    pub kind: PrincipalKind,
    pub disabled: bool,
    pub scope: Scope,
    pub passkey_count: usize,
}
pub struct Access {
    dir: PathBuf,
    state: Stored,
    webauthn: Webauthn,
    sessions: BTreeMap<String, Session>,
    ceremonies: BTreeMap<String, Ceremony>,
    poisoned: bool,
    last_time: u64,
    denial_time: u64,
    denied_dropped: u64,
}
fn denied() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "access denied")
}

fn require_resident_registration(challenge: &mut CreationChallengeResponse) -> io::Result<()> {
    let selection = challenge
        .public_key
        .authenticator_selection
        .as_mut()
        .ok_or_else(denied)?;
    // The library's prelude does not expose the resident-key enum. Infer its
    // public field type without adding another dependency or changing opaque
    // registration state, challenge bytes or the required UV policy.
    selection.resident_key = Some(
        serde_json::from_value(serde_json::Value::String("required".into()))
            .map_err(|_| denied())?,
    );
    selection.require_resident_key = true;
    Ok(())
}
fn passkey_revision(keys: &[Passkey]) -> io::Result<String> {
    let bytes = serde_json::to_vec(keys).map_err(|_| invalid("passkey serialization"))?;
    Ok(hex_bytes(&Sha256::digest(bytes)))
}
fn require_passkey_revision(keys: &[Passkey], expected: &str) -> io::Result<()> {
    if equal(&passkey_revision(keys)?, expected) {
        Ok(())
    } else {
        Err(denied())
    }
}

fn invalid(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn hash(s: &str) -> String {
    hex_bytes(&Sha256::digest(s.as_bytes()))
}
fn equal(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}
fn random() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn identifier() -> io::Result<String> {
    let token = random()?;
    let bytes = (0..16)
        .map(|i| u8::from_str_radix(&token[i * 2..i * 2 + 2], 16).unwrap_or(0))
        .collect::<Vec<_>>();
    Uuid::from_slice(&bytes)
        .map(|u| u.to_string())
        .map_err(|_| invalid("random identifier"))
}
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}
impl Access {
    fn audience(&self) -> String {
        format!(
            "{}#{}",
            self.state.config.audience, self.state.installation_epoch
        )
    }
    pub fn open(dir: &Path, config: AccessConfig, now: u64) -> io::Result<Self> {
        Self::open_mode(dir, config, now, false)
    }
    fn open_mode(
        dir: &Path,
        config: AccessConfig,
        now: u64,
        offline_reconcile: bool,
    ) -> io::Result<Self> {
        let origin = Url::parse(&config.origin).map_err(|_| invalid("invalid access origin"))?;
        if origin.scheme() != "https"
            || origin.origin().ascii_serialization() != config.origin
            || config.audience.is_empty()
            || config.audience.len() > 256
        {
            return Err(invalid(
                "access requires exact HTTPS origin and bounded audience",
            ));
        }
        let webauthn = WebauthnBuilder::new(&config.rp_id, &origin)
            .map_err(|_| invalid("invalid RP ID"))?
            .allow_subdomains(false)
            .allow_any_port(false)
            .build()
            .map_err(|_| invalid("WebAuthn configuration"))?;
        fs::create_dir_all(dir)?;
        if dir.join("access-recovery.intent").exists() {
            Self::replay_recovery(dir, &config, now)?;
        }
        let path = dir.join("access.json");
        let state = match File::open(&path) {
            Ok(f) => {
                let mut bytes = Vec::new();
                f.take((STATE_CAP + 1) as u64).read_to_end(&mut bytes)?;
                if bytes.len() > STATE_CAP {
                    return Err(invalid("access state size cap"));
                }
                let s: Stored = serde_json::from_slice(&bytes)
                    .map_err(|_| invalid("malformed access state"))?;
                let epoch = fs::read_to_string(dir.join("access.epoch"))?;
                if s.version != 1
                    || s.config != config
                    || !equal(&s.installation_epoch, &epoch)
                    || s.fake_credential_key.len() != 64
                    || s.principals.len() > PRINCIPAL_CAP
                    || s.invitations.len() > PRINCIPAL_CAP
                    || s.credentials.len() > PRINCIPAL_CAP * 16
                    || s.audit.len() > AUDIT_CAP
                    || (s.pending_control.is_some() && !offline_reconcile)
                    || s.last_time > now
                {
                    return Err(invalid(
                        "access state configuration/epoch/bounds/time mismatch",
                    ));
                }
                for (id, p) in &s.principals {
                    if id != &p.id
                        || Uuid::parse_str(id).is_err()
                        || p.name.len() > 128
                        || p.passkeys.len() > 8
                        || (p.kind == PrincipalKind::Workload && !p.passkeys.is_empty())
                    {
                        return Err(invalid("invalid principal"));
                    }
                    p.scope.validate()?;
                }
                for (id, c) in &s.credentials {
                    if id != &c.id
                        || !s.principals.contains_key(&c.principal)
                        || c.hash.len() != 64
                        || c.audience != format!("{}#{}", config.audience, s.installation_epoch)
                        || c.expires <= c.issued
                        || c.expires - c.issued > 2592000
                    {
                        return Err(invalid("invalid credential"));
                    }
                    c.scope.validate()?;
                }
                s
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                // The durable anchor is created first: interrupted initialization requires owner repair.
                if dir.join("access.epoch").exists() {
                    return Err(invalid("missing access state requires offline recovery"));
                }
                let epoch = random()?;
                write_new(&dir.join("access.epoch"), epoch.as_bytes())?;
                File::open(dir)?.sync_all()?;
                let secret = random()?;
                write_new(&dir.join("access-bootstrap.secret"), secret.as_bytes())?;
                Stored {
                    version: 1,
                    config,
                    installation_epoch: epoch,
                    fake_credential_key: random()?,
                    policy: 1,
                    last_time: now,
                    bootstrap_hash: Some(hash(&secret)),
                    bootstrap_expires: now.saturating_add(600),
                    recovery_principal: None,
                    principals: BTreeMap::new(),
                    invitations: BTreeMap::new(),
                    credentials: BTreeMap::new(),
                    audit: Vec::new(),
                    audit_dropped: 0,
                    audit_next_id: 0,
                    pending_control: None,
                }
            }
            Err(e) => return Err(e),
        };
        let mut access = Self {
            dir: dir.to_path_buf(),
            state,
            webauthn,
            sessions: BTreeMap::new(),
            ceremonies: BTreeMap::new(),
            poisoned: false,
            last_time: now,
            denial_time: 0,
            denied_dropped: 0,
        };
        if !path.exists() {
            access.publish(access.state.clone())?;
        }
        Ok(access)
    }
    pub fn bootstrap_pending(&self) -> bool {
        self.state.bootstrap_hash.is_some()
    }
    pub fn origin(&self) -> &str {
        &self.state.config.origin
    }
    pub fn policy_version(&self) -> u64 {
        self.state.policy
    }
    fn clock(&mut self, now: u64) -> io::Result<()> {
        if self.poisoned {
            return Err(denied());
        }
        if now < self.last_time {
            self.poisoned = true;
            self.sessions.clear();
            self.ceremonies.clear();
            return Err(denied());
        }
        self.last_time = now;
        // Persist a monotonically advancing watermark before time-sensitive authority.
        // Restart at an earlier clock must not resurrect an expired bearer.
        if now > self.state.last_time {
            self.publish(self.state.clone())?;
        }
        self.sessions.retain(|_, s| {
            now < s.issued.saturating_add(SESSION_TTL) && now < s.seen.saturating_add(IDLE_TTL)
        });
        self.ceremonies.retain(|_, c| {
            now < match c {
                Ceremony::Registration { expires, .. } | Ceremony::Login { expires, .. } => {
                    *expires
                }
            }
        });
        Ok(())
    }
    fn publish(&mut self, mut next: Stored) -> io::Result<()> {
        if self.poisoned {
            return Err(denied());
        }
        next.last_time = self.last_time;
        next.audit_dropped = next.audit_dropped.saturating_add(self.denied_dropped);
        let bytes = serde_json::to_vec(&next).map_err(|_| invalid("access serialization"))?;
        if bytes.len() > STATE_CAP {
            return Err(invalid("access state cap"));
        }
        let temp = self.dir.join(format!("access-{}.new", random()?));
        let result = (|| {
            write_new(&temp, &bytes)?;
            fs::rename(&temp, self.dir.join("access.json"))?;
            File::open(&self.dir)?.sync_all()
        })();
        if let Err(e) = result {
            self.poisoned = true;
            self.sessions.clear();
            self.ceremonies.clear();
            let _ = fs::remove_file(temp);
            return Err(e);
        }
        self.state = next;
        self.denied_dropped = 0;
        Ok(())
    }
    fn event(
        next: &mut Stored,
        now: u64,
        actor: Option<&str>,
        delegator: Option<&str>,
        action: &str,
        resource: Option<&str>,
        outcome: &str,
    ) -> io::Result<()> {
        let event_id = next
            .audit_next_id
            .checked_add(1)
            .ok_or_else(|| invalid("audit event sequence exhausted"))?;
        next.audit_next_id = event_id;
        next.audit.retain(|e| now.saturating_sub(e.time) <= 2592000);
        if next.audit.len() >= AUDIT_CAP {
            next.audit.remove(0);
            next.audit_dropped = next.audit_dropped.saturating_add(1);
        }
        next.audit.push(AuditEvent {
            time: now,
            request_id: format!("{}-{event_id}", next.installation_epoch),
            actor: actor.map(str::to_owned),
            on_behalf_of: delegator.map(str::to_owned),
            action: action.to_owned(),
            resource: resource.map(str::to_owned),
            policy_version: next.policy,
            outcome: outcome.to_owned(),
        });
        Ok(())
    }
    pub fn start_registration(
        &mut self,
        bootstrap_secret: &str,
        name: &str,
        now: u64,
    ) -> io::Result<CeremonyStart> {
        self.clock(now)?;
        if name.is_empty()
            || name.len() > 128
            || self.ceremonies.len() >= CEREMONY_CAP
            || now >= self.state.bootstrap_expires
            || (!self.state.principals.is_empty() && self.state.recovery_principal.is_none())
            || !self
                .state
                .bootstrap_hash
                .as_ref()
                .is_some_and(|h| equal(h, &hash(bootstrap_secret)))
        {
            return Err(denied());
        }
        let id = self
            .state
            .recovery_principal
            .clone()
            .map_or_else(identifier, Ok)?;
        self.registration(id, name, true, 0, None, now)
    }
    fn registration(
        &mut self,
        id: String,
        name: &str,
        bootstrap: bool,
        epoch: u64,
        exclude: Option<Vec<CredentialID>>,
        now: u64,
    ) -> io::Result<CeremonyStart> {
        let uuid = Uuid::parse_str(&id).map_err(|_| denied())?;
        let (mut challenge, state) = self
            .webauthn
            .start_passkey_registration(uuid, &id, name, exclude)
            .map_err(|_| denied())?;
        require_resident_registration(&mut challenge)?;
        let ceremony_id = random()?;
        let public_key = serde_json::to_value(challenge).map_err(|_| denied())?;
        self.ceremonies.insert(
            ceremony_id.clone(),
            Ceremony::Registration {
                principal: id.clone(),
                name: name.to_owned(),
                bootstrap,
                epoch,
                expires: now.saturating_add(CHALLENGE_TTL),
                state,
            },
        );
        Ok(CeremonyStart {
            ceremony_id,
            principal_id: id,
            public_key,
        })
    }
    pub fn finish_registration(
        &mut self,
        id: &str,
        response: &RegisterPublicKeyCredential,
        now: u64,
    ) -> io::Result<SessionIssued> {
        self.clock(now)?;
        let Some(Ceremony::Registration {
            principal,
            name,
            bootstrap,
            epoch,
            state,
            ..
        }) = self.ceremonies.remove(id)
        else {
            return Err(denied());
        };
        let passkey = match self.webauthn.finish_passkey_registration(response, &state) {
            Ok(key) => key,
            Err(_) => {
                self.record_denial(None, "passkey.register", None, now)?;
                return Err(denied());
            }
        };
        if self.state.principals.values().any(|p| {
            p.passkeys
                .iter()
                .any(|key| key.cred_id() == passkey.cred_id())
        }) {
            return Err(denied());
        }
        let mut next = self.state.clone();
        if bootstrap {
            if next.bootstrap_hash.is_none() || now >= next.bootstrap_expires {
                return Err(denied());
            }
            if let Some(target) = next.recovery_principal.take() {
                if target != principal {
                    return Err(denied());
                }
                let p = next.principals.get_mut(&target).ok_or_else(denied)?;
                p.passkeys = vec![passkey];
                p.disabled = false;
            } else {
                if !next.principals.is_empty() {
                    return Err(denied());
                }
                next.principals.insert(
                    principal.clone(),
                    Principal {
                        id: principal.clone(),
                        name,
                        kind: PrincipalKind::Human,
                        disabled: false,
                        epoch: 1,
                        scope: Scope::owner(),
                        passkeys: vec![passkey],
                    },
                );
            }
            next.bootstrap_hash = None;
        } else {
            let p = next.principals.get_mut(&principal).ok_or_else(denied)?;
            if p.disabled || p.epoch != epoch || p.passkeys.len() >= 8 {
                return Err(denied());
            }
            p.passkeys.push(passkey);
            p.epoch = p.epoch.checked_add(1).ok_or_else(denied)?;
        }
        next.policy = next.policy.checked_add(1).ok_or_else(denied)?;
        Self::event(
            &mut next,
            now,
            Some(&principal),
            None,
            "passkey.register",
            Some(&principal),
            "success",
        )?;
        self.publish(next)?;
        self.sessions.retain(|_, s| s.principal != principal);
        if bootstrap {
            let _ = fs::remove_file(self.dir.join("access-bootstrap.secret"));
        }
        self.issue_session(&principal, now)
    }
    pub fn start_add_passkey(
        &mut self,
        auth: &Authority,
        csrf: &str,
        origin: &str,
        now: u64,
    ) -> io::Result<CeremonyStart> {
        self.validate_mutation(auth, csrf, origin, now, true)?;
        let p = self
            .state
            .principals
            .get(&auth.principal_id)
            .ok_or_else(denied)?;
        if p.kind != PrincipalKind::Human
            || p.passkeys.len() >= 8
            || self.ceremonies.len() >= CEREMONY_CAP
        {
            return Err(denied());
        }
        self.registration(
            p.id.clone(),
            &p.name.clone(),
            false,
            p.epoch,
            Some(p.passkeys.iter().map(|k| k.cred_id().clone()).collect()),
            now,
        )
    }
    pub fn start_login(&mut self, principal: &str, now: u64) -> io::Result<CeremonyStart> {
        self.clock(now)?;
        if self.ceremonies.len() >= CEREMONY_CAP || principal.len() > 128 {
            return Err(denied());
        }
        // Generate the same library challenge shape for missing/disabled accounts. Its empty
        // server-side credential list cannot verify a signature or grant authority.
        let known = self
            .state
            .principals
            .get(principal)
            .filter(|p| !p.disabled && p.kind == PrincipalKind::Human);
        let keys = known.map_or_else(Vec::new, |p| p.passkeys.clone());
        let epoch = known.map_or(0, |p| p.epoch);
        let generator: WebauthnFakeCredentialGenerator<FakePasskeyDistribution> =
            WebauthnFakeCredentialGenerator::new(self.state.fake_credential_key.as_bytes())
                .map_err(|_| denied())?;
        // Perform the fake generation for both paths to avoid a cheap lookup-only missing path.
        let mut fake = Vec::new();
        for index in 0..16 {
            let candidate = generator
                .generate(format!("{principal}:{index}").as_bytes())
                .map_err(|_| denied())?;
            if fake.is_empty() {
                fake = candidate;
            }
        }
        let (challenge, state) = self
            .webauthn
            .start_passkey_authentication(&keys)
            .map_err(|_| denied())?;
        let mut public_key = serde_json::to_value(challenge).map_err(|_| denied())?;
        if keys.is_empty() {
            let fake = if fake.is_empty() {
                vec![CredentialID::from(random()?.into_bytes())]
            } else {
                fake
            };
            public_key["publicKey"]["allowCredentials"] = serde_json::Value::Array(
                fake.into_iter()
                    .map(|id| serde_json::json!({"type":"public-key","id":id}))
                    .collect(),
            );
        }
        let ceremony_id = random()?;
        self.ceremonies.insert(
            ceremony_id.clone(),
            Ceremony::Login {
                principal: principal.to_owned(),
                epoch,
                expires: now.saturating_add(CHALLENGE_TTL),
                credential_state: passkey_revision(&keys)?,
                state,
            },
        );
        Ok(CeremonyStart {
            ceremony_id,
            principal_id: principal.to_owned(),
            public_key,
        })
    }
    pub fn finish_login(
        &mut self,
        id: &str,
        response: &PublicKeyCredential,
        now: u64,
    ) -> io::Result<SessionIssued> {
        self.clock(now)?;
        let Some(Ceremony::Login {
            principal,
            epoch,
            state,
            credential_state,
            ..
        }) = self.ceremonies.remove(id)
        else {
            return Err(denied());
        };
        let result = match self
            .webauthn
            .finish_passkey_authentication(response, &state)
        {
            Ok(result) => result,
            Err(_) => {
                self.record_denial(None, "login", None, now)?;
                return Err(denied());
            }
        };
        let mut next = self.state.clone();
        let p = next
            .principals
            .get_mut(&principal)
            .filter(|p| !p.disabled && p.epoch == epoch)
            .ok_or_else(denied)?;
        // The library verifies counters against its start-time credential
        // snapshot. A concurrent successful assertion must not let this older
        // snapshot authorize a counter below the latest durable credential.
        require_passkey_revision(&p.passkeys, &credential_state)?;
        let key = p
            .passkeys
            .iter_mut()
            .find(|k| k.cred_id() == result.cred_id())
            .ok_or_else(denied)?;
        key.update_credential(&result);
        Self::event(
            &mut next,
            now,
            Some(&principal),
            None,
            "login",
            None,
            "success",
        )?;
        self.publish(next)?;
        self.issue_session(&principal, now)
    }
    fn issue_session(&mut self, principal: &str, now: u64) -> io::Result<SessionIssued> {
        if self.sessions.len() >= SESSION_CAP {
            return Err(denied());
        }
        let token = random()?;
        let csrf = random()?;
        let p = self.state.principals.get(principal).ok_or_else(denied)?;
        self.sessions.insert(
            hash(&token),
            Session {
                principal: principal.to_owned(),
                epoch: p.epoch,
                policy: self.state.policy,
                csrf_hash: hash(&csrf),
                csrf_token: csrf.clone(),
                issued: now,
                seen: now,
                verified: now,
            },
        );
        Ok(SessionIssued {
            session_token: token,
            csrf_token: csrf,
            principal_id: principal.to_owned(),
            expires_unix_s: now.saturating_add(SESSION_TTL),
        })
    }
    pub fn authenticate_session(&mut self, token: &str, now: u64) -> io::Result<Authority> {
        self.clock(now)?;
        if token.len() != 64 {
            return Err(denied());
        }
        let digest = hash(token);
        let s = self.sessions.get_mut(&digest).ok_or_else(denied)?;
        let p = self
            .state
            .principals
            .get(&s.principal)
            .filter(|p| !p.disabled && p.epoch == s.epoch && s.policy == self.state.policy)
            .ok_or_else(denied)?;
        s.seen = now;
        Ok(Authority {
            principal_id: p.id.clone(),
            kind: p.kind,
            scope: p.scope.clone(),
            policy_version: self.state.policy,
            on_behalf_of: None,
            session_hash: Some(digest),
            credential_id: None,
            epoch: p.epoch,
        })
    }
    pub fn authenticate_workload(&mut self, token: &str, now: u64) -> io::Result<Authority> {
        self.clock(now)?;
        if token.len() != 64 {
            return Err(denied());
        }
        let digest = hash(token);
        let c = self
            .state
            .credentials
            .values()
            .find(|c| equal(&c.hash, &digest))
            .ok_or_else(denied)?;
        let p = self
            .state
            .principals
            .get(&c.principal)
            .filter(|p| !p.disabled && p.kind == PrincipalKind::Workload && p.epoch == c.epoch)
            .ok_or_else(denied)?;
        if now < c.issued
            || now >= c.expires
            || c.audience != self.audience()
            || !p.scope.contains(&c.scope)
        {
            return Err(denied());
        }
        if let Some(parent) = &c.on_behalf_of {
            self.delegation_parents(c, now)?;
            let parent = self
                .state
                .principals
                .get(parent)
                .filter(|h| !h.disabled && Some(h.epoch) == c.parent_epoch)
                .ok_or_else(denied)?;
            if c.policy != self.state.policy || !parent.scope.contains(&c.scope) {
                return Err(denied());
            }
        }
        Ok(Authority {
            principal_id: p.id.clone(),
            kind: p.kind,
            scope: c.scope.clone(),
            policy_version: self.state.policy,
            on_behalf_of: c.on_behalf_of.clone(),
            session_hash: None,
            credential_id: Some(c.id.clone()),
            epoch: p.epoch,
        })
    }
    pub fn recheck(&self, auth: &Authority) -> io::Result<()> {
        if self.poisoned || auth.policy_version != self.state.policy {
            return Err(denied());
        }
        let p = self
            .state
            .principals
            .get(&auth.principal_id)
            .filter(|p| !p.disabled && p.epoch == auth.epoch)
            .ok_or_else(denied)?;
        if !p.scope.contains(&auth.scope) {
            return Err(denied());
        }
        if auth
            .credential_id
            .as_ref()
            .is_some_and(|id| !self.state.credentials.contains_key(id))
        {
            return Err(denied());
        }
        if auth
            .session_hash
            .as_ref()
            .is_some_and(|id| !self.sessions.contains_key(id))
        {
            return Err(denied());
        }
        Ok(())
    }
    pub fn authorize(
        &self,
        auth: &Authority,
        action: Action,
        node: Option<&str>,
        signal: Option<Signal>,
    ) -> bool {
        self.recheck(auth).is_ok() && auth.scope.allows(action, node, signal)
    }
    pub fn validate_mutation(
        &mut self,
        auth: &Authority,
        csrf: &str,
        origin: &str,
        now: u64,
        fresh: bool,
    ) -> io::Result<()> {
        self.recheck_at(auth, now)?;
        if let Some(id) = &auth.session_hash {
            let s = self.sessions.get(id).ok_or_else(denied)?;
            if origin != self.origin()
                || !equal(&hash(csrf), &s.csrf_hash)
                || (fresh && now >= s.verified.saturating_add(FRESH_TTL))
            {
                return Err(denied());
            }
        } else if fresh {
            return Err(denied());
        }
        Ok(())
    }
    pub fn logout(
        &mut self,
        auth: &Authority,
        csrf: &str,
        origin: &str,
        now: u64,
    ) -> io::Result<()> {
        self.validate_mutation(auth, csrf, origin, now, false)?;
        if let Some(id) = &auth.session_hash {
            let mut next = self.state.clone();
            Self::event(
                &mut next,
                now,
                Some(&auth.principal_id),
                None,
                "session.logout",
                Some(&auth.principal_id),
                "success",
            )?;
            self.publish(next)?;
            self.sessions.remove(id);
        }
        Ok(())
    }
    /// After successful WebAuthn completion, retire only the previous browser
    /// cookie session. The replacement must already be a live issued session;
    /// an absent/unknown old cookie never obstructs successful authentication.
    pub fn retire_previous_session(
        &mut self,
        previous_token: Option<&str>,
        issued: &SessionIssued,
        now: u64,
    ) -> io::Result<()> {
        let replacement = self.authenticate_session(&issued.session_token, now)?;
        if replacement.principal_id != issued.principal_id {
            return Err(denied());
        }
        let Some(previous) = previous_token.filter(|t| t.len() == 64) else {
            return Ok(());
        };
        let previous_id = hash(previous);
        if previous_id == hash(&issued.session_token) {
            return Ok(());
        }
        let Some(old) = self.sessions.get(&previous_id) else {
            return Ok(());
        };
        let principal = old.principal.clone();
        let mut next = self.state.clone();
        Self::event(
            &mut next,
            now,
            Some(&principal),
            None,
            "session.replace",
            Some(&principal),
            "success",
        )?;
        self.publish(next)?;
        self.sessions.remove(&previous_id);
        Ok(())
    }
    /// Additions/rotation do not alter the issuer's grants. Advance only that
    /// live cookie to the published policy; captured Authority and cursors keep
    /// their old version and must fail revalidation.
    fn preserve_issuer_session(&mut self, auth: &Authority) {
        let Some(id) = &auth.session_hash else {
            return;
        };
        let Some(p) = self.state.principals.get(&auth.principal_id) else {
            return;
        };
        if p.disabled || p.epoch != auth.epoch || p.scope != auth.scope {
            return;
        }
        if let Some(s) = self
            .sessions
            .get_mut(id)
            .filter(|s| s.principal == auth.principal_id && s.epoch == p.epoch)
        {
            s.policy = self.state.policy;
        }
    }
    pub fn principals(&self, auth: &Authority) -> io::Result<Vec<PrincipalView>> {
        if !self.authorize(auth, Action::IdentityManage, None, None)
            || !auth.scope.installation_wide
        {
            return Err(denied());
        }
        Ok(self
            .state
            .principals
            .values()
            .map(|p| PrincipalView {
                id: p.id.clone(),
                name: p.name.clone(),
                kind: p.kind,
                disabled: p.disabled,
                scope: p.scope.clone(),
                passkey_count: p.passkeys.len(),
            })
            .collect())
    }
    pub fn audit(&self, auth: &Authority) -> io::Result<&[AuditEvent]> {
        if !self.authorize(auth, Action::AuditRead, None, None) || !auth.scope.installation_wide {
            return Err(denied());
        }
        Ok(&self.state.audit)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Authority, issuance fields and mutation proof are explicit security inputs."
    )]
    pub fn issue_workload(
        &mut self,
        auth: &Authority,
        name: &str,
        scope: Scope,
        ttl: u64,
        csrf: &str,
        origin: &str,
        now: u64,
    ) -> io::Result<CredentialIssued> {
        self.validate_mutation(auth, csrf, origin, now, true)?;
        if !self.authorize(auth, Action::GrantManage, None, None)
            || !self.authorize(auth, Action::IdentityManage, None, None)
            || !auth.scope.contains(&scope)
            || name.is_empty()
            || name.len() > 128
            || ttl == 0
            || ttl > 2592000
            || self.state.principals.len() >= PRINCIPAL_CAP
            || scope.actions.contains(&Action::IdentityManage)
            || scope.actions.contains(&Action::GrantManage)
        {
            return Err(denied());
        }
        scope.validate()?;
        let principal = identifier()?;
        let id = identifier()?;
        let token = random()?;
        let expires = now.saturating_add(ttl);
        let mut next = self.state.clone();
        next.policy = next.policy.checked_add(1).ok_or_else(denied)?;
        next.principals.insert(
            principal.clone(),
            Principal {
                id: principal.clone(),
                name: name.to_owned(),
                kind: PrincipalKind::Workload,
                disabled: false,
                epoch: 1,
                scope: scope.clone(),
                passkeys: vec![],
            },
        );
        next.credentials.insert(
            id.clone(),
            WorkloadCredential {
                id: id.clone(),
                principal: principal.clone(),
                hash: hash(&token),
                audience: format!("{}#{}", next.config.audience, next.installation_epoch),
                issued: now,
                expires,
                epoch: 1,
                scope,
                on_behalf_of: None,
                parent_epoch: None,
                parent_session: None,
                parent_credential: None,
                policy: next.policy,
            },
        );
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            None,
            "workload.issue",
            Some(&principal),
            "success",
        )?;
        self.publish(next)?;
        self.preserve_issuer_session(auth);
        Ok(CredentialIssued {
            principal_id: principal,
            credential_id: id,
            token,
            expires_unix_s: expires,
        })
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Authority, issuance fields and mutation proof are explicit security inputs."
    )]
    pub fn delegate(
        &mut self,
        human: &Authority,
        workload: &str,
        scope: Scope,
        ttl: u64,
        csrf: &str,
        origin: &str,
        now: u64,
    ) -> io::Result<CredentialIssued> {
        self.validate_mutation(human, csrf, origin, now, true)?;
        scope.validate()?;
        let p = self
            .state
            .principals
            .get(workload)
            .filter(|p| !p.disabled && p.kind == PrincipalKind::Workload)
            .ok_or_else(denied)?;
        if !human.scope.contains(&scope)
            || !p.scope.contains(&scope)
            || ttl == 0
            || ttl > 900
            || scope.actions.iter().any(|a| {
                matches!(
                    a,
                    Action::IdentityManage
                        | Action::GrantManage
                        | Action::NodeEnroll
                        | Action::NodeRevoke
                )
            })
            || self
                .state
                .credentials
                .values()
                .filter(|c| c.principal == workload)
                .count()
                >= 16
        {
            return Err(denied());
        }
        let parent_credential = self
            .state
            .credentials
            .values()
            .filter(|c| {
                c.principal == workload
                    && c.on_behalf_of.is_none()
                    && c.epoch == p.epoch
                    && c.expires > now
                    && c.scope.contains(&scope)
            })
            .max_by_key(|c| c.expires)
            .ok_or_else(denied)?;
        let parent_id = parent_credential.id.clone();
        let session_id = human.session_hash.clone().ok_or_else(denied)?;
        let session = self.sessions.get(&session_id).ok_or_else(denied)?;
        let expires = now
            .saturating_add(ttl)
            .min(parent_credential.expires)
            .min(session.issued.saturating_add(SESSION_TTL))
            .min(session.seen.saturating_add(IDLE_TTL));
        if expires <= now {
            return Err(denied());
        }
        let id = identifier()?;
        let token = random()?;
        let mut next = self.state.clone();
        next.credentials.insert(
            id.clone(),
            WorkloadCredential {
                id: id.clone(),
                principal: workload.to_owned(),
                hash: hash(&token),
                audience: format!("{}#{}", next.config.audience, next.installation_epoch),
                issued: now,
                expires,
                epoch: p.epoch,
                scope,
                on_behalf_of: Some(human.principal_id.clone()),
                parent_epoch: Some(human.epoch),
                parent_session: Some(session_id),
                parent_credential: Some(parent_id),
                policy: next.policy,
            },
        );
        Self::event(
            &mut next,
            now,
            Some(workload),
            Some(&human.principal_id),
            "delegation.issue",
            Some(workload),
            "success",
        )?;
        self.publish(next)?;
        Ok(CredentialIssued {
            principal_id: workload.to_owned(),
            credential_id: id,
            token,
            expires_unix_s: expires,
        })
    }
    pub fn set_scope(
        &mut self,
        auth: &Authority,
        principal: &str,
        scope: Scope,
        csrf: &str,
        origin: &str,
        now: u64,
    ) -> io::Result<()> {
        self.validate_mutation(auth, csrf, origin, now, true)?;
        scope.validate()?;
        if !self.authorize(auth, Action::GrantManage, None, None) || !auth.scope.contains(&scope) {
            return Err(denied());
        }
        let mut next = self.state.clone();
        let p = next.principals.get_mut(principal).ok_or_else(denied)?;
        if !auth.scope.contains(&p.scope) {
            return Err(denied());
        }
        if p.kind == PrincipalKind::Workload
            && (scope.actions.contains(&Action::GrantManage)
                || scope.actions.contains(&Action::IdentityManage))
        {
            return Err(denied());
        }
        p.scope = scope;
        p.epoch = p.epoch.checked_add(1).ok_or_else(denied)?;
        if !next.principals.values().any(|p| {
            !p.disabled
                && p.kind == PrincipalKind::Human
                && !p.passkeys.is_empty()
                && p.scope.actions.contains(&Action::IdentityManage)
                && p.scope.actions.contains(&Action::GrantManage)
        }) {
            return Err(denied());
        }
        next.policy = next.policy.checked_add(1).ok_or_else(denied)?;
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            None,
            "grant.change",
            Some(principal),
            "success",
        )?;
        self.publish(next)
    }
    pub fn disable(
        &mut self,
        auth: &Authority,
        principal: &str,
        csrf: &str,
        origin: &str,
        now: u64,
    ) -> io::Result<()> {
        self.validate_mutation(auth, csrf, origin, now, true)?;
        if !self.authorize(auth, Action::IdentityManage, None, None) {
            return Err(denied());
        }
        let mut next = self.state.clone();
        let p = next.principals.get_mut(principal).ok_or_else(denied)?;
        if !auth.scope.contains(&p.scope) {
            return Err(denied());
        }
        p.disabled = true;
        p.epoch = p.epoch.checked_add(1).ok_or_else(denied)?;
        if !next.principals.values().any(|p| {
            !p.disabled
                && p.kind == PrincipalKind::Human
                && !p.passkeys.is_empty()
                && p.scope.actions.contains(&Action::IdentityManage)
                && p.scope.actions.contains(&Action::GrantManage)
        }) {
            return Err(denied());
        }
        next.policy = next.policy.checked_add(1).ok_or_else(denied)?;
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            None,
            "identity.disable",
            Some(principal),
            "success",
        )?;
        self.publish(next)
    }
    pub fn revoke_credential(
        &mut self,
        auth: &Authority,
        id: &str,
        csrf: &str,
        origin: &str,
        now: u64,
    ) -> io::Result<()> {
        self.validate_mutation(auth, csrf, origin, now, true)?;
        if !self.authorize(auth, Action::GrantManage, None, None) {
            return Err(denied());
        }
        let c = self.state.credentials.get(id).ok_or_else(denied)?;
        if !auth.scope.contains(&c.scope) {
            return Err(denied());
        }
        let mut next = self.state.clone();
        next.credentials.remove(id);
        next.policy = next.policy.checked_add(1).ok_or_else(denied)?;
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            None,
            "credential.revoke",
            Some(id),
            "success",
        )?;
        self.publish(next)
    }
    /// Publish durable intent before the caller touches control state while holding the access lock.
    /// An interrupted intent makes reopen fail closed; offline inspection reconciles both files.
    pub fn begin_control(
        &mut self,
        auth: &Authority,
        action: Action,
        enrollment: Option<&str>,
        resource: &str,
        now: u64,
    ) -> io::Result<String> {
        self.recheck_at(auth, now)?;
        if resource.len() > 128
            || !self.authorize(auth, action, enrollment, None)
            || self.state.pending_control.is_some()
        {
            return Err(denied());
        }
        let id = random()?;
        let mut next = self.state.clone();
        next.pending_control = Some(ControlIntent {
            id: id.clone(),
            action,
            resource: resource.to_owned(),
            actor: auth.principal_id.clone(),
            on_behalf_of: auth.on_behalf_of.clone(),
        });
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            auth.on_behalf_of.as_deref(),
            action.label(),
            Some(resource),
            "pending",
        )?;
        next.audit.last_mut().ok_or_else(denied)?.request_id = id.clone();
        self.publish(next)?;
        Ok(id)
    }
    pub fn finish_control(
        &mut self,
        id: &str,
        auth: &Authority,
        resource: &str,
        success: bool,
        now: u64,
    ) -> io::Result<()> {
        self.clock(now)?;
        let intent = self.state.pending_control.clone().ok_or_else(denied)?;
        if resource.len() > 128
            || intent.id != id
            || intent.actor != auth.principal_id
            || intent.resource != resource
            || intent.on_behalf_of != auth.on_behalf_of
        {
            return Err(denied());
        }
        let mut next = self.state.clone();
        next.pending_control = None;
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            auth.on_behalf_of.as_deref(),
            intent.action.label(),
            Some(resource),
            if success { "success" } else { "failed" },
        )?;
        next.audit.last_mut().ok_or_else(denied)?.request_id = intent.id;
        self.publish(next)
    }
    /// Explicitly refuse further authority when control publication is uncertain.
    pub fn quarantine(&mut self) {
        self.poisoned = true;
        self.sessions.clear();
        self.ceremonies.clear();
    }

    /// Only call while the composition root holds the stopped service's exclusive state lock.
    /// Rotates the installation epoch, invalidates machine secrets, and permits one verified
    /// passkey replacement for the existing owner ID. It cannot repair malformed state.
    pub fn recover_offline(
        dir: &Path,
        config: AccessConfig,
        principal: &str,
        now: u64,
    ) -> io::Result<String> {
        let access = Self::open(dir, config, now)?;
        let p = access
            .state
            .principals
            .get(principal)
            .filter(|p| {
                p.kind == PrincipalKind::Human
                    && p.scope.installation_wide
                    && p.scope.actions.contains(&Action::IdentityManage)
                    && p.scope.actions.contains(&Action::GrantManage)
            })
            .ok_or_else(denied)?;
        let target = p.id.clone();
        let token = random()?;
        let epoch = random()?;
        let mut next = access.state.clone();
        for p in next.principals.values_mut() {
            p.epoch = p.epoch.checked_add(1).ok_or_else(denied)?;
        }
        next.credentials.clear();
        next.invitations.clear();
        next.last_time = now;
        next.principals
            .get_mut(&target)
            .ok_or_else(denied)?
            .passkeys
            .clear();
        next.principals
            .get_mut(&target)
            .ok_or_else(denied)?
            .disabled = true;
        next.bootstrap_hash = Some(hash(&token));
        next.bootstrap_expires = now.saturating_add(600);
        next.recovery_principal = Some(target.clone());
        next.installation_epoch = epoch.clone();
        next.policy = next.policy.checked_add(1).ok_or_else(denied)?;
        Self::event(
            &mut next,
            now,
            Some(&target),
            None,
            "owner.recovery",
            Some(&target),
            "one_use_enrollment",
        )?;
        let intent = RecoveryIntent {
            version: 1,
            old_epoch: access.state.installation_epoch.clone(),
            old_state_hash: hex_bytes(&Sha256::digest(fs::read(dir.join("access.json"))?)),
            next,
            secret: token.clone(),
        };
        let bytes = serde_json::to_vec(&intent).map_err(|_| invalid("recovery serialization"))?;
        if bytes.len() > STATE_CAP + 4096 {
            return Err(invalid("recovery intent cap"));
        }
        write_new(&dir.join("access-recovery.intent"), &bytes)?;
        File::open(dir)?.sync_all()?;
        Self::apply_recovery(dir, &intent, |_| Ok(()))?;
        Ok(token)
    }
    pub fn revoke_sessions(
        &mut self,
        auth: &Authority,
        principal: &str,
        csrf: &str,
        origin: &str,
        now: u64,
    ) -> io::Result<()> {
        self.validate_mutation(auth, csrf, origin, now, true)?;
        if principal != auth.principal_id
            && !self.authorize(auth, Action::IdentityManage, None, None)
        {
            return Err(denied());
        }
        let p = self.state.principals.get(principal).ok_or_else(denied)?;
        if principal != auth.principal_id && !auth.scope.contains(&p.scope) {
            return Err(denied());
        }
        let mut next = self.state.clone();
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            None,
            "sessions.revoke",
            Some(principal),
            "success",
        )?;
        self.publish(next)?;
        self.sessions.retain(|_, s| s.principal != principal);
        Ok(())
    }
    /// Returned session IDs are random revocation handles, never reusable authentication tokens.
    pub fn sessions(&self, auth: &Authority) -> io::Result<Vec<SessionView>> {
        self.recheck(auth)?;
        Ok(self
            .sessions
            .iter()
            .filter(|(_, s)| s.principal == auth.principal_id)
            .map(|(id, s)| SessionView {
                id: id.clone(),
                issued_unix_s: s.issued,
                last_seen_unix_s: s.seen,
                expires_unix_s: s.issued.saturating_add(SESSION_TTL),
            })
            .collect())
    }
    pub fn revoke_session(
        &mut self,
        auth: &Authority,
        id: &str,
        csrf: &str,
        origin: &str,
        now: u64,
    ) -> io::Result<()> {
        self.validate_mutation(auth, csrf, origin, now, true)?;
        let s = self
            .sessions
            .get(id)
            .filter(|s| s.principal == auth.principal_id)
            .ok_or_else(denied)?;
        let principal = s.principal.clone();
        let mut next = self.state.clone();
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            None,
            "session.revoke",
            Some(&principal),
            "success",
        )?;
        self.publish(next)?;
        self.sessions.remove(id);
        Ok(())
    }
    pub fn revoke_passkey(
        &mut self,
        auth: &Authority,
        credential_id: &str,
        csrf: &str,
        origin: &str,
        now: u64,
    ) -> io::Result<()> {
        self.validate_mutation(auth, csrf, origin, now, true)?;
        let mut next = self.state.clone();
        let p = next
            .principals
            .get_mut(&auth.principal_id)
            .ok_or_else(denied)?;
        if p.passkeys.len() <= 1 {
            return Err(denied());
        }
        let before = p.passkeys.len();
        p.passkeys.retain(|k| {
            serde_json::to_value(k.cred_id())
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .as_deref()
                != Some(credential_id)
        });
        if before == p.passkeys.len() {
            return Err(denied());
        }
        p.epoch = p.epoch.checked_add(1).ok_or_else(denied)?;
        next.policy = next.policy.checked_add(1).ok_or_else(denied)?;
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            None,
            "passkey.revoke",
            Some(&auth.principal_id),
            "success",
        )?;
        self.publish(next)?;
        self.sessions
            .retain(|_, s| s.principal != auth.principal_id);
        Ok(())
    }
    pub fn passkey_ids(&self, auth: &Authority) -> io::Result<Vec<serde_json::Value>> {
        self.recheck(auth)?;
        self.state
            .principals
            .get(&auth.principal_id)
            .ok_or_else(denied)?
            .passkeys
            .iter()
            .map(|k| {
                serde_json::to_value(k.cred_id()).map_err(|_| invalid("credential serialization"))
            })
            .collect()
    }

    pub fn session_view(&self, auth: &Authority) -> io::Result<serde_json::Value> {
        self.recheck(auth)?;
        let s = self
            .sessions
            .get(auth.session_hash.as_ref().ok_or_else(denied)?)
            .ok_or_else(denied)?;
        let p = self
            .state
            .principals
            .get(&auth.principal_id)
            .ok_or_else(denied)?;
        Ok(
            serde_json::json!({"api_version":1,"principal_id":p.id,"display_name":p.name,"kind":p.kind,"actions":auth.scope.actions,"csrf":s.csrf_token,
            "scope":auth.scope,"policy_version":auth.policy_version,"csrf_token":s.csrf_token,
            "expires_unix_s":s.issued.saturating_add(SESSION_TTL),"fresh_until_unix_s":s.verified.saturating_add(FRESH_TTL)}),
        )
    }

    /// Revalidate lifetime as well as epoch immediately before releasing a long-running read.
    pub fn recheck_at(&mut self, auth: &Authority, now: u64) -> io::Result<()> {
        self.clock(now)?;
        self.recheck(auth)?;
        if let Some(id) = &auth.credential_id {
            let c = self.state.credentials.get(id).ok_or_else(denied)?;
            if now < c.issued || now >= c.expires {
                return Err(denied());
            }
            if let Some(parent) = &c.on_behalf_of {
                self.delegation_parents(c, now)?;
                let p = self
                    .state
                    .principals
                    .get(parent)
                    .filter(|p| !p.disabled && Some(p.epoch) == c.parent_epoch)
                    .ok_or_else(denied)?;
                if !p.scope.contains(&c.scope) || c.policy != self.state.policy {
                    return Err(denied());
                }
            }
        }
        Ok(())
    }
    /// Root must first reopen/validate the durable control state under the stopped-service lock.
    /// The digest records the exact control file inspected; no control transition is replayed.
    pub fn recover_offline_reconciled(
        dir: &Path,
        config: AccessConfig,
        principal: &str,
        control_sha256: &str,
        now: u64,
    ) -> io::Result<String> {
        if control_sha256.len() != 64 || !control_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid("control evidence digest"));
        }
        let mut access = Self::open_mode(dir, config.clone(), now, true)?;
        let mut next = access.state.clone();
        next.pending_control = None;
        Self::event(
            &mut next,
            now,
            Some(principal),
            None,
            "control.offline_reconcile",
            Some(control_sha256),
            "operator_inspected",
        )?;
        access.publish(next)?;
        Self::recover_offline(dir, config, principal, now)
    }
    pub fn audit_dropped(&self) -> u64 {
        self.state.audit_dropped.saturating_add(self.denied_dropped)
    }
    /// Call from the request boundary using fixed actions and immutable scoped resource IDs.
    /// Never pass a bearer, client error, URL, log path, telemetry or challenge as resource.
    pub fn record_denial(
        &mut self,
        auth: Option<&Authority>,
        action: &str,
        resource: Option<&str>,
        now: u64,
    ) -> io::Result<()> {
        self.clock(now)?;
        if action.len() > 64 || resource.is_some_and(|s| s.len() > 128) {
            return Err(invalid("audit field bound"));
        }
        if now == self.denial_time {
            self.denied_dropped = self.denied_dropped.saturating_add(1);
            return Ok(());
        }
        self.denial_time = now;
        let mut next = self.state.clone();
        Self::event(
            &mut next,
            now,
            auth.map(|a| a.principal_id.as_str()),
            auth.and_then(|a| a.on_behalf_of.as_deref()),
            action,
            resource,
            "denied",
        )?;
        self.publish(next)
    }

    pub fn invite_human(
        &mut self,
        auth: &Authority,
        name: &str,
        scope: Scope,
        csrf: &str,
        origin: &str,
        now: u64,
    ) -> io::Result<InvitationIssued> {
        self.validate_mutation(auth, csrf, origin, now, true)?;
        scope.validate()?;
        if !self.authorize(auth, Action::IdentityManage, None, None)
            || !self.authorize(auth, Action::GrantManage, None, None)
            || !auth.scope.contains(&scope)
            || name.is_empty()
            || name.len() > 128
            || self.state.principals.len() >= PRINCIPAL_CAP
        {
            return Err(denied());
        }
        let id = identifier()?;
        let invitation = random()?;
        let mut next = self.state.clone();
        next.invitations.retain(|_, v| v.expires > now);
        next.principals.insert(
            id.clone(),
            Principal {
                id: id.clone(),
                name: name.to_owned(),
                kind: PrincipalKind::Human,
                disabled: true,
                epoch: 1,
                scope,
                passkeys: vec![],
            },
        );
        next.invitations.insert(
            id.clone(),
            Invitation {
                principal: id.clone(),
                hash: hash(&invitation),
                expires: now.saturating_add(600),
                epoch: 1,
            },
        );
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            None,
            "human.invite",
            Some(&id),
            "success",
        )?;
        self.publish(next)?;
        self.preserve_issuer_session(auth);
        Ok(InvitationIssued {
            principal_id: id,
            invitation,
            expires_unix_s: now.saturating_add(600),
        })
    }
    pub fn start_invited_registration(
        &mut self,
        secret: &str,
        now: u64,
    ) -> io::Result<CeremonyStart> {
        self.clock(now)?;
        if secret.len() != 64 || self.ceremonies.len() >= CEREMONY_CAP {
            return Err(denied());
        }
        let digest = hash(secret);
        let invitation = self
            .state
            .invitations
            .values()
            .find(|v| equal(&v.hash, &digest) && v.expires > now)
            .cloned()
            .ok_or_else(denied)?;
        let mut next = self.state.clone();
        next.invitations.remove(&invitation.principal);
        let p = next
            .principals
            .get_mut(&invitation.principal)
            .filter(|p| p.epoch == invitation.epoch && p.passkeys.is_empty())
            .ok_or_else(denied)?;
        p.disabled = false;
        let name = p.name.clone();
        Self::event(
            &mut next,
            now,
            Some(&invitation.principal),
            None,
            "human.invitation_consume",
            Some(&invitation.principal),
            "success",
        )?;
        self.publish(next)?;
        self.registration(
            invitation.principal,
            &name,
            false,
            invitation.epoch,
            None,
            now,
        )
    }

    fn replay_recovery(dir: &Path, config: &AccessConfig, now: u64) -> io::Result<()> {
        let mut bytes = Vec::new();
        File::open(dir.join("access-recovery.intent"))?
            .take((STATE_CAP + 4097) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > STATE_CAP + 4096 {
            return Err(invalid("recovery intent cap"));
        }
        let intent: RecoveryIntent =
            serde_json::from_slice(&bytes).map_err(|_| invalid("malformed recovery intent"))?;
        let epoch = fs::read_to_string(dir.join("access.epoch"))?;
        let mut current = Vec::new();
        File::open(dir.join("access.json"))?
            .take((STATE_CAP + 1) as u64)
            .read_to_end(&mut current)?;
        let next = serde_json::to_vec(&intent.next)
            .map_err(|_| invalid("recovery state serialization"))?;
        if intent.version != 1
            || intent.next.version != 1
            || intent.next.config != *config
            || intent.old_epoch.len() != 64
            || intent.next.installation_epoch.len() != 64
            || intent.next.installation_epoch == intent.old_epoch
            || intent.secret.len() != 64
            || intent.next.last_time > now
            || !intent.next.credentials.is_empty()
            || !intent.next.invitations.is_empty()
            || intent.next.bootstrap_hash.as_deref() != Some(hash(&intent.secret).as_str())
            || match intent.next.recovery_principal.as_ref() {
                Some(p) => !intent.next.principals.get(p).is_some_and(|p| {
                    p.kind == PrincipalKind::Human && p.disabled && p.passkeys.is_empty()
                }),
                None => !intent.next.principals.is_empty(),
            }
            || (epoch != intent.old_epoch && epoch != intent.next.installation_epoch)
            || (hex_bytes(&Sha256::digest(&current)) != intent.old_state_hash && current != next)
            || next.len() > STATE_CAP
        {
            return Err(invalid("recovery intent/state disagreement"));
        }
        Self::apply_recovery(dir, &intent, |_| Ok(()))
    }
    fn apply_recovery(
        dir: &Path,
        intent: &RecoveryIntent,
        mut boundary: impl FnMut(u8) -> io::Result<()>,
    ) -> io::Result<()> {
        boundary(0)?;
        let next = serde_json::to_vec(&intent.next)
            .map_err(|_| invalid("recovery state serialization"))?;
        for (index, (name, bytes)) in [
            ("access.json", next.as_slice()),
            ("access.epoch", intent.next.installation_epoch.as_bytes()),
            ("access-bootstrap.secret", intent.secret.as_bytes()),
        ]
        .into_iter()
        .enumerate()
        {
            let temp = dir.join(format!("recovery-{}.new", random()?));
            let result = (|| {
                write_new(&temp, bytes)?;
                fs::rename(&temp, dir.join(name))?;
                File::open(dir)?.sync_all()
            })();
            if result.is_err() {
                let _ = fs::remove_file(temp);
            }
            result?;
            boundary(index as u8 + 1)?;
        }
        fs::remove_file(dir.join("access-recovery.intent"))?;
        File::open(dir)?.sync_all()?;
        boundary(4)
    }

    /// Rotate a workload credential without changing the persistent actor ID. Old live
    /// credentials overlap for at most ten minutes; delegated credentials lose policy authority.
    pub fn rotate_workload(
        &mut self,
        auth: &Authority,
        principal: &str,
        ttl: u64,
        csrf: &str,
        origin: &str,
        now: u64,
    ) -> io::Result<CredentialIssued> {
        self.validate_mutation(auth, csrf, origin, now, true)?;
        if !self.authorize(auth, Action::GrantManage, None, None) || ttl == 0 || ttl > 2592000 {
            return Err(denied());
        }
        let p = self
            .state
            .principals
            .get(principal)
            .filter(|p| !p.disabled && p.kind == PrincipalKind::Workload)
            .ok_or_else(denied)?;
        if !auth.scope.contains(&p.scope) {
            return Err(denied());
        }
        let epoch = p.epoch;
        let scope = p.scope.clone();
        let mut next = self.state.clone();
        next.credentials
            .retain(|_, c| c.expires > now && (c.principal != principal || c.epoch == epoch));
        if next
            .credentials
            .values()
            .filter(|c| c.principal == principal)
            .count()
            >= 16
        {
            return Err(denied());
        }
        for c in next
            .credentials
            .values_mut()
            .filter(|c| c.principal == principal)
        {
            c.expires = c.expires.min(now.saturating_add(600));
        }
        let id = identifier()?;
        let token = random()?;
        let expires = now.saturating_add(ttl);
        next.policy = next.policy.checked_add(1).ok_or_else(denied)?;
        next.credentials.insert(
            id.clone(),
            WorkloadCredential {
                id: id.clone(),
                principal: principal.to_owned(),
                hash: hash(&token),
                audience: format!("{}#{}", next.config.audience, next.installation_epoch),
                issued: now,
                expires,
                epoch,
                scope,
                on_behalf_of: None,
                parent_epoch: None,
                parent_session: None,
                parent_credential: None,
                policy: next.policy,
            },
        );
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            None,
            "workload.rotate",
            Some(principal),
            "success",
        )?;
        self.publish(next)?;
        self.preserve_issuer_session(auth);
        Ok(CredentialIssued {
            principal_id: principal.to_owned(),
            credential_id: id,
            token,
            expires_unix_s: expires,
        })
    }
    /// Renew only an unfinished first-owner setup while the stopped-service lock is held.
    /// Existing humans (including disabled accounts) make this operation unavailable.
    pub fn renew_bootstrap_offline(
        dir: &Path,
        config: AccessConfig,
        now: u64,
    ) -> io::Result<String> {
        let access = Self::open(dir, config, now)?;
        if !access.state.principals.is_empty()
            || !access.state.credentials.is_empty()
            || access.state.bootstrap_hash.is_none()
            || access.state.recovery_principal.is_some()
        {
            return Err(denied());
        }
        let token = random()?;
        let mut next = access.state.clone();
        next.installation_epoch = random()?;
        next.bootstrap_hash = Some(hash(&token));
        next.bootstrap_expires = now.saturating_add(600);
        next.policy = next.policy.checked_add(1).ok_or_else(denied)?;
        next.last_time = now;
        Self::event(
            &mut next,
            now,
            None,
            None,
            "bootstrap.offline_renew",
            None,
            "success",
        )?;
        let intent = RecoveryIntent {
            version: 1,
            old_epoch: access.state.installation_epoch.clone(),
            old_state_hash: hex_bytes(&Sha256::digest(fs::read(dir.join("access.json"))?)),
            next,
            secret: token.clone(),
        };
        let bytes = serde_json::to_vec(&intent).map_err(|_| invalid("recovery serialization"))?;
        if bytes.len() > STATE_CAP + 4096 {
            return Err(invalid("recovery intent cap"));
        }
        write_new(&dir.join("access-recovery.intent"), &bytes)?;
        File::open(dir)?.sync_all()?;
        Self::apply_recovery(dir, &intent, |_| Ok(()))?;
        Ok(token)
    }
    fn delegation_parents(&self, c: &WorkloadCredential, now: u64) -> io::Result<()> {
        let session = self
            .sessions
            .get(c.parent_session.as_ref().ok_or_else(denied)?)
            .ok_or_else(denied)?;
        let credential = self
            .state
            .credentials
            .get(c.parent_credential.as_ref().ok_or_else(denied)?)
            .ok_or_else(denied)?;
        if Some(&session.principal) != c.on_behalf_of.as_ref()
            || Some(session.epoch) != c.parent_epoch
            || session.policy != self.state.policy
            || now >= session.issued.saturating_add(SESSION_TTL)
            || now >= session.seen.saturating_add(IDLE_TTL)
            || credential.principal != c.principal
            || credential.epoch != c.epoch
            || credential.on_behalf_of.is_some()
            || now < credential.issued
            || now >= credential.expires
            || !credential.scope.contains(&c.scope)
        {
            return Err(denied());
        }
        Ok(())
    }
    /// HTTP policy test fixture only. It establishes a synthetic authenticated human;
    /// it provides no evidence about signatures, user verification or browser ceremonies.
    #[cfg(test)]
    pub(crate) fn fixture_session(&mut self, scope: Scope, now: u64) -> io::Result<SessionIssued> {
        self.clock(now)?;
        scope.validate()?;
        let id = identifier()?;
        let mut next = self.state.clone();
        next.bootstrap_hash = None;
        next.principals.insert(
            id.clone(),
            Principal {
                id: id.clone(),
                name: "policy-fixture".into(),
                kind: PrincipalKind::Human,
                disabled: false,
                epoch: 1,
                scope,
                passkeys: vec![],
            },
        );
        self.publish(next)?;
        self.issue_session(&id, now)
    }
}

#[derive(Serialize)]
pub struct InvitationIssued {
    pub principal_id: String,
    pub invitation: String,
    pub expires_unix_s: u64,
}
#[derive(Serialize)]
pub struct SessionView {
    pub id: String,
    pub issued_unix_s: u64,
    pub last_seen_unix_s: u64,
    pub expires_unix_s: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let root = std::env::var_os("FABRIC_SCRATCH_ROOT")
                .expect("resource launcher scratch required");
            let p = PathBuf::from(root).join(format!("access-tests-{}", random().unwrap()));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn config() -> AccessConfig {
        AccessConfig {
            origin: "https://fabric.example".into(),
            rp_id: "fabric.example".into(),
            audience: "installation-1".into(),
        }
    }
    fn owner(access: &mut Access, now: u64) -> SessionIssued {
        access.fixture_session(Scope::owner(), now).unwrap()
    }
    fn read_scope() -> Scope {
        let mut s = Scope::owner();
        s.installation_wide = false;
        s.actions = [Action::TelemetryRead].into();
        s.enrollments = ["enrollment-a".to_owned()].into();
        s.signals = [Signal::Logs].into();
        s
    }
    #[test]
    fn resident_registration_options_cover_bootstrap_add_invitation_and_recovery() {
        // Origin: Firefox diagnostic05 stored a nonresident key despite the
        // console protocol's resident-credential requirement. All enrollment
        // entrypoints must request resident credentials without weakening UV.
        fn assert_options(start: &CeremonyStart) {
            let key = &start.public_key["publicKey"];
            let selection = &key["authenticatorSelection"];
            assert_eq!(selection["residentKey"], "required");
            assert_eq!(selection["requireResidentKey"], true);
            assert_eq!(selection["userVerification"], "required");
            assert!(selection.get("authenticatorAttachment").is_none());
            assert_eq!(key["rp"]["id"], "fabric.example");
            assert!(!key["challenge"].as_str().unwrap().is_empty());
        }
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let bootstrap = fs::read_to_string(dir.0.join("access-bootstrap.secret")).unwrap();
        assert_options(&a.start_registration(&bootstrap, "owner", 100).unwrap());
        let session = owner(&mut a, 100);
        let human = a.authenticate_session(&session.session_token, 100).unwrap();
        assert_options(
            &a.start_add_passkey(&human, &session.csrf_token, "https://fabric.example", 100)
                .unwrap(),
        );
        let invite = a
            .invite_human(
                &human,
                "reader",
                read_scope(),
                &session.csrf_token,
                "https://fabric.example",
                100,
            )
            .unwrap();
        assert_options(
            &a.start_invited_registration(&invite.invitation, 100)
                .unwrap(),
        );
        drop(a);
        let recovery =
            Access::recover_offline(&dir.0, config(), &session.principal_id, 101).unwrap();
        let mut a = Access::open(&dir.0, config(), 101).unwrap();
        let start = a.start_registration(&recovery, "owner", 101).unwrap();
        assert_eq!(start.principal_id, session.principal_id);
        assert_options(&start);
    }
    #[test]
    fn resident_registration_preserves_library_options_and_rejects_missing_selection() {
        let dir = Scratch::new();
        let a = Access::open(&dir.0, config(), 100).unwrap();
        let (mut challenge, _) = a
            .webauthn
            .start_passkey_registration(
                Uuid::new_v4(),
                "owner-id",
                "owner",
                Some(vec![vec![7; 32].into()]),
            )
            .unwrap();
        let mut expected = serde_json::to_value(&challenge).unwrap();
        expected["publicKey"]["authenticatorSelection"]["residentKey"] =
            serde_json::json!("required");
        expected["publicKey"]["authenticatorSelection"]["requireResidentKey"] =
            serde_json::json!(true);
        require_resident_registration(&mut challenge).unwrap();
        // Challenge, RP/user identifiers, excluded keys, UV, attachment,
        // algorithms and extensions must remain exactly library-generated.
        assert_eq!(serde_json::to_value(&challenge).unwrap(), expected);
        challenge.public_key.authenticator_selection = None;
        let unchanged = serde_json::to_value(&challenge).unwrap();
        assert_eq!(
            require_resident_registration(&mut challenge)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(serde_json::to_value(&challenge).unwrap(), unchanged);
    }
    #[test]
    fn control_audit_binds_actual_action_actor_resource_and_transition_request_id() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let owner = owner(&mut a, 100);
        let auth = a.authenticate_session(&owner.session_token, 100).unwrap();
        let other = a.fixture_session(Scope::owner(), 100).unwrap();
        let other_auth = a.authenticate_session(&other.session_token, 100).unwrap();
        let id = a
            .begin_control(
                &auth,
                Action::NodePause,
                Some("enrollment-a"),
                "enrollment-a",
                100,
            )
            .unwrap();
        let pending = a.state.audit.last().unwrap();
        assert_eq!(pending.action, "node.pause");
        assert_eq!(pending.request_id, id);
        assert_eq!(pending.outcome, "pending");
        assert!(
            a.finish_control(&id, &other_auth, "enrollment-a", true, 100)
                .is_err()
        );
        assert!(
            a.finish_control(&id, &auth, "enrollment-b", true, 100)
                .is_err()
        );
        assert!(a.state.pending_control.is_some());
        a.finish_control(&id, &auth, "enrollment-a", true, 100)
            .unwrap();
        let complete = a.state.audit.last().unwrap();
        assert_eq!(complete.action, "node.pause");
        assert_eq!(complete.request_id, id);
        assert_eq!(complete.outcome, "success");
        assert_eq!(complete.actor.as_deref(), Some(auth.principal_id.as_str()));
        assert_eq!(complete.resource.as_deref(), Some("enrollment-a"));
        assert!(a.state.pending_control.is_none());
    }
    #[test]
    fn delegated_authority_cannot_outlive_either_parent_session_or_credential() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let owner = owner(&mut a, 100);
        let human = a.authenticate_session(&owner.session_token, 100).unwrap();
        let credential = a
            .issue_workload(
                &human,
                "ai",
                read_scope(),
                100,
                &owner.csrf_token,
                "https://fabric.example",
                100,
            )
            .unwrap();
        let owner = a.issue_session(&owner.principal_id, 100).unwrap();
        let human = a.authenticate_session(&owner.session_token, 100).unwrap();
        let delegated = a
            .delegate(
                &human,
                &credential.principal_id,
                read_scope(),
                900,
                &owner.csrf_token,
                "https://fabric.example",
                100,
            )
            .unwrap();
        assert_eq!(delegated.expires_unix_s, 200);
        assert!(a.authenticate_workload(&delegated.token, 100).is_ok());
        a.logout(&human, &owner.csrf_token, "https://fabric.example", 100)
            .unwrap();
        assert!(a.authenticate_workload(&delegated.token, 100).is_err());
        assert!(a.authenticate_workload(&credential.token, 100).is_ok());
        drop(a);
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        assert!(a.authenticate_workload(&delegated.token, 100).is_err());
    }
    #[test]
    fn workload_rotation_preserves_actor_and_bounds_overlap() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let owner = owner(&mut a, 100);
        let human = a.authenticate_session(&owner.session_token, 100).unwrap();
        let old = a
            .issue_workload(
                &human,
                "reader",
                read_scope(),
                86400,
                &owner.csrf_token,
                "https://fabric.example",
                100,
            )
            .unwrap();
        let owner = a.issue_session(&owner.principal_id, 200).unwrap();
        let human = a.authenticate_session(&owner.session_token, 200).unwrap();
        let new = a
            .rotate_workload(
                &human,
                &old.principal_id,
                86400,
                &owner.csrf_token,
                "https://fabric.example",
                200,
            )
            .unwrap();
        assert_eq!(new.principal_id, old.principal_id);
        assert_ne!(new.credential_id, old.credential_id);
        assert!(a.authenticate_workload(&old.token, 799).is_ok());
        assert!(a.authenticate_workload(&old.token, 800).is_err());
        assert_eq!(
            a.authenticate_workload(&new.token, 800)
                .unwrap()
                .principal_id,
            old.principal_id
        );
        assert_eq!(
            a.state.credentials[&new.credential_id].audience,
            format!("installation-1#{}", a.state.installation_epoch)
        );
        let encoded = serde_json::to_string(&a.state).unwrap();
        assert!(!encoded.contains(&old.token));
        assert!(!encoded.contains(&new.token));
    }
    #[test]
    fn expired_first_bootstrap_can_only_be_renewed_by_offline_owner_operation() {
        let dir = Scratch::new();
        let a = Access::open(&dir.0, config(), 100).unwrap();
        let old = fs::read_to_string(dir.0.join("access-bootstrap.secret")).unwrap();
        let old_epoch = a.state.installation_epoch.clone();
        drop(a);
        let new = Access::renew_bootstrap_offline(&dir.0, config(), 700).unwrap();
        let mut a = Access::open(&dir.0, config(), 700).unwrap();
        assert_ne!(a.state.installation_epoch, old_epoch);
        assert!(a.start_registration(&old, "owner", 700).is_err());
        assert!(a.start_registration(&new, "owner", 700).is_ok());
        let _owner = owner(&mut a, 700);
        drop(a);
        assert!(Access::renew_bootstrap_offline(&dir.0, config(), 700).is_err());
    }
    #[test]
    fn throttled_denial_counts_persist_with_next_durable_watermark() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let owner = owner(&mut a, 100);
        for _ in 0..3 {
            a.record_denial(None, "login", None, 100).unwrap();
        }
        assert_eq!(a.audit_dropped(), 2);
        a.authenticate_session(&owner.session_token, 101).unwrap();
        drop(a);
        let a = Access::open(&dir.0, config(), 101).unwrap();
        assert_eq!(a.audit_dropped(), 2);
    }
    #[test]
    fn owner_recovery_rolls_forward_at_every_durable_boundary() {
        // Origin: adversarial review found a permanently mismatched epoch after access.json
        // publication. Contract: interrupted owner recovery must never resurrect old authority.
        for cut in 0..=4 {
            let dir = Scratch::new();
            let mut a = Access::open(&dir.0, config(), 100).unwrap();
            let session = owner(&mut a, 100);
            let secret = random().unwrap();
            let mut next = a.state.clone();
            next.installation_epoch = random().unwrap();
            next.policy += 1;
            next.credentials.clear();
            next.invitations.clear();
            next.principals
                .get_mut(&session.principal_id)
                .unwrap()
                .disabled = true;
            next.principals
                .get_mut(&session.principal_id)
                .unwrap()
                .epoch += 1;
            next.bootstrap_hash = Some(hash(&secret));
            next.bootstrap_expires = 700;
            next.recovery_principal = Some(session.principal_id.clone());
            let intent = RecoveryIntent {
                version: 1,
                old_epoch: a.state.installation_epoch.clone(),
                old_state_hash: hex_bytes(&Sha256::digest(
                    fs::read(dir.0.join("access.json")).unwrap(),
                )),
                next,
                secret: secret.clone(),
            };
            write_new(
                &dir.0.join("access-recovery.intent"),
                &serde_json::to_vec(&intent).unwrap(),
            )
            .unwrap();
            File::open(&dir.0).unwrap().sync_all().unwrap();
            let e = Access::apply_recovery(&dir.0, &intent, |step| {
                if step == cut {
                    Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "recovery boundary cut",
                    ))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::Interrupted);
            drop(a);
            let mut reopened = Access::open(&dir.0, config(), 100).unwrap();
            assert!(!dir.0.join("access-recovery.intent").exists());
            assert!(
                reopened
                    .authenticate_session(&session.session_token, 100)
                    .is_err()
            );
            assert_eq!(
                reopened
                    .start_registration(&secret, "owner", 100)
                    .unwrap()
                    .principal_id,
                session.principal_id
            );
            assert!(reopened.state.audit.len() <= AUDIT_CAP);
        }
    }
    #[test]
    fn workload_expiry_is_rechecked_after_body_wait_before_mutation() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let session = owner(&mut a, 100);
        let human = a.authenticate_session(&session.session_token, 100).unwrap();
        let mut scope = read_scope();
        scope.actions.insert(Action::NodePause);
        let credential = a
            .issue_workload(
                &human,
                "short-lived",
                scope,
                1,
                &session.csrf_token,
                "https://fabric.example",
                100,
            )
            .unwrap();
        let auth = a.authenticate_workload(&credential.token, 100).unwrap();
        assert!(a.validate_mutation(&auth, "", "", 101, false).is_err());
        assert!(
            a.begin_control(
                &auth,
                Action::NodePause,
                Some("enrollment-a"),
                "enrollment-a",
                101
            )
            .is_err()
        );
    }
    #[test]
    fn missing_and_disabled_accounts_get_bounded_non_authorizing_ceremonies() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let start = a.start_login("missing-principal", 100).unwrap();
        assert_eq!(
            start.public_key["publicKey"]["userVerification"],
            "required"
        );
        assert!(
            !start.public_key["publicKey"]["allowCredentials"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let response:PublicKeyCredential=serde_json::from_value(serde_json::json!({"id":"AA","rawId":"AA","type":"public-key","response":{"authenticatorData":"AA","clientDataJSON":"AA","signature":"AA","userHandle":null}})).unwrap();
        assert!(a.finish_login(&start.ceremony_id, &response, 100).is_err());
        assert!(!a.ceremonies.contains_key(&start.ceremony_id));
    }
    #[test]
    fn invitations_are_one_use_bound_to_server_created_identity_and_scope() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let session = owner(&mut a, 100);
        let auth = a.authenticate_session(&session.session_token, 100).unwrap();
        let invite = a
            .invite_human(
                &auth,
                "reader",
                read_scope(),
                &session.csrf_token,
                "https://fabric.example",
                100,
            )
            .unwrap();
        assert_eq!(a.state.principals[&invite.principal_id].scope, read_scope());
        assert!(a.state.principals[&invite.principal_id].disabled);
        let start = a
            .start_invited_registration(&invite.invitation, 100)
            .unwrap();
        assert_eq!(start.principal_id, invite.principal_id);
        assert!(
            a.start_invited_registration(&invite.invitation, 100)
                .is_err()
        );
        assert!(a.state.principals[&invite.principal_id].passkeys.is_empty());
    }
    #[test]
    fn scope_contains_actions_nodes_signals_and_query_bounds() {
        let s = read_scope();
        assert!(s.allows(
            Action::TelemetryRead,
            Some("enrollment-a"),
            Some(Signal::Logs)
        ));
        assert!(!s.allows(
            Action::TelemetryRead,
            Some("enrollment-b"),
            Some(Signal::Logs)
        ));
        assert!(!s.allows(
            Action::TelemetryRead,
            Some("enrollment-a"),
            Some(Signal::Metrics)
        ));
        assert!(!s.allows(Action::NodeConfigure, Some("enrollment-a"), None));
        let mut wider = s.clone();
        wider.installation_wide = true;
        assert!(!s.contains(&wider));
        wider = s.clone();
        wider.max_query_rows += 1;
        assert!(!s.contains(&wider));
    }
    #[test]
    fn bootstrap_secret_is_protected_one_use_challenges_are_server_side() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let secret = fs::read_to_string(dir.0.join("access-bootstrap.secret")).unwrap();
        assert_eq!(
            fs::metadata(dir.0.join("access-bootstrap.secret"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(a.start_registration("wrong", "owner", 100).is_err());
        let start = a.start_registration(&secret, "owner", 100).unwrap();
        assert!(start.public_key.get("publicKey").is_some());
        assert!(
            !serde_json::to_string(&a.state)
                .unwrap()
                .contains(&start.ceremony_id)
        );
        let response:RegisterPublicKeyCredential=serde_json::from_value(serde_json::json!({"id":"AA","rawId":"AA","type":"public-key","response":{"attestationObject":"AA","clientDataJSON":"AA"}})).unwrap();
        assert!(
            a.finish_registration(&start.ceremony_id, &response, 100)
                .is_err()
        );
        assert!(!a.ceremonies.contains_key(&start.ceremony_id));
        assert!(
            a.finish_registration(&start.ceremony_id, &response, 100)
                .is_err()
        );
        assert!(a.start_registration(&secret, "owner", 700).is_err());
    }
    #[test]
    fn sessions_enforce_csrf_origin_freshness_logout_and_restart() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let issued = owner(&mut a, 100);
        assert!(
            serde_json::to_value(&issued)
                .unwrap()
                .get("session_token")
                .is_none()
        );
        let auth = a.authenticate_session(&issued.session_token, 100).unwrap();
        assert!(
            a.validate_mutation(&auth, "wrong", "https://fabric.example", 100, true)
                .is_err()
        );
        assert!(
            a.validate_mutation(&auth, &issued.csrf_token, "https://evil.example", 100, true)
                .is_err()
        );
        assert!(
            a.validate_mutation(
                &auth,
                &issued.csrf_token,
                "https://fabric.example",
                100,
                true
            )
            .is_ok()
        );
        assert_eq!(
            a.session_view(&auth).unwrap()["csrf_token"],
            issued.csrf_token
        );
        assert!(
            a.validate_mutation(
                &auth,
                &issued.csrf_token,
                "https://fabric.example",
                400,
                true
            )
            .is_err()
        );
        a.logout(&auth, &issued.csrf_token, "https://fabric.example", 400)
            .unwrap();
        assert!(a.recheck(&auth).is_err());
        assert!(a.authenticate_session(&issued.session_token, 400).is_err());
        let fresh = a.issue_session(&issued.principal_id, 400).unwrap();
        drop(a);
        let mut a = Access::open(&dir.0, config(), 400).unwrap();
        assert!(a.authenticate_session(&fresh.session_token, 400).is_err());
    }
    #[test]
    fn credential_additions_preserve_issuer_cookie_but_not_captured_authority() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let issuer = owner(&mut a, 100);
        let other_device = a.issue_session(&issuer.principal_id, 100).unwrap();
        let old = a.authenticate_session(&issuer.session_token, 100).unwrap();
        let workload = a
            .issue_workload(
                &old,
                "reader",
                read_scope(),
                3600,
                &issuer.csrf_token,
                "https://fabric.example",
                100,
            )
            .unwrap();
        assert!(a.recheck(&old).is_err());
        assert!(
            a.authenticate_session(&other_device.session_token, 100)
                .is_err()
        );
        let current = a.authenticate_session(&issuer.session_token, 100).unwrap();
        assert!(a.authenticate_workload(&workload.token, 100).is_ok());
        a.rotate_workload(
            &current,
            &workload.principal_id,
            3600,
            &issuer.csrf_token,
            "https://fabric.example",
            100,
        )
        .unwrap();
        assert!(a.recheck(&current).is_err());
        let current = a.authenticate_session(&issuer.session_token, 100).unwrap();
        a.invite_human(
            &current,
            "invited",
            read_scope(),
            &issuer.csrf_token,
            "https://fabric.example",
            100,
        )
        .unwrap();
        // A disabled, keyless invitation grants no live authority and does
        // not advance policy until enrollment changes the credential set.
        assert!(a.recheck(&current).is_ok());
        assert!(a.authenticate_session(&issuer.session_token, 100).is_ok());
    }
    #[test]
    fn concurrent_counter_snapshot_cannot_authorize_after_durable_counter_advance() {
        // Origin: maintained library authentication state captures counters at
        // start; trace start(A,N=1), start(B,N=1), finish(B,N=3), finish(A,N=2).
        // This synthetic persisted public credential tests snapshot rejection,
        // not valid signatures or browser/device interoperability.
        let key: Passkey = serde_json::from_value(serde_json::json!({"cred":{
            "cred_id":"AQ","cred":{"type_":"ES256","key":{"EC_EC2":{"curve":"SECP256R1","x":"AA","y":"AA"}}},
            "counter":1,"transports":null,"user_verified":true,"backup_eligible":false,"backup_state":false,
            "registration_policy":"required","extensions":{},"attestation":{"data":"None","metadata":"None"},"attestation_format":"none"
        }})).unwrap();
        let keys = vec![key];
        let snapshot = passkey_revision(&keys).unwrap();
        assert!(require_passkey_revision(&keys, &snapshot).is_ok());
        let mut newer = serde_json::to_value(&keys).unwrap();
        newer[0]["cred"]["counter"] = serde_json::json!(3);
        let newer: Vec<Passkey> = serde_json::from_value(newer).unwrap();
        assert!(require_passkey_revision(&newer, &snapshot).is_err());
        let mut synced = serde_json::to_value(&keys).unwrap();
        synced[0]["cred"]["counter"] = serde_json::json!(0);
        let synced: Vec<Passkey> = serde_json::from_value(synced).unwrap();
        let synced_snapshot = passkey_revision(&synced).unwrap();
        assert!(require_passkey_revision(&synced, &synced_snapshot).is_ok());
    }
    #[test]
    fn reauthentication_retires_only_previous_cookie_and_its_delegations() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let old = owner(&mut a, 100);
        let human = a.authenticate_session(&old.session_token, 100).unwrap();
        let workload = a
            .issue_workload(
                &human,
                "reader",
                read_scope(),
                3600,
                &old.csrf_token,
                "https://fabric.example",
                100,
            )
            .unwrap();
        let old = a.issue_session(&old.principal_id, 100).unwrap();
        let human = a.authenticate_session(&old.session_token, 100).unwrap();
        let delegated = a
            .delegate(
                &human,
                &workload.principal_id,
                read_scope(),
                300,
                &old.csrf_token,
                "https://fabric.example",
                100,
            )
            .unwrap();
        let other_device = a.issue_session(&old.principal_id, 100).unwrap();
        let replacement = a.issue_session(&old.principal_id, 100).unwrap();
        a.retire_previous_session(Some(&old.session_token), &replacement, 100)
            .unwrap();
        assert!(a.authenticate_session(&old.session_token, 100).is_err());
        assert!(a.authenticate_workload(&delegated.token, 100).is_err());
        assert!(
            a.authenticate_session(&replacement.session_token, 100)
                .is_ok()
        );
        assert!(
            a.authenticate_session(&other_device.session_token, 100)
                .is_ok()
        );
        a.retire_previous_session(Some(&"f".repeat(64)), &replacement, 100)
            .unwrap();
        assert!(a.state.audit.iter().any(|e| e.action == "session.replace"));
        let auth = a
            .authenticate_session(&replacement.session_token, 100)
            .unwrap();
        a.logout(
            &auth,
            &replacement.csrf_token,
            "https://fabric.example",
            100,
        )
        .unwrap();
        assert!(a.state.audit.iter().any(|e| e.action == "session.logout"));
        let reopened = Access::open(&dir.0, config(), 100).unwrap();
        assert!(
            reopened
                .state
                .audit
                .iter()
                .any(|e| e.action == "session.replace")
        );
        assert!(
            reopened
                .state
                .audit
                .iter()
                .any(|e| e.action == "session.logout")
        );
    }
    #[test]
    fn workload_scope_expiry_revocation_parent_policy_and_restart() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let session = owner(&mut a, 100);
        let auth = a.authenticate_session(&session.session_token, 100).unwrap();
        let cred = a
            .issue_workload(
                &auth,
                "reader",
                read_scope(),
                86400,
                &session.csrf_token,
                "https://fabric.example",
                100,
            )
            .unwrap();
        let auth = a.authenticate_workload(&cred.token, 100).unwrap();
        assert!(a.authorize(
            &auth,
            Action::TelemetryRead,
            Some("enrollment-a"),
            Some(Signal::Logs)
        ));
        assert!(!a.authorize(
            &auth,
            Action::TelemetryRead,
            Some("enrollment-b"),
            Some(Signal::Logs)
        ));
        drop(a);
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        assert!(a.authenticate_workload(&cred.token, 100).is_ok());
        let session = a.issue_session(&session.principal_id, 100).unwrap();
        let human = a.authenticate_session(&session.session_token, 100).unwrap();
        let delegated = a
            .delegate(
                &human,
                &cred.principal_id,
                read_scope(),
                900,
                &session.csrf_token,
                "https://fabric.example",
                100,
            )
            .unwrap();
        let delegated_auth = a.authenticate_workload(&delegated.token, 100).unwrap();
        assert_eq!(
            delegated_auth.on_behalf_of.as_deref(),
            Some(human.principal_id.as_str())
        );
        assert!(a.recheck_at(&delegated_auth, 1000).is_err());
        let human = a
            .authenticate_session(&session.session_token, 1000)
            .unwrap();
        // A fresh reauthentication is explicitly required for the revocation below.
        let fresh = a.issue_session(&human.principal_id, 1000).unwrap();
        let human = a.authenticate_session(&fresh.session_token, 1000).unwrap();
        a.revoke_credential(
            &human,
            &cred.credential_id,
            &fresh.csrf_token,
            "https://fabric.example",
            1000,
        )
        .unwrap();
        assert!(a.authenticate_workload(&cred.token, 1000).is_err());
        assert!(a.recheck(&auth).is_err());
    }
    #[test]
    fn rollback_and_interrupted_control_require_explicit_offline_reconciliation() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let session = owner(&mut a, 100);
        let auth = a.authenticate_session(&session.session_token, 110).unwrap();
        let _intent = a
            .begin_control(
                &auth,
                Action::NodePause,
                Some("enrollment-a"),
                "enrollment-a",
                110,
            )
            .unwrap();
        drop(a);
        assert!(Access::open(&dir.0, config(), 110).is_err());
        assert!(Access::recover_offline(&dir.0, config(), &session.principal_id, 110).is_err());
        let token = Access::recover_offline_reconciled(
            &dir.0,
            config(),
            &session.principal_id,
            &"a".repeat(64),
            110,
        )
        .unwrap();
        let mut a = Access::open(&dir.0, config(), 110).unwrap();
        assert!(a.start_registration(&token, "owner", 110).is_ok());
        assert!(a.start_registration(&token, "owner", 109).is_err());
        drop(a);
        assert!(Access::open(&dir.0, config(), 109).is_err());
    }
    #[test]
    fn state_corruption_missing_state_epoch_mismatch_and_audit_bound_fail_closed() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        for _ in 0..(AUDIT_CAP + 2) {
            Access::event(&mut a.state, 100, None, None, "test", None, "denied").unwrap();
        }
        assert_eq!(a.state.audit.len(), AUDIT_CAP);
        assert_eq!(a.audit_dropped(), 2);
        a.publish(a.state.clone()).unwrap();
        drop(a);
        let original = fs::read(dir.0.join("access.json")).unwrap();
        fs::write(dir.0.join("access.json"), b"{}").unwrap();
        assert!(Access::open(&dir.0, config(), 100).is_err());
        fs::write(dir.0.join("access.json"), original).unwrap();
        fs::write(dir.0.join("access.epoch"), "different").unwrap();
        assert!(Access::open(&dir.0, config(), 100).is_err());
        fs::remove_file(dir.0.join("access.json")).unwrap();
        assert!(Access::open(&dir.0, config(), 100).is_err());
    }
}
