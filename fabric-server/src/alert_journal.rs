//! Explicit local alert journal. Provisioning is caller authority, not authentication;
//! this module neither reads a source nor sends or confirms notifications.
use fabric_alert::{Checkpoint, Decision, Evaluation, Outcome, Reducer, Rule};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use rustix::fs::{FlockOperation, flock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    time::{Duration, Instant},
};

const MAX_EVENTS: u32 = 4096;
const MAX_ID: usize = 128;
const MAX_STATE: usize = 16384;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provision {
    pub version: u8,
    pub authority: String,
    pub scope: String,
    pub rule: Rule,
    pub capacity: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub event_id: String,
    pub intent_id: Option<String>,
    pub version: u64,
    pub decision: Decision,
}
/// Local pagination position, meaningful only for the explicitly opened store.
/// This is not an authenticated token and must not be accepted as one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PagePosition {
    pub through: u64,
    pub after: u64,
}
/// A stored evaluation and its immutable receipt in commit order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalEntry {
    pub request_id: String,
    pub expected_version: u64,
    pub input: Evaluation,
    pub receipt: Receipt,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalPage {
    pub through: u64,
    pub entries: Vec<JournalEntry>,
    pub next: Option<PagePosition>,
}
/// Maximum returned entries per local read.
pub const MAX_PAGE_ENTRIES: u32 = 64;
#[derive(Debug)]
pub enum JournalError {
    Invalid,
    ScopeMismatch,
    Conflict,
    Stale,
    Full,
    Corrupt,
    Storage,
}
fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ID
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}
fn encode<T: Serialize>(v: &T) -> Result<Vec<u8>, JournalError> {
    let bytes = serde_json::to_vec(v).map_err(|_| JournalError::Invalid)?;
    if bytes.len() > MAX_STATE {
        return Err(JournalError::Invalid);
    }
    Ok(bytes)
}
fn decode<T: for<'a> Deserialize<'a>>(bytes: &[u8]) -> Result<T, JournalError> {
    if bytes.len() > MAX_STATE {
        return Err(JournalError::Corrupt);
    }
    serde_json::from_slice(bytes).map_err(|_| JournalError::Corrupt)
}

fn write_error(error: rusqlite::Error) -> JournalError {
    if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull) {
        JournalError::Full
    } else {
        JournalError::Storage
    }
}

// The identifier depends only on immutable provision and committed version.
fn event_digest(provision: &Provision, version: i64) -> Result<String, JournalError> {
    let mut hash = Sha256::new();
    hash.update(b"fabric-alert-event-v1\0");
    hash.update(encode(provision)?);
    hash.update(version.to_be_bytes());
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn forbidden_state_path(path: &Path) -> bool {
    ["/tmp", "/dev/shm", "/run/shm"]
        .into_iter()
        .any(|prefix| path.starts_with(prefix))
}

fn require_disk(directory: &File) -> Result<(), JournalError> {
    let stat = rustix::fs::fstatfs(directory).map_err(|_| JournalError::Storage)?;
    if matches!(stat.f_type as u64, 0x0102_1994 | 0x8584_58f6) {
        return Err(JournalError::Invalid);
    }
    Ok(())
}

/// Local work state, not evidence of notification delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentState {
    LegacyUnmanaged,
    Queued,
    Leased,
    Uncertain,
}
/// Opaque, process-local capability; it is invalid after reopen or in another journal.
#[derive(Debug)]
pub struct IntentLease {
    intent_id: String,
    session: String,
    fence: i64,
}
const MAX_LEASE: Duration = Duration::from_secs(300);

pub struct AlertJournal {
    db: Connection,
    session: String,
    active: Option<(String, i64, Instant)>,
    work_cutover: i64,
    _owner: File,
    provision: Provision,
}
impl AlertJournal {
    /// Caller creates a private directory and supplies the immutable provision.
    pub fn open(dir: &Path, provision: Provision) -> Result<Self, JournalError> {
        if provision.version != 1
            || !valid_id(&provision.authority)
            || !valid_id(&provision.scope)
            || provision.capacity == 0
            || provision.capacity > MAX_EVENTS
        {
            return Err(JournalError::Invalid);
        }
        let initial = Reducer::new(provision.rule)
            .map_err(|_| JournalError::Invalid)?
            .checkpoint();
        if !dir.is_absolute() || forbidden_state_path(dir) {
            return Err(JournalError::Invalid);
        }
        let name = dir.file_name().ok_or(JournalError::Invalid)?;
        let parent = dir
            .parent()
            .ok_or(JournalError::Invalid)?
            .canonicalize()
            .map_err(|_| JournalError::Storage)?;
        let dir = parent.join(name);
        if forbidden_state_path(&dir) {
            return Err(JournalError::Invalid);
        }
        require_disk(&File::open(parent).map_err(|_| JournalError::Storage)?)?;
        crate::ledger::private_dir(&dir).map_err(|_| JournalError::Storage)?;
        let directory = File::from(
            rustix::fs::open(
                &dir,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::DIRECTORY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .map_err(|_| JournalError::Storage)?,
        );
        require_disk(&directory)?;
        let metadata = directory.metadata().map_err(|_| JournalError::Storage)?;
        if !metadata.is_dir()
            || metadata.uid() != rustix::process::getuid().as_raw()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(JournalError::Storage);
        }
        let owner = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(dir.join("alert-owner.lock"))
            .map_err(|_| JournalError::Storage)?;
        flock(&owner, FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| JournalError::Storage)?;
        let path = dir.join("alert.sqlite");
        if let Ok(m) = fs::symlink_metadata(&path)
            && (!m.is_file() || m.file_type().is_symlink())
        {
            return Err(JournalError::Storage);
        }
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(&path)
            .map_err(|_| JournalError::Storage)?;
        let metadata = file.metadata().map_err(|_| JournalError::Storage)?;
        if !metadata.is_file()
            || metadata.uid() != rustix::process::getuid().as_raw()
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(JournalError::Storage);
        }
        drop(file);
        let mut db = Connection::open(path).map_err(|_| JournalError::Storage)?;
        db.execute_batch("PRAGMA journal_mode=PERSIST; PRAGMA synchronous=EXTRA; PRAGMA temp_store=MEMORY; CREATE TABLE IF NOT EXISTS alert_state(id INTEGER PRIMARY KEY CHECK(id=1), provision BLOB NOT NULL, version INTEGER NOT NULL, checkpoint BLOB NOT NULL); CREATE TABLE IF NOT EXISTS alert_events(id TEXT PRIMARY KEY, expected INTEGER NOT NULL, input BLOB NOT NULL, receipt BLOB NOT NULL);").map_err(|_| JournalError::Storage)?;
        let page_size: u64 = db
            .pragma_query_value(None, "page_size", |r| r.get(0))
            .map_err(|_| JournalError::Storage)?;
        if page_size != 4096 {
            return Err(JournalError::Storage);
        }
        db.pragma_update(None, "max_page_count", 8192)
            .map_err(|_| JournalError::Storage)?;
        let pages: u64 = db
            .pragma_query_value(None, "max_page_count", |r| r.get(0))
            .map_err(|_| JournalError::Storage)?;
        if pages != 8192 {
            return Err(JournalError::Storage);
        }
        let bytes = encode(&provision)?;
        db.execute(
            "INSERT OR IGNORE INTO alert_state VALUES(1,?1,0,?2)",
            params![bytes, encode(&initial)?],
        )
        .map_err(|_| JournalError::Storage)?;
        let saved: Vec<u8> = db
            .query_row("SELECT provision FROM alert_state WHERE id=1", [], |r| {
                r.get(0)
            })
            .map_err(|_| JournalError::Storage)?;
        if saved != encode(&provision)? {
            return Err(JournalError::ScopeMismatch);
        }
        // Additive migration intentionally leaves old receipt-only intents unmanaged.
        // Recovery and schema marker commit together after provision and quota checks.
        let session = fabric::canonical::random_nonce_hex().map_err(|_| JournalError::Storage)?;
        let work_cutover;
        {
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(write_error)?;
            let schema: i64 = tx
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .map_err(|_| JournalError::Corrupt)?;
            let tables: i64 = tx.query_row("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name IN ('alert_work_meta','alert_intent_work')", [], |r| r.get(0)).map_err(|_| JournalError::Corrupt)?;
            match (schema, tables) {
                (0, 0) => {
                    tx.execute_batch("CREATE TABLE alert_work_meta(id INTEGER PRIMARY KEY CHECK(id=1), version INTEGER NOT NULL, cutover INTEGER NOT NULL); CREATE TABLE alert_intent_work(intent TEXT PRIMARY KEY, request_id TEXT NOT NULL UNIQUE, fence INTEGER NOT NULL, state TEXT NOT NULL);").map_err(write_error)?;
                    tx.execute("INSERT INTO alert_work_meta SELECT 1,1,version FROM alert_state WHERE id=1", []).map_err(write_error)?;
                    tx.pragma_update(None, "user_version", 1)
                        .map_err(write_error)?;
                }
                (1, 2) => {}
                _ => return Err(JournalError::Corrupt),
            }
            let (count, version, cutover): (i64, i64, i64) = tx
                .query_row(
                    "SELECT count(*),coalesce(max(version),0),coalesce(max(cutover),-1) FROM alert_work_meta",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .map_err(|_| JournalError::Corrupt)?;
            if count != 1 || version != 1 || cutover < 0 {
                return Err(JournalError::Corrupt);
            }
            work_cutover = cutover;
            let current: i64 = tx
                .query_row("SELECT version FROM alert_state WHERE id=1", [], |r| {
                    r.get(0)
                })
                .map_err(|_| JournalError::Corrupt)?;
            if current < cutover || current as u64 > u64::from(provision.capacity) {
                return Err(JournalError::Corrupt);
            }
            let oversized: i64 = tx
                .query_row(
                    "SELECT count(*) FROM alert_events WHERE length(receipt)>?1 OR length(id)>?2",
                    params![MAX_STATE as i64, MAX_ID as i64],
                    |r| r.get(0),
                )
                .map_err(|_| JournalError::Corrupt)?;
            let oversized_work: i64 = tx.query_row("SELECT count(*) FROM alert_intent_work WHERE length(intent)>?1 OR length(request_id)>?2 OR length(state)>16", params![MAX_ID as i64,MAX_ID as i64], |r| r.get(0)).map_err(|_| JournalError::Corrupt)?;
            if oversized != 0 || oversized_work != 0 {
                return Err(JournalError::Corrupt);
            }
            let mut history = tx.prepare("SELECT e.id,e.receipt,w.intent FROM alert_events e LEFT JOIN alert_intent_work w ON w.request_id=e.id WHERE e.expected>=?1").map_err(|_| JournalError::Corrupt)?;
            let mut historical_rows = history
                .query([cutover])
                .map_err(|_| JournalError::Corrupt)?;
            while let Some(row) = historical_rows.next().map_err(|_| JournalError::Corrupt)? {
                let receipt: Vec<u8> = row.get(1).map_err(|_| JournalError::Corrupt)?;
                let receipt: Receipt = decode(&receipt)?;
                let intent: Option<String> = row.get(2).map_err(|_| JournalError::Corrupt)?;
                if intent != receipt.intent_id {
                    return Err(JournalError::Corrupt);
                }
            }
            drop(historical_rows);
            drop(history);
            let mut stmt = tx.prepare("SELECT w.intent,w.request_id,w.fence,w.state,e.receipt FROM alert_intent_work w LEFT JOIN alert_events e ON e.id=w.request_id ORDER BY w.intent").map_err(|_| JournalError::Corrupt)?;
            let mut rows = stmt.query([]).map_err(|_| JournalError::Corrupt)?;
            let mut work_count = 0u32;
            while let Some(row) = rows.next().map_err(|_| JournalError::Corrupt)? {
                work_count = work_count.checked_add(1).ok_or(JournalError::Corrupt)?;
                if work_count > provision.capacity {
                    return Err(JournalError::Corrupt);
                }
                let intent: String = row.get(0).map_err(|_| JournalError::Corrupt)?;
                let id: String = row.get(1).map_err(|_| JournalError::Corrupt)?;
                let fence: i64 = row.get(2).map_err(|_| JournalError::Corrupt)?;
                let state: String = row.get(3).map_err(|_| JournalError::Corrupt)?;
                let receipt: Vec<u8> = row.get(4).map_err(|_| JournalError::Corrupt)?;
                let receipt: Receipt = decode(&receipt)?;
                if !valid_id(&id)
                    || receipt.intent_id.as_deref() != Some(intent.as_str())
                    || fence < 0
                    || !matches!(state.as_str(), "queued" | "leased" | "uncertain")
                    || (state == "queued" && fence != 0)
                    || (state != "queued" && fence == 0)
                {
                    return Err(JournalError::Corrupt);
                }
            }
            drop(rows);
            drop(stmt);
            tx.execute(
                "UPDATE alert_intent_work SET state='uncertain' WHERE state='leased'",
                [],
            )
            .map_err(write_error)?;
            tx.commit().map_err(write_error)?;
        }
        // Commit creates database, journal and lock names in this directory.
        // Surface a failed directory barrier instead of reporting a durable open.
        directory.sync_all().map_err(|_| JournalError::Storage)?;
        Ok(Self {
            db,
            session,
            active: None,
            work_cutover,
            _owner: owner,
            provision,
        })
    }
    /// Read an existing immutable local event and its optional intent by request ID.
    pub fn read(&self, request_id: &str) -> Result<Option<Receipt>, JournalError> {
        if !valid_id(request_id) {
            return Err(JournalError::Invalid);
        }
        let bytes: Option<Vec<u8>> = self
            .db
            .query_row(
                "SELECT receipt FROM alert_events WHERE id=?1",
                [request_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| JournalError::Storage)?;
        bytes.map(|value| decode(&value)).transpose()
    }
    /// Read a bounded immutable page in version order. `None` fixes the current
    /// version as the high-water mark; subsequent positions retain that mark.
    /// Positions belong to this explicitly opened store and provide no security
    /// boundary or cross-store identity. Authority and scope are caller claims.
    /// Local consistency checks do not re-evaluate or authenticate event history.
    pub fn read_page(
        &self,
        authority: &str,
        scope: &str,
        position: Option<PagePosition>,
        limit: u32,
    ) -> Result<JournalPage, JournalError> {
        if authority != self.provision.authority || scope != self.provision.scope {
            return Err(JournalError::ScopeMismatch);
        }
        if limit == 0 || limit > MAX_PAGE_ENTRIES {
            return Err(JournalError::Invalid);
        }
        let current: i64 = self
            .db
            .query_row("SELECT version FROM alert_state WHERE id=1", [], |r| {
                r.get(0)
            })
            .map_err(|_| JournalError::Storage)?;
        if current < 0 || current as u64 > u64::from(self.provision.capacity) {
            return Err(JournalError::Corrupt);
        }
        // There is no schema index or uniqueness constraint on `expected`.
        // Verify the bounded history globally so a duplicate at a page boundary
        // cannot hide a gap or be silently skipped by the next page.
        let (count, distinct, min, max): (i64, i64, Option<i64>, Option<i64>) = self.db
            .query_row("SELECT count(*),count(DISTINCT expected),min(expected),max(expected) FROM alert_events", [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(|_| JournalError::Storage)?;
        if count != current
            || distinct != count
            || (current == 0 && (min.is_some() || max.is_some()))
            || (current > 0 && (min != Some(0) || max != Some(current - 1)))
        {
            return Err(JournalError::Corrupt);
        }
        let (through, after) = match position {
            None => (current as u64, 0),
            Some(p) if p.after < p.through && p.through <= current as u64 => (p.through, p.after),
            Some(_) => return Err(JournalError::Invalid),
        };
        let expected_count = (through - after).min(u64::from(limit)) as usize;
        let mut stmt = self.db.prepare(
            "SELECT id,expected,input,receipt FROM alert_events WHERE expected>=?1 AND expected<?2 ORDER BY expected LIMIT ?3"
        ).map_err(|_| JournalError::Storage)?;
        let mut rows = stmt
            .query(params![after as i64, through as i64, limit])
            .map_err(|_| JournalError::Storage)?;
        let mut entries = Vec::with_capacity(expected_count);
        while let Some(row) = rows.next().map_err(|_| JournalError::Storage)? {
            let request_id: String = row.get(0).map_err(|_| JournalError::Corrupt)?;
            let expected: i64 = row.get(1).map_err(|_| JournalError::Corrupt)?;
            let input: Vec<u8> = row.get(2).map_err(|_| JournalError::Corrupt)?;
            let receipt: Vec<u8> = row.get(3).map_err(|_| JournalError::Corrupt)?;
            let receipt: Receipt = decode(&receipt)?;
            let input: Evaluation = decode(&input)?;
            let wanted = after + entries.len() as u64;
            let digest = event_digest(&self.provision, (wanted + 1) as i64)
                .map_err(|_| JournalError::Corrupt)?;
            let validity_matches = match input.sample {
                fabric_alert::Sample::Covered {
                    coverage_start,
                    coverage_end,
                    ..
                } => {
                    coverage_start <= input.time
                        && coverage_end == input.time
                        && receipt.decision.validity == fabric_alert::Validity::Covered
                }
                fabric_alert::Sample::Missing(validity) => {
                    validity != fabric_alert::Validity::Covered
                        && receipt.decision.validity == validity
                }
            };
            if !valid_id(&request_id)
                || expected < 0
                || expected as u64 != wanted
                || receipt.version != wanted + 1
                || input != receipt.decision.input
                || input.profile != self.provision.rule.profile
                || input.source != self.provision.rule.source
                || !validity_matches
                || receipt.event_id != format!("alert-v1:{digest}")
                || receipt
                    .intent_id
                    .as_ref()
                    .is_some_and(|id| id != &format!("intent-v1:{digest}"))
            {
                return Err(JournalError::Corrupt);
            }
            entries.push(JournalEntry {
                request_id,
                expected_version: wanted,
                input,
                receipt,
            });
        }
        if entries.len() != expected_count {
            return Err(JournalError::Corrupt);
        }
        let end = after + entries.len() as u64;
        let next = (end < through).then_some(PagePosition {
            through,
            after: end,
        });
        Ok(JournalPage {
            through,
            entries,
            next,
        })
    }
    /// Inspect an event's local work state. Historical receipt-only intents are
    /// explicitly unmanaged; a receipt without an intent returns None. This is
    /// persisted state: expiration is observed by claim or abandon, not this read.
    pub fn intent_state(&self, request_id: &str) -> Result<Option<IntentState>, JournalError> {
        let Some(receipt) = self.read(request_id)? else {
            return Ok(None);
        };
        let Some(intent) = receipt.intent_id else {
            return Ok(None);
        };
        let row: Option<(String, String, i64)> = self
            .db
            .query_row(
                "SELECT intent,state,fence FROM alert_intent_work WHERE request_id=?1",
                [request_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|_| JournalError::Storage)?;
        match row {
            None => {
                let expected: i64 = self
                    .db
                    .query_row(
                        "SELECT expected FROM alert_events WHERE id=?1",
                        [request_id],
                        |r| r.get(0),
                    )
                    .map_err(|_| JournalError::Corrupt)?;
                if expected < 0 || expected >= self.work_cutover {
                    return Err(JournalError::Corrupt);
                }
                Ok(Some(IntentState::LegacyUnmanaged))
            }
            Some((id, state, fence)) if id == intent && fence == 0 && state == "queued" => {
                Ok(Some(IntentState::Queued))
            }
            Some((id, state, fence)) if id == intent && fence > 0 && state == "leased" => {
                Ok(Some(IntentState::Leased))
            }
            Some((id, state, fence)) if id == intent && fence > 0 && state == "uncertain" => {
                Ok(Some(IntentState::Uncertain))
            }
            _ => Err(JournalError::Corrupt),
        }
    }
    // Process-local monotonic time is never stored. A missed deadline is uncertain,
    // not retryable: a worker may have acted before its lease was lost.
    fn expire_at(&mut self, now: Instant) -> Result<(), JournalError> {
        if let Some((id, fence, deadline)) = &self.active
            && now >= *deadline
        {
            let n = self.db.execute("UPDATE alert_intent_work SET state='uncertain' WHERE intent=?1 AND fence=?2 AND state='leased'", params![id, fence]).map_err(write_error)?;
            if n != 1 {
                return Err(JournalError::Corrupt);
            }
            self.active = None;
        }
        Ok(())
    }
    /// Claim one new local intent once. The fence reserves it; no retry,
    /// reclaim, external authorization or delivery outcome is implied.
    pub fn claim_intent(
        &mut self,
        authority: &str,
        scope: &str,
        request_id: &str,
        duration: Duration,
    ) -> Result<IntentLease, JournalError> {
        self.claim_intent_at(authority, scope, request_id, duration, Instant::now())
    }
    fn claim_intent_at(
        &mut self,
        authority: &str,
        scope: &str,
        request_id: &str,
        duration: Duration,
        now: Instant,
    ) -> Result<IntentLease, JournalError> {
        if authority != self.provision.authority || scope != self.provision.scope {
            return Err(JournalError::ScopeMismatch);
        }
        if duration.is_zero() || duration > MAX_LEASE {
            return Err(JournalError::Invalid);
        }
        let deadline = now.checked_add(duration).ok_or(JournalError::Invalid)?;
        self.expire_at(now)?;
        if self.active.is_some() {
            return Err(JournalError::Conflict);
        }
        if self.intent_state(request_id)? != Some(IntentState::Queued) {
            return Err(JournalError::Conflict);
        }
        let intent = self
            .read(request_id)?
            .and_then(|r| r.intent_id)
            .ok_or(JournalError::Corrupt)?;
        let n = self.db.execute("UPDATE alert_intent_work SET state='leased',fence=fence+1 WHERE request_id=?1 AND intent=?2 AND state='queued' AND fence=0", params![request_id, intent]).map_err(write_error)?;
        if n != 1 {
            return Err(JournalError::Corrupt);
        }
        self.active = Some((intent.clone(), 1, deadline));
        Ok(IntentLease {
            intent_id: intent,
            session: self.session.clone(),
            fence: 1,
        })
    }
    /// Relinquishment does not establish what the worker did; the result is uncertain.
    pub fn abandon_intent(&mut self, lease: IntentLease) -> Result<(), JournalError> {
        self.expire_at(Instant::now())?;
        if lease.session != self.session
            || !self
                .active
                .as_ref()
                .is_some_and(|(id, fence, _)| id == &lease.intent_id && *fence == lease.fence)
        {
            return Err(JournalError::Stale);
        }
        let n = self.db.execute("UPDATE alert_intent_work SET state='uncertain' WHERE intent=?1 AND fence=?2 AND state='leased'", params![lease.intent_id, lease.fence]).map_err(write_error)?;
        if n != 1 {
            return Err(JournalError::Corrupt);
        }
        self.active = None;
        Ok(())
    }
    /// ID is caller-stable within this store. All state and intent commit together.
    pub fn apply(
        &mut self,
        authority: &str,
        scope: &str,
        request_id: &str,
        expected_version: u64,
        input: Evaluation,
    ) -> Result<Receipt, JournalError> {
        if authority != self.provision.authority || scope != self.provision.scope {
            return Err(JournalError::ScopeMismatch);
        }
        if !valid_id(request_id) || expected_version > i64::MAX as u64 {
            return Err(JournalError::Invalid);
        }
        let input_bytes = encode(&input)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(write_error)?;
        let prior: Option<(i64, Vec<u8>, Vec<u8>)> = tx
            .query_row(
                "SELECT expected,input,receipt FROM alert_events WHERE id=?1",
                [request_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|_| JournalError::Storage)?;
        if let Some((old, bytes, receipt)) = prior {
            if old != expected_version as i64 || bytes != input_bytes {
                return Err(JournalError::Conflict);
            }
            return decode(&receipt);
        }
        let (version, checkpoint): (i64, Vec<u8>) = tx
            .query_row(
                "SELECT version,checkpoint FROM alert_state WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|_| JournalError::Storage)?;
        if version < 0 || version as u64 != expected_version {
            return Err(JournalError::Stale);
        }
        let count: u32 = tx
            .query_row("SELECT count(*) FROM alert_events", [], |r| r.get(0))
            .map_err(|_| JournalError::Storage)?;
        if count >= self.provision.capacity {
            return Err(JournalError::Full);
        }
        let checkpoint: Checkpoint = decode(&checkpoint)?;
        let old = checkpoint.lifecycle;
        let mut reducer =
            Reducer::restore(self.provision.rule, checkpoint).map_err(|_| JournalError::Corrupt)?;
        let decision = match reducer.evaluate(input).map_err(|_| JournalError::Invalid)? {
            Outcome::Applied(d) => d,
            Outcome::Duplicate(_) => return Err(JournalError::Conflict),
        };
        let next = version.checked_add(1).ok_or(JournalError::Full)?;
        let digest = event_digest(&self.provision, next)?;
        let event_id = format!("alert-v1:{digest}");
        let intent_id = (old != decision.lifecycle).then(|| format!("intent-v1:{digest}"));
        let receipt = Receipt {
            event_id,
            intent_id,
            version: next as u64,
            decision,
        };
        tx.execute(
            "INSERT INTO alert_events VALUES(?1,?2,?3,?4)",
            params![request_id, version, input_bytes, encode(&receipt)?],
        )
        .map_err(write_error)?;
        if let Some(intent) = &receipt.intent_id {
            tx.execute("INSERT INTO alert_intent_work(intent,request_id,fence,state) VALUES(?1,?2,0,'queued')", params![intent,request_id]).map_err(write_error)?;
        }
        tx.execute(
            "UPDATE alert_state SET version=?1,checkpoint=?2 WHERE id=1",
            params![next, encode(&reducer.checkpoint())?],
        )
        .map_err(write_error)?;
        tx.commit().map_err(write_error)?;
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_alert::{Direction, Lifecycle, Sample, Validity};
    fn provision(capacity: u32) -> Provision {
        Provision {
            version: 1,
            authority: "auth".into(),
            scope: "scope".into(),
            capacity,
            rule: Rule {
                rule_id: [1; 16],
                instance_id: [2; 16],
                revision: 1,
                profile: [3; 32],
                source: [4; 32],
                direction: Direction::Above,
                trigger: 10,
                recovery: 5,
                firing_hold: 2,
                recovery_hold: 2,
            },
        }
    }
    fn input(frontier: u64, time: u64, value: i64) -> Evaluation {
        Evaluation {
            profile: [3; 32],
            source: [4; 32],
            frontier,
            time,
            sample: Sample::Covered {
                value,
                evidence: [5; 32],
                coverage_start: 0,
                coverage_end: time,
            },
        }
    }
    #[test]
    fn local_intent_fence_expiry_and_restart_are_uncertain() {
        let root = tempfile::tempdir_in(std::env::var_os("TMPDIR").expect("disk TMPDIR")).unwrap();
        let path = root.path().join("journal");
        let mut j = AlertJournal::open(&path, provision(4)).unwrap();
        let receipt = j.apply("auth", "scope", "one", 0, input(1, 1, 11)).unwrap();
        assert!(receipt.intent_id.is_some());
        assert_eq!(j.intent_state("one").unwrap(), Some(IntentState::Queued));
        assert!(matches!(
            j.claim_intent("auth", "scope", "one", Duration::ZERO),
            Err(JournalError::Invalid)
        ));
        assert!(matches!(
            j.claim_intent("other", "scope", "one", Duration::from_secs(1)),
            Err(JournalError::ScopeMismatch)
        ));
        let now = Instant::now();
        let lease = j
            .claim_intent_at("auth", "scope", "one", Duration::from_secs(1), now)
            .unwrap();
        assert_eq!(j.intent_state("one").unwrap(), Some(IntentState::Leased));
        assert!(matches!(
            j.claim_intent("auth", "scope", "one", Duration::from_secs(1)),
            Err(JournalError::Conflict)
        ));
        j.expire_at(now + Duration::from_secs(1)).unwrap();
        assert_eq!(j.intent_state("one").unwrap(), Some(IntentState::Uncertain));
        assert!(matches!(j.abandon_intent(lease), Err(JournalError::Stale)));
        let other = j.apply("auth", "scope", "two", 1, input(2, 3, 11)).unwrap();
        assert!(other.intent_id.is_some());
        let old = j
            .claim_intent("auth", "scope", "two", Duration::from_secs(30))
            .unwrap();
        drop(j);
        let mut reopened = AlertJournal::open(&path, provision(4)).unwrap();
        assert_eq!(
            reopened.intent_state("two").unwrap(),
            Some(IntentState::Uncertain)
        );
        assert!(matches!(
            reopened.abandon_intent(old),
            Err(JournalError::Stale)
        ));
        assert!(matches!(
            reopened.claim_intent("auth", "scope", "two", Duration::from_secs(1)),
            Err(JournalError::Conflict)
        ));
    }

    #[test]
    fn work_insert_failure_rolls_back_event_and_legacy_stays_unmanaged() {
        let root = tempfile::tempdir_in(std::env::var_os("TMPDIR").expect("disk TMPDIR")).unwrap();
        let path = root.path().join("journal");
        let mut j = AlertJournal::open(&path, provision(2)).unwrap();
        j.db.execute_batch("CREATE TRIGGER fail_work BEFORE INSERT ON alert_intent_work BEGIN SELECT RAISE(ABORT,'fail'); END;").unwrap();
        assert!(j.apply("auth", "scope", "one", 0, input(1, 1, 11)).is_err());
        assert!(j.read("one").unwrap().is_none());
        j.db.execute_batch("DROP TRIGGER fail_work;").unwrap();
        let receipt = j.apply("auth", "scope", "one", 0, input(1, 1, 11)).unwrap();
        assert_eq!(
            j.apply("auth", "scope", "one", 0, input(1, 1, 11)).unwrap(),
            receipt
        );
        j.db.execute("DELETE FROM alert_work_meta", []).unwrap();
        drop(j);
        assert!(matches!(
            AlertJournal::open(&path, provision(2)),
            Err(JournalError::Corrupt)
        ));
    }

    fn create_v0(path: &Path) -> (Provision, Receipt) {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        let db = Connection::open(path.join("alert.sqlite")).unwrap();
        fs::set_permissions(path.join("alert.sqlite"), fs::Permissions::from_mode(0o600)).unwrap();
        db.execute_batch("CREATE TABLE alert_state(id INTEGER PRIMARY KEY CHECK(id=1), provision BLOB NOT NULL, version INTEGER NOT NULL, checkpoint BLOB NOT NULL); CREATE TABLE alert_events(id TEXT PRIMARY KEY, expected INTEGER NOT NULL, input BLOB NOT NULL, receipt BLOB NOT NULL);").unwrap();
        let p = provision(2);
        let sample = input(1, 1, 11);
        let mut reducer = Reducer::new(p.rule).unwrap();
        let Outcome::Applied(decision) = reducer.evaluate(sample).unwrap() else {
            panic!("expected event")
        };
        let digest = event_digest(&p, 1).unwrap();
        let receipt = Receipt {
            event_id: format!("alert-v1:{digest}"),
            intent_id: Some(format!("intent-v1:{digest}")),
            version: 1,
            decision,
        };
        db.execute(
            "INSERT INTO alert_state VALUES(1,?1,1,?2)",
            params![encode(&p).unwrap(), encode(&reducer.checkpoint()).unwrap()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO alert_events VALUES('one',0,?1,?2)",
            params![encode(&sample).unwrap(), encode(&receipt).unwrap()],
        )
        .unwrap();
        drop(db);
        (p, receipt)
    }

    #[test]
    fn genuine_v0_migrates_without_backfilling_legacy_intents() {
        let root = tempfile::tempdir_in(std::env::var_os("TMPDIR").expect("disk TMPDIR")).unwrap();
        let path = root.path().join("journal");
        let (p, receipt) = create_v0(&path);
        let mut j = AlertJournal::open(&path, p).unwrap();
        assert_eq!(j.read("one").unwrap(), Some(receipt));
        assert_eq!(
            j.intent_state("one").unwrap(),
            Some(IntentState::LegacyUnmanaged)
        );
        assert!(matches!(
            j.claim_intent("auth", "scope", "one", Duration::from_secs(1)),
            Err(JournalError::Conflict)
        ));
        j.apply("auth", "scope", "two", 1, input(2, 3, 11)).unwrap();
        assert_eq!(j.intent_state("two").unwrap(), Some(IntentState::Queued));
        assert!(matches!(
            j.apply("auth", "scope", "three", 2, input(3, 4, 11)),
            Err(JournalError::Full)
        ));
        j.db.execute_batch("DROP TABLE alert_intent_work").unwrap();
        drop(j);
        assert!(matches!(
            AlertJournal::open(&path, provision(2)),
            Err(JournalError::Corrupt)
        ));
    }

    #[test]
    fn failed_v0_migration_rolls_back_schema_and_retries() {
        let root = tempfile::tempdir_in(std::env::var_os("TMPDIR").expect("disk TMPDIR")).unwrap();
        let path = root.path().join("journal");
        let (p, receipt) = create_v0(&path);
        let db = Connection::open(path.join("alert.sqlite")).unwrap();
        let original_state: (Vec<u8>, i64, Vec<u8>) = db
            .query_row(
                "SELECT provision,version,checkpoint FROM alert_state WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        let original_input: Vec<u8> = db
            .query_row("SELECT input FROM alert_events WHERE id='one'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let bad = vec![b'x'; MAX_STATE + 1];
        db.execute("UPDATE alert_events SET receipt=?1 WHERE id='one'", [&bad])
            .unwrap();
        drop(db);
        assert!(matches!(
            AlertJournal::open(&path, p.clone()),
            Err(JournalError::Corrupt)
        ));
        let db = Connection::open(path.join("alert.sqlite")).unwrap();
        let marker: i64 = db
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        let tables: i64 = db.query_row("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name IN ('alert_work_meta','alert_intent_work')", [], |r| r.get(0)).unwrap();
        let state: (Vec<u8>, i64, Vec<u8>) = db
            .query_row(
                "SELECT provision,version,checkpoint FROM alert_state WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        let (actual_input, actual_bad): (Vec<u8>, Vec<u8>) = db
            .query_row(
                "SELECT input,receipt FROM alert_events WHERE id='one'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((marker, tables), (0, 0));
        assert_eq!(state, original_state);
        assert_eq!(actual_input, original_input);
        assert_eq!(actual_bad, bad);
        db.execute(
            "UPDATE alert_events SET receipt=?1 WHERE id='one'",
            [encode(&receipt).unwrap()],
        )
        .unwrap();
        drop(db);
        let j = AlertJournal::open(&path, p).unwrap();
        assert_eq!(j.read("one").unwrap(), Some(receipt));
        assert_eq!(
            j.intent_state("one").unwrap(),
            Some(IntentState::LegacyUnmanaged)
        );
    }

    #[test]
    fn corrupt_work_and_cross_journal_lease_fail_closed() {
        let root = tempfile::tempdir_in(std::env::var_os("TMPDIR").expect("disk TMPDIR")).unwrap();
        let mut a = AlertJournal::open(&root.path().join("a"), provision(2)).unwrap();
        let mut b = AlertJournal::open(&root.path().join("b"), provision(2)).unwrap();
        a.apply("auth", "scope", "one", 0, input(1, 1, 11)).unwrap();
        b.apply("auth", "scope", "one", 0, input(1, 1, 11)).unwrap();
        let token = a
            .claim_intent("auth", "scope", "one", Duration::from_secs(30))
            .unwrap();
        b.claim_intent("auth", "scope", "one", Duration::from_secs(30))
            .unwrap();
        assert!(matches!(b.abandon_intent(token), Err(JournalError::Stale)));
        b.db.execute("UPDATE alert_intent_work SET state='bad'", [])
            .unwrap();
        drop(b);
        assert!(matches!(
            AlertJournal::open(&root.path().join("b"), provision(2)),
            Err(JournalError::Corrupt)
        ));
    }

    #[test]
    fn refuses_non_disk_state_before_creating_journal_files() {
        for invalid in [
            "relative",
            "/tmp/fabric-alert-journal",
            "/dev/shm/fabric-alert-journal",
        ] {
            assert!(matches!(
                AlertJournal::open(Path::new(invalid), provision(4)),
                Err(JournalError::Invalid)
            ));
        }
    }

    #[test]
    fn disk_replay_fencing_and_bounded_intents() {
        let root = tempfile::tempdir_in(std::env::var_os("TMPDIR").expect("disk TMPDIR")).unwrap();
        let p = provision(3);
        let mut j = AlertJournal::open(&root.path().join("journal"), p.clone()).unwrap();
        let first = j.apply("auth", "scope", "one", 0, input(1, 1, 11)).unwrap();
        assert_eq!(first.decision.lifecycle, Lifecycle::Pending);
        assert!(first.intent_id.is_some());
        drop(j);
        let mut j = AlertJournal::open(&root.path().join("journal"), p.clone()).unwrap();
        let second = j.apply("auth", "scope", "two", 1, input(2, 3, 11)).unwrap();
        assert_eq!(second.decision.lifecycle, Lifecycle::Firing);
        assert!(second.intent_id.is_some());
        assert_eq!(
            j.apply("auth", "scope", "one", 0, input(1, 1, 11)).unwrap(),
            first
        );
        assert!(matches!(
            j.apply("auth", "scope", "one", 0, input(1, 1, 12)),
            Err(JournalError::Conflict)
        ));
        assert!(matches!(
            j.apply("wrong", "scope", "x", 2, input(3, 4, 11)),
            Err(JournalError::ScopeMismatch)
        ));
        assert!(matches!(
            j.apply("auth", "scope", "x", 1, input(3, 4, 11)),
            Err(JournalError::Stale)
        ));
        let mut bad = input(3, 4, 11);
        bad.source = [9; 32];
        assert!(matches!(
            j.apply("auth", "scope", "x", 2, bad),
            Err(JournalError::Invalid)
        ));
        let nochange = j
            .apply("auth", "scope", "three", 2, input(3, 4, 11))
            .unwrap();
        assert_eq!(nochange.decision.lifecycle, Lifecycle::Firing);
        assert_eq!(nochange.intent_id, None);
        assert!(matches!(
            j.apply("auth", "scope", "four", 3, input(4, 5, 11)),
            Err(JournalError::Full)
        ));
        assert_eq!(
            j.apply("auth", "scope", "two", 1, input(2, 3, 11)).unwrap(),
            second
        );
        drop(j);
        let mut wrong = p.clone();
        wrong.rule.source = [7; 32];
        assert!(matches!(
            AlertJournal::open(&root.path().join("journal"), wrong),
            Err(JournalError::ScopeMismatch)
        ));
        let mut j = AlertJournal::open(&root.path().join("journal"), p).unwrap();
        assert_eq!(
            j.apply("auth", "scope", "three", 2, input(3, 4, 11))
                .unwrap(),
            nochange
        );
        assert_eq!(j.read("one").unwrap(), Some(first));
        assert!(matches!(
            j.apply("auth", "scope", "four", 3, input(4, 5, 11)),
            Err(JournalError::Full)
        ));
    }
    #[test]
    fn missing_inputs_can_advance_time_at_same_frontier() {
        let root = tempfile::tempdir_in(std::env::var_os("TMPDIR").expect("disk TMPDIR")).unwrap();
        let mut journal = AlertJournal::open(&root.path().join("journal"), provision(4)).unwrap();
        journal
            .apply("auth", "scope", "initial", 0, input(1, 1, 11))
            .unwrap();
        let mut missing = input(1, 2, 11);
        missing.sample = Sample::Missing(Validity::NoData);
        let first = journal
            .apply("auth", "scope", "missing-1", 1, missing)
            .unwrap();
        assert_eq!(first.decision.lifecycle, Lifecycle::Inactive);
        missing.time = 3;
        let second = journal
            .apply("auth", "scope", "missing-2", 2, missing)
            .unwrap();
        assert_eq!(second.decision.lifecycle, Lifecycle::Inactive);
        assert!(second.intent_id.is_none());
    }

    #[test]
    fn sqlite_page_capacity_preserves_replay_and_frontier() {
        let root = tempfile::tempdir_in(std::env::var_os("TMPDIR").expect("disk TMPDIR")).unwrap();
        let dir = root.path().join("journal");
        let provision = provision(128);
        let mut journal = AlertJournal::open(&dir, provision.clone()).unwrap();
        let initial = input(1, 1, 11);
        let first = journal
            .apply("auth", "scope", "initial", 0, initial)
            .unwrap();
        let pages: u64 = journal
            .db
            .pragma_query_value(None, "page_count", |r| r.get(0))
            .unwrap();
        journal
            .db
            .pragma_update(None, "max_page_count", pages)
            .unwrap();

        let mut refused = None;
        for expected in 1..32 {
            let id = format!("event-{expected}");
            let evaluation = input(expected + 1, expected + 1, 11);
            match journal.apply("auth", "scope", &id, expected, evaluation) {
                Ok(receipt) => assert_eq!(receipt.version, expected + 1),
                Err(JournalError::Full) => {
                    refused = Some((id, expected, evaluation));
                    break;
                }
                Err(error) => panic!("SQLite page limit returned {error:?}"),
            }
        }
        let (id, expected, evaluation) = refused.expect("SQLite page limit must refuse a write");
        assert_eq!(journal.read(&id).unwrap(), None);
        assert_eq!(
            journal
                .apply("auth", "scope", "initial", 0, initial)
                .unwrap(),
            first
        );

        drop(journal);
        // Opening the journal restores the configured page limit.
        let mut reopened = AlertJournal::open(&dir, provision).unwrap();
        assert_eq!(reopened.read(&id).unwrap(), None);
        let accepted = reopened
            .apply("auth", "scope", &id, expected, evaluation)
            .unwrap();
        assert_eq!(accepted.version, expected + 1);
    }

    #[test]
    fn page_is_bounded_scoped_ordered_and_frozen_across_reopen() {
        let root = tempfile::tempdir_in(std::env::var_os("TMPDIR").expect("disk TMPDIR")).unwrap();
        let path = root.path().join("journal");
        let mut journal = AlertJournal::open(&path, provision(8)).unwrap();
        let empty = journal.read_page("auth", "scope", None, 2).unwrap();
        assert_eq!(empty.through, 0);
        assert!(empty.entries.is_empty() && empty.next.is_none());
        let first = journal
            .apply("auth", "scope", "z", 0, input(1, 1, 11))
            .unwrap();
        let second = journal
            .apply("auth", "scope", "a", 1, input(2, 3, 11))
            .unwrap();
        let third = journal
            .apply("auth", "scope", "m", 2, input(3, 4, 11))
            .unwrap();
        let page = journal.read_page("auth", "scope", None, 2).unwrap();
        assert_eq!(page.through, 3);
        assert_eq!(
            page.entries
                .iter()
                .map(|e| e.request_id.as_str())
                .collect::<Vec<_>>(),
            vec!["z", "a"]
        );
        assert_eq!(page.entries[0].receipt, first);
        assert_eq!(page.entries[1].receipt, second);
        let cursor = page.next.unwrap();
        journal
            .apply("auth", "scope", "new", 3, input(4, 5, 11))
            .unwrap();
        drop(journal);
        let journal = AlertJournal::open(&path, provision(8)).unwrap();
        let last = journal
            .read_page("auth", "scope", Some(cursor.clone()), 2)
            .unwrap();
        assert_eq!(last.through, 3);
        assert_eq!(last.entries.len(), 1);
        assert_eq!(last.entries[0].receipt, third);
        assert!(last.next.is_none());
        assert_eq!(
            last,
            journal.read_page("auth", "scope", Some(cursor), 2).unwrap()
        );
        assert!(matches!(
            journal.read_page("foreign", "scope", None, 1),
            Err(JournalError::ScopeMismatch)
        ));
        assert!(matches!(
            journal.read_page("auth", "foreign", None, 1),
            Err(JournalError::ScopeMismatch)
        ));
        for limit in [0, MAX_PAGE_ENTRIES + 1] {
            assert!(matches!(
                journal.read_page("auth", "scope", None, limit),
                Err(JournalError::Invalid)
            ));
        }
        for position in [
            PagePosition {
                through: 5,
                after: 0,
            },
            PagePosition {
                through: 3,
                after: 3,
            },
        ] {
            assert!(matches!(
                journal.read_page("auth", "scope", Some(position), 1),
                Err(JournalError::Invalid)
            ));
        }
    }
    #[test]
    fn page_refuses_corrupt_order_and_receipt() {
        let root = tempfile::tempdir_in(std::env::var_os("TMPDIR").expect("disk TMPDIR")).unwrap();
        let mut journal = AlertJournal::open(&root.path().join("journal"), provision(4)).unwrap();
        journal
            .apply("auth", "scope", "one", 0, input(1, 1, 11))
            .unwrap();
        journal
            .apply("auth", "scope", "two", 1, input(2, 3, 11))
            .unwrap();
        journal
            .db
            .execute("UPDATE alert_events SET expected=0 WHERE id='two'", [])
            .unwrap();
        assert!(matches!(
            journal.read_page("auth", "scope", None, 1),
            Err(JournalError::Corrupt)
        ));
        journal
            .db
            .execute(
                "UPDATE alert_events SET expected=1,receipt=x'00' WHERE id='two'",
                [],
            )
            .unwrap();
        assert!(matches!(
            journal.read_page(
                "auth",
                "scope",
                Some(PagePosition {
                    through: 2,
                    after: 1
                }),
                1
            ),
            Err(JournalError::Corrupt)
        ));
    }

    #[test]
    fn page_refuses_valid_but_inconsistent_persisted_entries() {
        let root = tempfile::tempdir_in(std::env::var_os("TMPDIR").expect("disk TMPDIR")).unwrap();
        let mut journal = AlertJournal::open(&root.path().join("journal"), provision(4)).unwrap();
        let original_input = input(1, 1, 11);
        let receipt = journal
            .apply("auth", "scope", "one", 0, original_input)
            .unwrap();
        let mut changed = original_input;
        changed.sample = Sample::Covered {
            value: 12,
            evidence: [5; 32],
            coverage_start: 0,
            coverage_end: 1,
        };
        journal
            .db
            .execute(
                "UPDATE alert_events SET input=?1 WHERE id='one'",
                [encode(&changed).unwrap()],
            )
            .unwrap();
        assert!(matches!(
            journal.read_page("auth", "scope", None, 1),
            Err(JournalError::Corrupt)
        ));
        let mut changed_receipt = receipt.clone();
        changed_receipt.decision.input = changed;
        changed.source = [9; 32];
        changed_receipt.decision.input.source = changed.source;
        journal
            .db
            .execute(
                "UPDATE alert_events SET input=?1,receipt=?2 WHERE id='one'",
                params![encode(&changed).unwrap(), encode(&changed_receipt).unwrap()],
            )
            .unwrap();
        assert!(matches!(
            journal.read_page("auth", "scope", None, 1),
            Err(JournalError::Corrupt)
        ));
        changed_receipt = receipt.clone();
        changed_receipt.event_id = "alert-v1:forged".into();
        journal
            .db
            .execute(
                "UPDATE alert_events SET input=?1,receipt=?2 WHERE id='one'",
                params![
                    encode(&original_input).unwrap(),
                    encode(&changed_receipt).unwrap()
                ],
            )
            .unwrap();
        assert!(matches!(
            journal.read_page("auth", "scope", None, 1),
            Err(JournalError::Corrupt)
        ));
    }

    #[test]
    fn committed_alert_survives_owner_process_kill() {
        use std::{
            io::Write,
            os::unix::process::ExitStatusExt,
            process::{Child, Command, Stdio},
            time::{Duration, Instant},
        };
        const CHILD: &str = "FABRIC_ALERT_JOURNAL_CHILD_ROOT";
        if let Some(root) = std::env::var_os(CHILD) {
            let root = std::path::PathBuf::from(root);
            let mut journal = AlertJournal::open(&root.join("journal"), provision(4)).unwrap();
            let receipt = journal
                .apply("auth", "scope", "one", 0, input(1, 1, 11))
                .unwrap();
            let _lease = journal
                .claim_intent("auth", "scope", "one", Duration::from_secs(300))
                .unwrap();
            assert_eq!(
                journal.intent_state("one").unwrap(),
                Some(IntentState::Leased)
            );
            let mut marker = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(root.join("ready-staged"))
                .unwrap();
            marker
                .write_all(&serde_json::to_vec(&receipt).unwrap())
                .unwrap();
            marker.sync_all().unwrap();
            drop(marker);
            std::fs::rename(root.join("ready-staged"), root.join("ready")).unwrap();
            loop {
                std::thread::park();
            }
        }
        struct Guard(Child);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let root = tempfile::tempdir_in(std::env::var_os("TMPDIR").expect("disk TMPDIR")).unwrap();
        let mut child = Guard(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "alert_journal::tests::committed_alert_survives_owner_process_kill",
                    "--nocapture",
                ])
                .env(CHILD, root.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        let marker = root.path().join("ready");
        while !marker.exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "child exited before readiness"
            );
            assert!(Instant::now() < deadline, "child readiness deadline");
            std::thread::sleep(Duration::from_millis(10));
        }
        // The marker appears after both the event and the local lease commit.
        let bytes = std::fs::read(&marker).unwrap();
        let receipt: Receipt = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(receipt.decision.lifecycle, Lifecycle::Pending);
        assert!(matches!(
            AlertJournal::open(&root.path().join("journal"), provision(4)),
            Err(JournalError::Storage)
        ));
        child.0.kill().unwrap();
        let status = child.0.wait().unwrap();
        assert_eq!(status.signal(), Some(9));
        let mut reopened = AlertJournal::open(&root.path().join("journal"), provision(4)).unwrap();
        assert_eq!(reopened.read("one").unwrap(), Some(receipt.clone()));
        assert_eq!(
            reopened.intent_state("one").unwrap(),
            Some(IntentState::Uncertain)
        );
        assert!(matches!(
            reopened.claim_intent("auth", "scope", "one", Duration::from_secs(1)),
            Err(JournalError::Conflict)
        ));
        assert_eq!(
            reopened
                .apply("auth", "scope", "one", 0, input(1, 1, 11))
                .unwrap(),
            receipt
        );
        let next = reopened
            .apply("auth", "scope", "two", 1, input(2, 3, 11))
            .unwrap();
        assert_eq!(next.decision.lifecycle, Lifecycle::Firing);
    }
}
