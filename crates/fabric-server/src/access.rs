//! Local passkey authentication and the three fixed roles (ADR-0029).
//!
//! The owner (passkey session) and reader (bearer token) roles live here; Spindle
//! tokens stay in `control`. Callers hold their access mutex through authorization
//! and control publication. Sessions and one-use ceremony state never leave this
//! server except opaque IDs.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use subtle::ConstantTimeEq;
use webauthn_rs::fake::{FakePasskeyDistribution, WebauthnFakeCredentialGenerator};
use webauthn_rs::prelude::*;

const FORMAT: u32 = 2;
const STATE_CAP: usize = 16 * 1024 * 1024;
const READER_CAP: usize = 256;
const READER_TTL_MAX: u64 = 2592000;
const PASSKEY_CAP: usize = 8;
const SESSION_CAP: usize = 4096;
const CEREMONY_CAP: usize = 1024;
const AUDIT_CAP: usize = 4096;
const DENIAL_CAP: usize = 256;
const DENIED: &str = "denied";
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
/// Console roles, ordered by authority. A Spindle token is not a console credential.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Reader,
    Owner,
}
/// The single human operator. An owner without passkeys is awaiting recovery.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    id: String,
    name: String,
    epoch: u64,
    passkeys: Vec<Passkey>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reader {
    id: String,
    name: String,
    hash: String,
    audience: String,
    issued: u64,
    expires: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditEvent {
    pub time: u64,
    pub request_id: String,
    pub actor: Option<String>,
    pub action: String,
    pub resource: Option<String>,
    pub outcome: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    config: AccessConfig,
    installation_epoch: String,
    fake_credential_key: String,
    last_time: u64,
    bootstrap_hash: Option<String>,
    bootstrap_expires: u64,
    owner: Option<Owner>,
    readers: BTreeMap<String, Reader>,
    audit: Vec<AuditEvent>,
    audit_dropped: u64,
    audit_next_id: u64,
    pending_control: Option<ControlIntent>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlIntent {
    id: String,
    action: String,
    resource: String,
    actor: String,
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
    epoch: u64,
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
/// A verified credential. `handle` is the session hash (owner) or credential ID
/// (reader); `principal_id` identifies cursors and audit actors.
#[derive(Clone, Debug)]
pub struct Authority {
    pub principal_id: String,
    pub role: Role,
    handle: String,
    epoch: u64,
}
#[derive(Serialize)]
pub struct ReaderIssued {
    pub credential_id: String,
    pub token: String,
    pub expires_unix_s: u64,
}
#[derive(Serialize)]
pub struct ReaderView {
    pub id: String,
    pub name: String,
    pub issued_unix_s: u64,
    pub expires_unix_s: u64,
}
#[derive(Serialize)]
pub struct SessionView {
    pub id: String,
    pub issued_unix_s: u64,
    pub last_seen_unix_s: u64,
    pub expires_unix_s: u64,
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
                #[derive(Deserialize)]
                struct Format {
                    version: u32,
                }
                let format: Format = serde_json::from_slice(&bytes)
                    .map_err(|_| invalid("malformed access state"))?;
                if format.version != FORMAT {
                    return Err(invalid(
                        "access state predates the three-role format; stop the service, remove state_dir/access and re-bootstrap the owner",
                    ));
                }
                let s: Stored = serde_json::from_slice(&bytes)
                    .map_err(|_| invalid("malformed access state"))?;
                let epoch = fs::read_to_string(dir.join("access.epoch"))?;
                if s.config != config
                    || !equal(&s.installation_epoch, &epoch)
                    || s.fake_credential_key.len() != 64
                    || s.readers.len() > READER_CAP
                    || s.audit.len() > AUDIT_CAP + DENIAL_CAP
                    || (s.pending_control.is_some() && !offline_reconcile)
                    || s.last_time > now
                {
                    return Err(invalid(
                        "access state configuration/epoch/bounds/time mismatch",
                    ));
                }
                if s.owner.as_ref().is_some_and(|o| {
                    Uuid::parse_str(&o.id).is_err()
                        || o.name.len() > 128
                        || o.passkeys.len() > PASSKEY_CAP
                }) {
                    return Err(invalid("invalid owner"));
                }
                for (id, c) in &s.readers {
                    if id != &c.id
                        || c.name.len() > 128
                        || c.hash.len() != 64
                        || c.audience != format!("{}#{}", config.audience, s.installation_epoch)
                        || c.expires <= c.issued
                        || c.expires - c.issued > READER_TTL_MAX
                    {
                        return Err(invalid("invalid reader token"));
                    }
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
                    version: FORMAT,
                    config,
                    installation_epoch: epoch,
                    fake_credential_key: random()?,
                    last_time: now,
                    bootstrap_hash: Some(hash(&secret)),
                    bootstrap_expires: now.saturating_add(600),
                    owner: None,
                    readers: BTreeMap::new(),
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
    pub fn origin(&self) -> &str {
        &self.state.config.origin
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
        action: &str,
        resource: Option<&str>,
        outcome: &str,
    ) -> io::Result<()> {
        let event_id = next
            .audit_next_id
            .checked_add(1)
            .ok_or_else(|| invalid("audit event sequence exhausted"))?;
        next.audit_next_id = event_id;
        // Denials are capped apart from every other event, so a flood of them can never evict
        // a login, credential or control record.
        let denial = outcome == DENIED;
        let same = |e: &AuditEvent| (e.outcome == DENIED) == denial;
        let cap = if denial { DENIAL_CAP } else { AUDIT_CAP };
        next.audit.retain(|e| now.saturating_sub(e.time) <= 2592000);
        if next.audit.iter().filter(|e| same(e)).count() >= cap
            && let Some(oldest) = next.audit.iter().position(same)
        {
            next.audit.remove(oldest);
            next.audit_dropped = next.audit_dropped.saturating_add(1);
        }
        next.audit.push(AuditEvent {
            time: now,
            request_id: format!("{}-{event_id}", next.installation_epoch),
            actor: actor.map(str::to_owned),
            action: action.to_owned(),
            resource: resource.map(str::to_owned),
            outcome: outcome.to_owned(),
        });
        Ok(())
    }
    /// The owner record for a live owner authority; every owner-only operation starts here.
    fn owner(&self, auth: &Authority) -> io::Result<&Owner> {
        self.recheck(auth)?;
        self.state
            .owner
            .as_ref()
            .filter(|o| auth.role == Role::Owner && o.id == auth.principal_id)
            .ok_or_else(denied)
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
            || self
                .state
                .owner
                .as_ref()
                .is_some_and(|o| !o.passkeys.is_empty())
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
            .owner
            .as_ref()
            .map_or_else(identifier, |o| Ok(o.id.clone()))?;
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
        let mut next = self.state.clone();
        if next
            .owner
            .iter()
            .flat_map(|o| &o.passkeys)
            .any(|key| key.cred_id() == passkey.cred_id())
        {
            return Err(denied());
        }
        if bootstrap {
            if next.bootstrap_hash.is_none() || now >= next.bootstrap_expires {
                return Err(denied());
            }
            match &mut next.owner {
                Some(o) if o.id == principal && o.passkeys.is_empty() => o.passkeys = vec![passkey],
                None => {
                    next.owner = Some(Owner {
                        id: principal.clone(),
                        name,
                        epoch: 1,
                        passkeys: vec![passkey],
                    })
                }
                Some(_) => return Err(denied()),
            }
            next.bootstrap_hash = None;
        } else {
            let o = next
                .owner
                .as_mut()
                .filter(|o| o.id == principal && o.epoch == epoch && o.passkeys.len() < PASSKEY_CAP)
                .ok_or_else(denied)?;
            o.passkeys.push(passkey);
            o.epoch = o.epoch.checked_add(1).ok_or_else(denied)?;
        }
        Self::event(
            &mut next,
            now,
            Some(&principal),
            "passkey.register",
            Some(&principal),
            "success",
        )?;
        self.publish(next)?;
        self.sessions.clear();
        if bootstrap {
            let _ = fs::remove_file(self.dir.join("access-bootstrap.secret"));
        }
        self.issue_session(now)
    }
    pub fn start_add_passkey(
        &mut self,
        auth: &Authority,
        csrf: &str,
        origin: &str,
        now: u64,
    ) -> io::Result<CeremonyStart> {
        self.validate_mutation(auth, csrf, origin, now, true)?;
        let o = self.owner(auth)?;
        if o.passkeys.len() >= PASSKEY_CAP || self.ceremonies.len() >= CEREMONY_CAP {
            return Err(denied());
        }
        self.registration(
            o.id.clone(),
            &o.name.clone(),
            false,
            o.epoch,
            Some(o.passkeys.iter().map(|k| k.cred_id().clone()).collect()),
            now,
        )
    }
    pub fn start_login(&mut self, principal: &str, now: u64) -> io::Result<CeremonyStart> {
        self.clock(now)?;
        if self.ceremonies.len() >= CEREMONY_CAP || principal.len() > 128 {
            return Err(denied());
        }
        // Generate the same library challenge shape for missing accounts and a
        // recovering owner. Its empty server-side credential list cannot verify
        // a signature or grant authority.
        let known = self.state.owner.as_ref().filter(|o| o.id == principal);
        let keys = known.map_or_else(Vec::new, |o| o.passkeys.clone());
        let epoch = known.map_or(0, |o| o.epoch);
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
        let o = next
            .owner
            .as_mut()
            .filter(|o| o.id == principal && o.epoch == epoch)
            .ok_or_else(denied)?;
        // The library verifies counters against its start-time credential
        // snapshot. A concurrent successful assertion must not let this older
        // snapshot authorize a counter below the latest durable credential.
        require_passkey_revision(&o.passkeys, &credential_state)?;
        let key = o
            .passkeys
            .iter_mut()
            .find(|k| k.cred_id() == result.cred_id())
            .ok_or_else(denied)?;
        key.update_credential(&result);
        Self::event(&mut next, now, Some(&principal), "login", None, "success")?;
        self.publish(next)?;
        self.issue_session(now)
    }
    fn issue_session(&mut self, now: u64) -> io::Result<SessionIssued> {
        if self.sessions.len() >= SESSION_CAP {
            return Err(denied());
        }
        let token = random()?;
        let csrf = random()?;
        let o = self.state.owner.as_ref().ok_or_else(denied)?;
        let principal_id = o.id.clone();
        self.sessions.insert(
            hash(&token),
            Session {
                epoch: o.epoch,
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
            principal_id,
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
        let o = self
            .state
            .owner
            .as_ref()
            .filter(|o| o.epoch == s.epoch)
            .ok_or_else(denied)?;
        s.seen = now;
        Ok(Authority {
            principal_id: o.id.clone(),
            role: Role::Owner,
            handle: digest,
            epoch: o.epoch,
        })
    }
    pub fn authenticate_reader(&mut self, token: &str, now: u64) -> io::Result<Authority> {
        self.clock(now)?;
        if token.len() != 64 {
            return Err(denied());
        }
        let digest = hash(token);
        let c = self
            .state
            .readers
            .values()
            .find(|c| equal(&c.hash, &digest))
            .ok_or_else(denied)?;
        if now < c.issued || now >= c.expires || c.audience != self.audience() {
            return Err(denied());
        }
        Ok(Authority {
            principal_id: c.id.clone(),
            role: Role::Reader,
            handle: c.id.clone(),
            epoch: 0,
        })
    }
    pub fn recheck(&self, auth: &Authority) -> io::Result<()> {
        let live = !self.poisoned
            && match auth.role {
                Role::Owner => {
                    self.state
                        .owner
                        .as_ref()
                        .is_some_and(|o| o.id == auth.principal_id && o.epoch == auth.epoch)
                        && self.sessions.contains_key(&auth.handle)
                }
                Role::Reader => self.state.readers.contains_key(&auth.handle),
            };
        if live { Ok(()) } else { Err(denied()) }
    }
    /// Revalidate lifetime as well as epoch immediately before releasing a long-running read.
    pub fn recheck_at(&mut self, auth: &Authority, now: u64) -> io::Result<()> {
        self.clock(now)?;
        self.recheck(auth)?;
        match self.state.readers.get(&auth.handle) {
            Some(c) if auth.role == Role::Reader && (now < c.issued || now >= c.expires) => {
                Err(denied())
            }
            _ => Ok(()),
        }
    }
    /// Cookie mutations need the exact Origin and session CSRF token. `fresh`
    /// adds the five-minute passkey recheck and so admits only an owner session.
    pub fn validate_mutation(
        &mut self,
        auth: &Authority,
        csrf: &str,
        origin: &str,
        now: u64,
        fresh: bool,
    ) -> io::Result<()> {
        self.recheck_at(auth, now)?;
        if auth.role == Role::Owner {
            let s = self.sessions.get(&auth.handle).ok_or_else(denied)?;
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
        if auth.role == Role::Owner {
            let mut next = self.state.clone();
            Self::event(
                &mut next,
                now,
                Some(&auth.principal_id),
                "session.logout",
                Some(&auth.principal_id),
                "success",
            )?;
            self.publish(next)?;
            self.sessions.remove(&auth.handle);
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
        let Some(previous) = previous_token.filter(|t| t.len() == 64) else {
            return Ok(());
        };
        let previous_id = hash(previous);
        if previous_id == replacement.handle || !self.sessions.contains_key(&previous_id) {
            return Ok(());
        }
        let mut next = self.state.clone();
        Self::event(
            &mut next,
            now,
            Some(&replacement.principal_id),
            "session.replace",
            Some(&replacement.principal_id),
            "success",
        )?;
        self.publish(next)?;
        self.sessions.remove(&previous_id);
        Ok(())
    }
    pub fn audit(&self, auth: &Authority) -> io::Result<&[AuditEvent]> {
        self.owner(auth)?;
        Ok(&self.state.audit)
    }
    pub fn issue_reader(
        &mut self,
        auth: &Authority,
        name: &str,
        ttl: u64,
        csrf: &str,
        origin: &str,
        now: u64,
    ) -> io::Result<ReaderIssued> {
        self.validate_mutation(auth, csrf, origin, now, true)?;
        if name.is_empty() || name.len() > 128 || ttl == 0 || ttl > READER_TTL_MAX {
            return Err(denied());
        }
        let mut next = self.state.clone();
        next.readers.retain(|_, c| c.expires > now);
        if next.readers.len() >= READER_CAP {
            return Err(denied());
        }
        let id = identifier()?;
        let token = random()?;
        let expires = now.saturating_add(ttl);
        next.readers.insert(
            id.clone(),
            Reader {
                id: id.clone(),
                name: name.to_owned(),
                hash: hash(&token),
                audience: format!("{}#{}", next.config.audience, next.installation_epoch),
                issued: now,
                expires,
            },
        );
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            "reader.issue",
            Some(&id),
            "success",
        )?;
        self.publish(next)?;
        Ok(ReaderIssued {
            credential_id: id,
            token,
            expires_unix_s: expires,
        })
    }
    pub fn readers(&self, auth: &Authority) -> io::Result<Vec<ReaderView>> {
        self.owner(auth)?;
        Ok(self
            .state
            .readers
            .values()
            .map(|c| ReaderView {
                id: c.id.clone(),
                name: c.name.clone(),
                issued_unix_s: c.issued,
                expires_unix_s: c.expires,
            })
            .collect())
    }
    pub fn revoke_reader(
        &mut self,
        auth: &Authority,
        id: &str,
        csrf: &str,
        origin: &str,
        now: u64,
    ) -> io::Result<()> {
        self.validate_mutation(auth, csrf, origin, now, true)?;
        let mut next = self.state.clone();
        next.readers.remove(id).ok_or_else(denied)?;
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            "reader.revoke",
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
        action: &str,
        resource: &str,
        now: u64,
    ) -> io::Result<String> {
        self.recheck_at(auth, now)?;
        self.owner(auth)?;
        if resource.len() > 128 || self.state.pending_control.is_some() {
            return Err(denied());
        }
        let id = random()?;
        let mut next = self.state.clone();
        next.pending_control = Some(ControlIntent {
            id: id.clone(),
            action: action.to_owned(),
            resource: resource.to_owned(),
            actor: auth.principal_id.clone(),
        });
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            action,
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
        {
            return Err(denied());
        }
        let mut next = self.state.clone();
        next.pending_control = None;
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            &intent.action,
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
    /// Rotates the installation epoch, invalidates reader tokens, and permits one verified
    /// passkey replacement for the existing owner ID. It cannot repair malformed state.
    pub fn recover_offline(
        dir: &Path,
        config: AccessConfig,
        principal: &str,
        now: u64,
    ) -> io::Result<String> {
        let access = Self::open(dir, config, now)?;
        let mut next = access.state.clone();
        let o = next
            .owner
            .as_mut()
            .filter(|o| o.id == principal)
            .ok_or_else(denied)?;
        o.epoch = o.epoch.checked_add(1).ok_or_else(denied)?;
        o.passkeys.clear();
        let token = random()?;
        next.readers.clear();
        next.last_time = now;
        next.bootstrap_hash = Some(hash(&token));
        next.bootstrap_expires = now.saturating_add(600);
        next.installation_epoch = random()?;
        Self::event(
            &mut next,
            now,
            Some(principal),
            "owner.recovery",
            Some(principal),
            "one_use_enrollment",
        )?;
        Self::write_recovery(dir, &access, next, token)
    }
    /// Stage the next state beside the old one, then roll the three files forward.
    fn write_recovery(
        dir: &Path,
        access: &Self,
        next: Stored,
        secret: String,
    ) -> io::Result<String> {
        let intent = RecoveryIntent {
            version: FORMAT,
            old_epoch: access.state.installation_epoch.clone(),
            old_state_hash: hex_bytes(&Sha256::digest(fs::read(dir.join("access.json"))?)),
            next,
            secret: secret.clone(),
        };
        let bytes = serde_json::to_vec(&intent).map_err(|_| invalid("recovery serialization"))?;
        if bytes.len() > STATE_CAP + 4096 {
            return Err(invalid("recovery intent cap"));
        }
        write_new(&dir.join("access-recovery.intent"), &bytes)?;
        File::open(dir)?.sync_all()?;
        Self::apply_recovery(dir, &intent, |_| Ok(()))?;
        Ok(secret)
    }
    /// Returned session IDs are random revocation handles, never reusable authentication tokens.
    pub fn sessions(&self, auth: &Authority) -> io::Result<Vec<SessionView>> {
        self.owner(auth)?;
        Ok(self
            .sessions
            .iter()
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
        if !self.sessions.contains_key(id) {
            return Err(denied());
        }
        let mut next = self.state.clone();
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            "session.revoke",
            Some(&auth.principal_id),
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
        let o = next.owner.as_mut().ok_or_else(denied)?;
        if o.passkeys.len() <= 1 {
            return Err(denied());
        }
        let before = o.passkeys.len();
        o.passkeys.retain(|k| {
            serde_json::to_value(k.cred_id())
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .as_deref()
                != Some(credential_id)
        });
        if before == o.passkeys.len() {
            return Err(denied());
        }
        o.epoch = o.epoch.checked_add(1).ok_or_else(denied)?;
        Self::event(
            &mut next,
            now,
            Some(&auth.principal_id),
            "passkey.revoke",
            Some(&auth.principal_id),
            "success",
        )?;
        self.publish(next)?;
        self.sessions.clear();
        Ok(())
    }
    pub fn passkey_ids(&self, auth: &Authority) -> io::Result<Vec<serde_json::Value>> {
        self.owner(auth)?
            .passkeys
            .iter()
            .map(|k| {
                serde_json::to_value(k.cred_id()).map_err(|_| invalid("credential serialization"))
            })
            .collect()
    }

    pub fn session_view(&self, auth: &Authority) -> io::Result<serde_json::Value> {
        let o = self.owner(auth)?;
        let s = self.sessions.get(&auth.handle).ok_or_else(denied)?;
        Ok(serde_json::json!({
            "api_version": 1, "principal_id": o.id, "display_name": o.name, "role": auth.role,
            "csrf": s.csrf_token, "expires_unix_s": s.issued.saturating_add(SESSION_TTL),
            "fresh_until_unix_s": s.verified.saturating_add(FRESH_TTL)}))
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
    /// Call from the request boundary using fixed actions and immutable resource IDs.
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
            action,
            resource,
            DENIED,
        )?;
        self.publish(next)
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
        if intent.version != FORMAT
            || intent.next.version != FORMAT
            || intent.next.config != *config
            || intent.old_epoch.len() != 64
            || intent.next.installation_epoch.len() != 64
            || intent.next.installation_epoch == intent.old_epoch
            || intent.secret.len() != 64
            || intent.next.last_time > now
            || !intent.next.readers.is_empty()
            || intent.next.bootstrap_hash.as_deref() != Some(hash(&intent.secret).as_str())
            || intent
                .next
                .owner
                .as_ref()
                .is_some_and(|o| !o.passkeys.is_empty())
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

    /// Renew only an unfinished first-owner setup while the stopped-service lock is held.
    /// An existing owner makes this operation unavailable.
    pub fn renew_bootstrap_offline(
        dir: &Path,
        config: AccessConfig,
        now: u64,
    ) -> io::Result<String> {
        let access = Self::open(dir, config, now)?;
        if access.state.owner.is_some()
            || !access.state.readers.is_empty()
            || access.state.bootstrap_hash.is_none()
        {
            return Err(denied());
        }
        let token = random()?;
        let mut next = access.state.clone();
        next.installation_epoch = random()?;
        next.bootstrap_hash = Some(hash(&token));
        next.bootstrap_expires = now.saturating_add(600);
        next.last_time = now;
        Self::event(
            &mut next,
            now,
            None,
            "bootstrap.offline_renew",
            None,
            "success",
        )?;
        Self::write_recovery(dir, &access, next, token)
    }
    /// HTTP policy test fixture only. It establishes a synthetic owner session;
    /// it provides no evidence about signatures, user verification or browser ceremonies.
    #[cfg(test)]
    pub(crate) fn fixture_session(&mut self, now: u64) -> io::Result<SessionIssued> {
        self.clock(now)?;
        let mut next = self.state.clone();
        next.bootstrap_hash = None;
        if next.owner.is_none() {
            next.owner = Some(Owner {
                id: identifier()?,
                name: "policy-fixture".into(),
                epoch: 1,
                passkeys: vec![],
            });
        }
        self.publish(next)?;
        self.issue_session(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ORIGIN: &str = "https://fabric.example";
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
            origin: ORIGIN.into(),
            rp_id: "fabric.example".into(),
            audience: "installation-1".into(),
        }
    }
    fn owner(access: &mut Access, now: u64) -> SessionIssued {
        access.fixture_session(now).unwrap()
    }
    fn reader(a: &mut Access, session: &SessionIssued, ttl: u64, now: u64) -> ReaderIssued {
        let auth = a.authenticate_session(&session.session_token, now).unwrap();
        a.issue_reader(&auth, "reader", ttl, &session.csrf_token, ORIGIN, now)
            .unwrap()
    }
    #[test]
    fn resident_registration_options_cover_bootstrap_add_and_recovery() {
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
            &a.start_add_passkey(&human, &session.csrf_token, ORIGIN, 100)
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
        let other = reader(&mut a, &owner, 100, 100);
        let other_auth = a.authenticate_reader(&other.token, 100).unwrap();
        assert!(
            a.begin_control(&other_auth, "node.pause", "enrollment-a", 100)
                .is_err()
        );
        let id = a
            .begin_control(&auth, "node.pause", "enrollment-a", 100)
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
    fn reader_tokens_are_hashed_expire_revoke_and_never_reach_owner_operations() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let session = owner(&mut a, 100);
        let human = a.authenticate_session(&session.session_token, 100).unwrap();
        let short = reader(&mut a, &session, 100, 100);
        let long = reader(&mut a, &session, 1000, 100);
        // Issuing readers changes nothing about the owner's live authority.
        assert!(a.recheck(&human).is_ok());
        assert!(
            !serde_json::to_string(&a.state)
                .unwrap()
                .contains(&short.token)
        );
        let auth = a.authenticate_reader(&short.token, 100).unwrap();
        assert_eq!(auth.role, Role::Reader);
        assert_eq!(a.readers(&human).unwrap().len(), 2);
        for refusal in [
            a.readers(&auth).err(),
            a.audit(&auth).err(),
            a.passkey_ids(&auth).err(),
            a.sessions(&auth).err(),
            a.session_view(&auth).err(),
            a.issue_reader(&auth, "x", 60, "", "", 100).err(),
            a.revoke_reader(&auth, &long.credential_id, "", "", 100)
                .err(),
            a.start_add_passkey(&auth, "", "", 100).err(),
            a.begin_control(&auth, "node.pause", "enrollment-a", 100)
                .err(),
        ] {
            assert!(refusal.is_some());
        }
        drop(a);
        let mut a = Access::open(&dir.0, config(), 150).unwrap();
        let long_auth = a.authenticate_reader(&long.token, 150).unwrap();
        assert!(a.authenticate_reader(&short.token, 150).is_ok());
        let session = a.fixture_session(150).unwrap();
        let human = a.authenticate_session(&session.session_token, 150).unwrap();
        let revoke = |a: &mut Access| {
            a.revoke_reader(
                &human,
                &long.credential_id,
                &session.csrf_token,
                ORIGIN,
                150,
            )
        };
        revoke(&mut a).unwrap();
        assert!(a.authenticate_reader(&long.token, 150).is_err());
        assert!(a.recheck(&long_auth).is_err());
        assert!(revoke(&mut a).is_err());
        // Expiry is rechecked after a body wait, before any mutation or release.
        assert!(a.authenticate_reader(&short.token, 200).is_err());
        assert!(a.recheck_at(&auth, 200).is_err());
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
    fn denial_floods_never_evict_security_or_mutation_events() {
        // Origin: anonymous 401/403 responses stored one denial per second in the shared
        // ring, so about 68 minutes of junk evicted every real login. The stored flood is
        // primed in memory to avoid 4,096 fsynced publications; the entry point adds more.
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        owner(&mut a, 100);
        let mut next = a.state.clone();
        Access::event(&mut next, 100, Some("owner"), "login", None, "success").unwrap();
        for now in 101..=100 + AUDIT_CAP as u64 {
            Access::event(&mut next, now, None, "console.request", None, DENIED).unwrap();
        }
        a.publish(next).unwrap();
        for now in 5000..5003 {
            a.record_denial(None, "console.request", None, now).unwrap();
        }
        let kept = a.state.audit.iter().filter(|e| e.outcome == DENIED).count();
        assert!(a.state.audit.iter().any(|e| e.action == "login"));
        assert_eq!(kept, DENIAL_CAP);
        assert_eq!(a.audit_dropped(), (AUDIT_CAP + 3 - DENIAL_CAP) as u64);
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
            next.readers.clear();
            let o = next.owner.as_mut().unwrap();
            o.epoch += 1;
            o.passkeys.clear();
            next.bootstrap_hash = Some(hash(&secret));
            next.bootstrap_expires = 700;
            let intent = RecoveryIntent {
                version: FORMAT,
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
    fn missing_accounts_get_bounded_non_authorizing_ceremonies() {
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
            a.validate_mutation(&auth, "wrong", ORIGIN, 100, true)
                .is_err()
        );
        assert!(
            a.validate_mutation(&auth, &issued.csrf_token, "https://evil.example", 100, true)
                .is_err()
        );
        assert!(
            a.validate_mutation(&auth, &issued.csrf_token, ORIGIN, 100, true)
                .is_ok()
        );
        let view = a.session_view(&auth).unwrap();
        assert_eq!(view["csrf"], issued.csrf_token);
        assert_eq!(view["role"], "owner");
        assert!(
            a.validate_mutation(&auth, &issued.csrf_token, ORIGIN, 400, true)
                .is_err()
        );
        a.logout(&auth, &issued.csrf_token, ORIGIN, 400).unwrap();
        assert!(a.recheck(&auth).is_err());
        assert!(a.authenticate_session(&issued.session_token, 400).is_err());
        let fresh = a.issue_session(400).unwrap();
        drop(a);
        let mut a = Access::open(&dir.0, config(), 400).unwrap();
        assert!(a.authenticate_session(&fresh.session_token, 400).is_err());
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
    fn reauthentication_retires_only_previous_cookie() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let old = owner(&mut a, 100);
        let other_device = a.issue_session(100).unwrap();
        let replacement = a.issue_session(100).unwrap();
        a.retire_previous_session(Some(&old.session_token), &replacement, 100)
            .unwrap();
        assert!(a.authenticate_session(&old.session_token, 100).is_err());
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
        a.logout(&auth, &replacement.csrf_token, ORIGIN, 100)
            .unwrap();
        assert!(a.state.audit.iter().any(|e| e.action == "session.logout"));
        let reopened = Access::open(&dir.0, config(), 100).unwrap();
        for action in ["session.replace", "session.logout"] {
            assert!(reopened.state.audit.iter().any(|e| e.action == action));
        }
    }
    #[test]
    fn rollback_and_interrupted_control_require_explicit_offline_reconciliation() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        let session = owner(&mut a, 100);
        let issued = reader(&mut a, &session, 3600, 100);
        let auth = a.authenticate_session(&session.session_token, 110).unwrap();
        let _intent = a
            .begin_control(&auth, "node.pause", "enrollment-a", 110)
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
        // Recovery rotates the installation epoch: reader tokens do not survive it.
        assert!(a.authenticate_reader(&issued.token, 110).is_err());
        assert!(a.start_registration(&token, "owner", 110).is_ok());
        assert!(a.start_registration(&token, "owner", 109).is_err());
        drop(a);
        assert!(Access::open(&dir.0, config(), 109).is_err());
    }
    #[test]
    fn state_corruption_older_format_missing_state_epoch_mismatch_and_audit_bound_fail_closed() {
        let dir = Scratch::new();
        let mut a = Access::open(&dir.0, config(), 100).unwrap();
        for _ in 0..(AUDIT_CAP + 2) {
            Access::event(&mut a.state, 100, None, "test", None, "success").unwrap();
        }
        assert_eq!(a.state.audit.len(), AUDIT_CAP);
        assert_eq!(a.audit_dropped(), 2);
        a.publish(a.state.clone()).unwrap();
        drop(a);
        let original = fs::read(dir.0.join("access.json")).unwrap();
        fs::write(dir.0.join("access.json"), b"{}").unwrap();
        assert!(Access::open(&dir.0, config(), 100).is_err());
        // Origin: format 1 (scoped principals) was written only by development trials.
        fs::write(
            dir.0.join("access.json"),
            br#"{"version":1,"principals":{}}"#,
        )
        .unwrap();
        let message = Access::open(&dir.0, config(), 100)
            .err()
            .unwrap()
            .to_string();
        assert!(message.contains("re-bootstrap"), "{message}");
        fs::write(dir.0.join("access.json"), original).unwrap();
        fs::write(dir.0.join("access.epoch"), "different").unwrap();
        assert!(Access::open(&dir.0, config(), 100).is_err());
        fs::remove_file(dir.0.join("access.json")).unwrap();
        assert!(Access::open(&dir.0, config(), 100).is_err());
    }
}
