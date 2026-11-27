//! Canonical coordination and bounded durable engine handoff.
//! SQLite owns raw history in the baseline; engine mode retains only a bounded
//! unpublished batch locally. Acknowledged raw history then belongs to Greptime.
use fabric::{
    canonical::{CanonicalRow, ForwardRecord, MAX_BATCH, MAX_BATCH_BYTES},
    observation::{self, Identity},
};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};
pub(crate) const RESERVE: u64 = 128 * 1024;
const NODE_QUOTA_SHARE_DIVISOR: u64 = 4;
const COLUMNS: &str = "id,origin,source_id,raw,raw_len,raw_hash";
/// Explicitly releases `flock` before closing so sibling `fork()`s do not pin `owner.lock`.
struct ExclusiveLock(File);
impl Drop for ExclusiveLock {
    fn drop(&mut self) {
        let _ = rustix::fs::flock(&self.0, rustix::fs::FlockOperation::Unlock);
    }
}

pub struct Ledger {
    conn: Connection,
    dir: PathBuf,
    quota: u64,
    node_quota_bytes: BTreeMap<String, u64>,
    default_origin_quota: Option<u64>,
    enrolled_origins: Vec<String>,
    engine: Option<crate::engine::Greptime>,
    pub authority: String,
    _lock: ExclusiveLock,
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn digest(raw: &[u8]) -> String {
    Sha256::digest(raw)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn private_dir(dir: &Path) -> Result<(), String> {
    match fs::symlink_metadata(dir) {
        Ok(m)
            if !m.is_dir()
                || m.file_type().is_symlink()
                || m.uid() != rustix::process::getuid().as_raw()
                || m.permissions().mode() & 0o077 != 0 =>
        {
            return Err("state must be a private owned directory".into());
        }
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            builder.create(dir).map_err(err)?;
            let dir_fd = rustix::fs::open(
                dir,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::DIRECTORY
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::empty(),
            )
            .map_err(err)?;
            rustix::fs::fchmod(&dir_fd, rustix::fs::Mode::from_raw_mode(0o700)).map_err(err)?;
            if let Some(p) = dir.parent() {
                File::open(p).and_then(|f| f.sync_all()).map_err(err)?;
            }
        }
        Err(e) => return Err(err(e)),
    }
    Ok(())
}
fn enforce_private_state_files(dir: &Path, names: &[&str]) -> Result<(), String> {
    let dir_fd = rustix::fs::open(
        dir,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(err)?;
    for name in names {
        match rustix::fs::openat(
            &dir_fd,
            *name,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        ) {
            Ok(fd) => {
                let st = rustix::fs::fstat(&fd).map_err(err)?;
                if rustix::fs::FileType::from_raw_mode(st.st_mode as _)
                    != rustix::fs::FileType::RegularFile
                {
                    return Err("invalid state file".into());
                }
                if ((st.st_mode as u32) & 0o777) != 0o600 {
                    rustix::fs::fchmod(&fd, rustix::fs::Mode::from_raw_mode(0o600)).map_err(err)?;
                }
            }
            Err(rustix::io::Errno::NOENT) => {}
            Err(rustix::io::Errno::LOOP) => return Err("invalid state file".into()),
            Err(e) => return Err(err(e)),
        }
    }
    Ok(())
}
impl Ledger {
    pub fn open(dir: &Path, quota: u64) -> Result<Self, String> {
        Self::open_with_engine_auth(dir, quota, None, None, false)
    }
    pub fn open_with_engine(
        dir: &Path,
        quota: u64,
        endpoint: Option<&str>,
    ) -> Result<Self, String> {
        Self::open_with_engine_auth(dir, quota, endpoint, None, false)
    }
    pub fn open_with_engine_auth(
        dir: &Path,
        quota: u64,
        endpoint: Option<&str>,
        engine_auth_token: Option<&str>,
        allow_unauthenticated_loopback: bool,
    ) -> Result<Self, String> {
        Self::open_with_quotas_and_auth(
            dir,
            quota,
            endpoint,
            BTreeMap::new(),
            None,
            Vec::new(),
            engine_auth_token,
            allow_unauthenticated_loopback,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_with_quotas_and_auth(
        dir: &Path,
        quota: u64,
        endpoint: Option<&str>,
        node_quota_bytes: BTreeMap<String, u64>,
        default_origin_quota: Option<u64>,
        enrolled_origins: Vec<String>,
        engine_auth_token: Option<&str>,
        allow_unauthenticated_loopback: bool,
    ) -> Result<Self, String> {
        if quota < 1024 * 1024 {
            return Err("canonical quota must be >=1MiB".into());
        }
        let dedicated_cap = node_quota_bytes
            .values()
            .try_fold(0u64, |total, &limit| total.checked_add(limit));
        if node_quota_bytes.values().any(|&limit| limit == 0)
            || !dedicated_cap.is_some_and(|cap| cap <= quota / NODE_QUOTA_SHARE_DIVISOR)
        {
            return Err("canonical node quotas must be positive and total at most one quarter of canonical quota".into());
        }
        if default_origin_quota == Some(0) {
            return Err("canonical fair-share origin quota must be positive".into());
        }
        private_dir(dir)?;
        for name in ["owner.lock", "canonical.sqlite", "canonical.sqlite-journal"] {
            if let Ok(m) = fs::symlink_metadata(dir.join(name))
                && (m.file_type().is_symlink()
                    || !m.is_file()
                    || m.uid() != rustix::process::getuid().as_raw())
            {
                return Err("invalid state file".into());
            }
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(dir.join("owner.lock"))
            .map_err(err)?;
        rustix::fs::fchmod(&lock, rustix::fs::Mode::from_raw_mode(0o600)).map_err(err)?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .map_err(err)?;
        let lock = ExclusiveLock(lock);
        let conn = Connection::open(dir.join("canonical.sqlite")).map_err(err)?;
        conn.busy_timeout(std::time::Duration::from_millis(100))
            .map_err(err)?;
        conn.execute_batch("PRAGMA journal_mode=PERSIST; PRAGMA synchronous=EXTRA; PRAGMA temp_store=MEMORY; PRAGMA cache_size=-512;
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS records(id INTEGER PRIMARY KEY AUTOINCREMENT,origin TEXT NOT NULL,source_id INTEGER NOT NULL,producer TEXT NOT NULL,generation TEXT NOT NULL,sequence TEXT NOT NULL,raw BLOB NOT NULL,raw_len INTEGER NOT NULL,raw_hash TEXT NOT NULL,published INTEGER NOT NULL,UNIQUE(origin,source_id),UNIQUE(origin,producer,generation,sequence));
CREATE TABLE IF NOT EXISTS origins(origin TEXT PRIMARY KEY,through INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS registries(name TEXT PRIMARY KEY,descriptor TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS pending_records ON records(id) WHERE published=0;").map_err(err)?;
        enforce_private_state_files(dir, &["canonical.sqlite", "canonical.sqlite-journal"])?;
        let page: u64 = conn
            .pragma_query_value(None, "page_size", |r| r.get(0))
            .map_err(err)?;
        // Retain the rollback journal and reuse its synced commit marker instead
        // of unlinking/recreating it for each bounded batch. It remains included
        // in disk_bytes and the database-plus-recovery quota reservation.
        let journal: String = conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .map_err(err)?;
        let sync: u64 = conn
            .pragma_query_value(None, "synchronous", |r| r.get(0))
            .map_err(err)?;
        if journal != "persist" || sync != 3 || !(512..=65536).contains(&page) {
            return Err("unsupported canonical durability configuration".into());
        }
        let max = (quota - RESERVE) / 2 / page;
        let pages: u64 = conn
            .pragma_query_value(None, "page_count", |r| r.get(0))
            .map_err(err)?;
        if pages > max {
            return Err("canonical database exceeds quota".into());
        }
        conn.pragma_update(None, "max_page_count", max)
            .map_err(err)?;
        let backend = if endpoint.is_some() {
            "greptime"
        } else {
            "sqlite"
        };
        let old: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key='backend'", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(err)?;
        if old.as_deref().is_some_and(|v| v != backend) {
            return Err("canonical backend change requires explicit migration".into());
        }
        conn.execute("INSERT OR IGNORE INTO meta VALUES('backend',?1)", [backend])
            .map_err(err)?;
        let authority = match conn
            .query_row("SELECT value FROM meta WHERE key='authority'", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .map_err(err)?
        {
            Some(id) => id,
            None => {
                let id = fs::read_to_string("/proc/sys/kernel/random/uuid")
                    .map_err(err)?
                    .trim()
                    .to_owned();
                conn.execute("INSERT INTO meta VALUES('authority',?1)", [&id])
                    .map_err(err)?;
                id
            }
        };
        for (name, descriptor) in [
            (observation::REGISTRY, observation::DESCRIPTOR),
            (observation::REGISTRY_V3, observation::DESCRIPTOR_V3),
        ] {
            let old: Option<String> = conn
                .query_row(
                    "SELECT descriptor FROM registries WHERE name=?1",
                    [name],
                    |r| r.get(0),
                )
                .optional()
                .map_err(err)?;
            if old.as_deref().is_some_and(|v| v != descriptor) {
                return Err("canonical registry conflict".into());
            }
            conn.execute(
                "INSERT OR IGNORE INTO registries VALUES(?1,?2)",
                params![name, descriptor],
            )
            .map_err(err)?;
        }
        let engine = if let Some(endpoint) = endpoint {
            let old: Option<String> = conn
                .query_row("SELECT value FROM meta WHERE key='cursor_key'", [], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(err)?;
            let key = if let Some(encoded) = old {
                if encoded.len() != 64 || !encoded.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err("invalid persisted cursor key".into());
                }
                let mut key = [0; 32];
                for (i, b) in key.iter_mut().enumerate() {
                    *b = u8::from_str_radix(&encoded[2 * i..2 * i + 2], 16).map_err(err)?;
                }
                key
            } else {
                let mut key = [0; 32];
                File::open("/dev/urandom")
                    .and_then(|mut f| f.read_exact(&mut key))
                    .map_err(err)?;
                let encoded: String = key.iter().map(|b| format!("{b:02x}")).collect();
                conn.execute("INSERT INTO meta VALUES('cursor_key',?1)", [encoded])
                    .map_err(err)?;
                key
            };
            let engine = crate::engine::Greptime::open_with_auth(
                endpoint,
                key,
                engine_auth_token.map(str::to_owned),
                allow_unauthenticated_loopback,
            )?;
            engine.bind_authority(&authority)?;
            Some(engine)
        } else {
            None
        };
        File::open(dir).and_then(|f| f.sync_all()).map_err(err)?;
        let mut ledger = Self {
            conn,
            dir: dir.to_owned(),
            quota,
            node_quota_bytes,
            default_origin_quota,
            enrolled_origins,
            engine,
            authority,
            _lock: lock,
        };
        if let Some(engine) = &ledger.engine {
            let reserved: u64 = ledger
                .conn
                .query_row("SELECT coalesce(max(id),0) FROM records", [], |r| r.get(0))
                .map_err(err)?;
            if engine.highest_canonical_id()? > reserved {
                return Err("canonical engine is newer than its control history; coordinated recovery required".into());
            }
            engine.verify_frontiers(&ledger.frontiers()?)?;
        }
        ledger.refresh_node_usage()?;
        ledger.recover_pending()?;
        Ok(ledger)
    }
    fn refresh_node_usage(&mut self) -> Result<(), String> {
        let mut tracked: std::collections::BTreeSet<String> =
            self.node_quota_bytes.keys().cloned().collect();
        if self.default_origin_quota.is_some() {
            tracked.extend(self.enrolled_origins.iter().cloned());
        }
        if tracked.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction().map_err(err)?;
        tx.execute(
            "CREATE TABLE IF NOT EXISTS node_quota_usage (
                origin TEXT PRIMARY KEY,
                raw_bytes INTEGER NOT NULL CHECK(raw_bytes >= 0),
                source_through INTEGER NOT NULL CHECK(source_through >= 0)
            )",
            [],
        )
        .map_err(err)?;
        // Existing ledgers and older writers may have admitted rows without
        // accounting. The origin/source index finds just the uncounted tail,
        // including reservations that have yet to publish to the engine.
        for origin in &tracked {
            let existing: Option<(u64, u64)> = tx
                .query_row(
                    "SELECT raw_bytes, source_through FROM node_quota_usage WHERE origin=?1",
                    [origin],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(err)?;
            let (used, through) = existing.unwrap_or((0, 0));
            let (additional, last): (u64, u64) = tx
                .query_row(
                    "SELECT COALESCE(SUM(raw_len),0), COALESCE(MAX(source_id),?2)
                       FROM records WHERE origin=?1 AND source_id>?2",
                    params![origin, through],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(err)?;
            let total = used
                .checked_add(additional)
                .ok_or("canonical node quota usage overflow")?;
            if existing != Some((total, last)) {
                tx.execute(
                    "INSERT OR REPLACE INTO node_quota_usage VALUES(?1,?2,?3)",
                    params![origin, total, last],
                )
                .map_err(err)?;
            }
        }
        tx.commit().map_err(err)
    }
    pub fn engine(&self) -> Option<crate::engine::Greptime> {
        self.engine.clone()
    }
    pub fn frontiers(&self) -> Result<BTreeMap<String, u64>, String> {
        self.conn
            .prepare("SELECT origin,through FROM origins ORDER BY origin")
            .map_err(err)?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(err)?
            .collect::<Result<_, _>>()
            .map_err(err)
    }
    pub fn disk_bytes(&self) -> Result<u64, String> {
        fs::read_dir(&self.dir)
            .map_err(err)?
            .try_fold(0u64, |sum, e| {
                let e = e.map_err(err)?;
                let m = e.metadata().map_err(err)?;
                if !m.is_file() {
                    return Err("unexpected canonical state entry".into());
                }
                sum.checked_add(m.len()).ok_or("state byte overflow".into())
            })
    }
    fn room_for(&self, bytes: usize, ceiling: u64) -> Result<(), String> {
        let free: u64 = self
            .conn
            .pragma_query_value(None, "freelist_count", |r| r.get(0))
            .map_err(err)?;
        let page: u64 = self
            .conn
            .pragma_query_value(None, "page_size", |r| r.get(0))
            .map_err(err)?;
        let growth = (bytes as u64 + 32768).saturating_sub(free.saturating_mul(page));
        if self
            .disk_bytes()?
            .saturating_add(growth.saturating_mul(2))
            .saturating_add(RESERVE)
            > ceiling
        {
            let reason = if ceiling < self.quota {
                "canonical storage headroom reserved for workload origins"
            } else {
                "canonical storage full"
            };
            return Err(reason.into());
        }
        Ok(())
    }
    /// Install an immutable, operator-provisioned scope/owner binding.
    pub(crate) fn provision_plan_scopes(
        &mut self,
        grants: &[crate::DecisionWriterGrant],
    ) -> Result<(), String> {
        let tx = self.conn.transaction().map_err(err)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS plan_schema(version INTEGER NOT NULL CHECK(version=1));
            CREATE TABLE IF NOT EXISTS plan_scope(scope TEXT PRIMARY KEY, owner TEXT NOT NULL, origin TEXT NOT NULL, producer TEXT NOT NULL, head INTEGER);
            CREATE TABLE IF NOT EXISTS plan_decision(id INTEGER PRIMARY KEY, scope TEXT NOT NULL, writer TEXT NOT NULL, command_id TEXT NOT NULL, request_hash TEXT NOT NULL, previous_head INTEGER, proposal_id TEXT NOT NULL, result TEXT NOT NULL, UNIQUE(scope,writer,command_id));
            CREATE TABLE IF NOT EXISTS plan_cursor_epoch(scope TEXT PRIMARY KEY, epoch INTEGER NOT NULL);").map_err(err)?;
        let versions: Vec<i64> = {
            let mut q = tx.prepare("SELECT version FROM plan_schema").map_err(err)?;
            q.query_map([], |r| r.get(0))
                .map_err(err)?
                .collect::<Result<_, _>>()
                .map_err(err)?
        };
        if versions.is_empty() {
            tx.execute("INSERT INTO plan_schema VALUES(1)", [])
                .map_err(err)?;
        } else if versions != [1] {
            return Err("unsupported plan schema".into());
        }
        let cursor_key = fabric::canonical_auth::random_nonce_hex()?;
        tx.execute(
            "INSERT OR IGNORE INTO meta(key,value) VALUES('plan_cursor_key',?1)",
            [&cursor_key],
        )
        .map_err(err)?;
        for grant in grants {
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM plan_scope WHERE scope=?1)",
                    [&grant.scope],
                    |r| r.get(0),
                )
                .map_err(err)?;
            let count: u64 = tx
                .query_row("SELECT count(*) FROM plan_scope", [], |r| r.get(0))
                .map_err(err)?;
            if !exists && count >= 64 {
                return Err("plan scope capacity exhausted".into());
            }
            tx.execute("INSERT OR IGNORE INTO plan_scope(scope,owner,origin,producer,head) VALUES(?1,?2,?3,?4,NULL)", params![grant.scope,grant.writer,grant.origin,grant.producer]).map_err(err)?;
            let matches: bool = tx
                .query_row(
                    "SELECT owner=?2 AND origin=?3 AND producer=?4 FROM plan_scope WHERE scope=?1",
                    params![grant.scope, grant.writer, grant.origin, grant.producer],
                    |r| r.get(0),
                )
                .map_err(err)?;
            if !matches {
                return Err("plan scope policy mismatch".into());
            }
            tx.execute(
                "INSERT OR IGNORE INTO plan_cursor_epoch(scope,epoch) VALUES(?1,0)",
                [&grant.scope],
            )
            .map_err(err)?;
        }
        tx.commit().map_err(err)
    }

    /// The caller has authenticated a decision writer; scope ownership remains authoritative here.
    pub(crate) fn plan_cursor_identity(
        &self,
        writer: &str,
        scope: &str,
    ) -> Result<(i64, i64), String> {
        if scope.is_empty() || scope.len() > 128 {
            return Err("unauthorized plan scope".into());
        }
        self.conn.query_row("SELECT e.epoch, COALESCE((SELECT MAX(id) FROM plan_decision WHERE scope=?1),0) FROM plan_scope s JOIN plan_cursor_epoch e ON e.scope=s.scope WHERE s.scope=?1 AND s.owner=?2", params![scope, writer], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional().map_err(err)?.ok_or_else(|| "unauthorized plan scope".to_string())
    }
    pub(crate) fn revoke_plan_cursors(&mut self, writer: &str, scope: &str) -> Result<(), String> {
        if scope.is_empty() || scope.len() > 128 {
            return Err("unauthorized plan scope".into());
        }
        let tx = self.conn.transaction().map_err(err)?;
        let changed = tx.execute("UPDATE plan_cursor_epoch SET epoch=epoch+1 WHERE scope=?1 AND epoch<9223372036854775807 AND EXISTS (SELECT 1 FROM plan_scope WHERE scope=?1 AND owner=?2)", params![scope, writer]).map_err(err)?;
        if changed != 1 {
            return Err("unauthorized plan scope or exhausted epoch".into());
        }
        tx.commit().map_err(err)
    }
    pub(crate) fn plan_page(
        &mut self,
        writer: &str,
        request: &fabric::canonical::PlanPageRequest,
    ) -> Result<Result<fabric::canonical::PlanPage, fabric::canonical::PlanCursorFailure>, String>
    {
        use fabric::canonical::{PlanCursorFailure as Failure, PlanDecision, PlanPage};
        let (epoch, frontier) = self.plan_cursor_identity(writer, &request.scope.0)?;
        if request.limit == 0
            || request.limit > 64
            || request.ttl_secs == 0
            || request.ttl_secs > 900
        {
            return Ok(Err(Failure::Invalid));
        }
        let key: String = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key='plan_cursor_key'",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(err)?
            .as_secs();
        let mut cursor = if let Some(token) = &request.cursor {
            let Some(cursor) =
                crate::plan_cursor::decode(token, &key, &self.authority, writer, &request.scope.0)
            else {
                return Ok(Err(Failure::Invalid));
            };
            if cursor.epoch != epoch {
                return Ok(Err(Failure::Revoked));
            }
            if now >= cursor.expires {
                return Ok(Err(Failure::Expired));
            }
            cursor
        } else {
            let count: i64 = self
                .conn
                .query_row(
                    "SELECT count(*) FROM plan_decision WHERE scope=?1 AND id<=?2",
                    params![request.scope.0, frontier],
                    |r| r.get(0),
                )
                .map_err(err)?;
            crate::plan_cursor::Cursor {
                epoch,
                frontier,
                count,
                last: 0,
                expires: now
                    .checked_add(u64::from(request.ttl_secs))
                    .ok_or("clock overflow")?,
            }
        };
        if cursor.frontier < 0
            || cursor.last < 0
            || cursor.last > cursor.frontier
            || cursor.count < 0
            || cursor.count > 4096
        {
            return Ok(Err(Failure::Invalid));
        }
        let count: i64 = self
            .conn
            .query_row(
                "SELECT count(*) FROM plan_decision WHERE scope=?1 AND id<=?2",
                params![request.scope.0, cursor.frontier],
                |r| r.get(0),
            )
            .map_err(err)?;
        if count != cursor.count {
            return Ok(Err(Failure::Gap));
        }
        let mut stmt = self.conn.prepare("SELECT id,result FROM plan_decision WHERE scope=?1 AND id>?2 AND id<=?3 ORDER BY id LIMIT ?4").map_err(err)?;
        let rows: Vec<(i64, String)> = stmt
            .query_map(
                params![
                    request.scope.0,
                    cursor.last,
                    cursor.frontier,
                    i64::from(request.limit) + 1
                ],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(err)?
            .collect::<Result<_, _>>()
            .map_err(err)?;
        let more = rows.len() > usize::from(request.limit);
        let mut decisions = Vec::with_capacity(rows.len().min(usize::from(request.limit)));
        for (id, row) in rows.into_iter().take(usize::from(request.limit)) {
            let decision = serde_json::from_str::<PlanDecision>(&row).map_err(err)?;
            if decision.head != id
                || decision.scope != request.scope
                || decision.authority != self.authority
            {
                return Ok(Err(Failure::Gap));
            }
            decisions.push(decision);
        }
        let next_cursor = if more {
            cursor.last = decisions.last().ok_or("missing decision")?.head;
            Some(crate::plan_cursor::encode(
                &cursor,
                &key,
                &self.authority,
                writer,
                &request.scope.0,
            )?)
        } else {
            None
        };
        Ok(Ok(PlanPage {
            authority: self.authority.clone(),
            scope: request.scope.clone(),
            frontier: cursor.frontier,
            decisions,
            next_cursor,
        }))
    }

    /// Authenticate outside this method; policy, replay, CAS and result commit together.
    pub(crate) fn accept_plan(
        &mut self,
        binding: &crate::handshake::WriterBinding,
        body: &str,
    ) -> Result<String, String> {
        use fabric::{
            canonical::{self, AcceptPlan, PlanDecision, PlanProposal},
            observation::{Data, StoredObservation},
        };
        let valid = |s: &str| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
        };
        observation::check_json_depth(
            body.as_bytes(),
            canonical::MAX_ACCEPT_BODY,
            observation::MAX_DEPTH + 2,
        )?;
        let command: AcceptPlan = serde_json::from_str(body).map_err(err)?;
        if command.version != 1
            || command.authority != self.authority
            || !valid(&command.scope.0)
            || !valid(&command.command_id)
            || !valid(&command.origin)
            || command.source_id == 0
            || command.source_id > i64::MAX as u64
            || command.expected_head.is_some_and(|h| h <= 0)
            || command.proposal_raw.len() > observation::MAX_FRAME
        {
            return Err("invalid acceptance command".into());
        }
        let mut hash = Sha256::new();
        hash.update(b"FabricO11y accept plan body v1\0");
        hash.update(body.as_bytes());
        let request_hash: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        // Do not reject an existing committed retry just because new storage is full.
        let available = self.room_for(body.len().saturating_add(4096), self.quota);
        let authority = self.authority.clone();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        // The in-memory value was loaded at open; fence it against the durable
        // authority within the same write transaction as policy and replay.
        let durable_authority: Option<String> = tx
            .query_row("SELECT value FROM meta WHERE key='authority'", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(err)?;
        if durable_authority.as_deref() != Some(authority.as_str())
            || durable_authority.as_deref() != Some(command.authority.as_str())
        {
            return Err("canonical authority mismatch".into());
        }
        let policy: Option<(String, String, String)> = tx
            .query_row(
                "SELECT owner,origin,producer FROM plan_scope WHERE scope=?1",
                [&command.scope.0],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(err)?;
        let (owner, origin, producer) = policy.ok_or("unauthorized plan scope")?;
        let writer = binding.writer.as_str();
        if command.scope.0 != binding.scope
            || owner != writer
            || origin != binding.origin
            || producer != binding.producer
        {
            return Err("unauthorized plan scope".into());
        }
        let saved: Option<(String,String)> = tx.query_row("SELECT request_hash,result FROM plan_decision WHERE scope=?1 AND writer=?2 AND command_id=?3", params![command.scope.0,writer,command.command_id], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(err)?;
        if saved
            .as_ref()
            .is_some_and(|(saved_hash, _)| saved_hash != &request_hash)
        {
            return Err("plan command conflict".into());
        }
        if command.origin != origin {
            return Err("unauthorized proposal origin".into());
        }
        let StoredObservation::V3(observation) =
            observation::decode_versioned(command.proposal_raw.as_bytes())?
        else {
            return Err("proposal is not v3".into());
        };
        if observation.identity.producer != producer
            || serde_json::to_vec(&observation).map_err(err)? != command.proposal_raw.as_bytes()
        {
            return Err("invalid canonical proposal frame".into());
        }
        if let Some((_, result)) = saved {
            return Ok(result);
        }
        let head: Option<i64> = tx
            .query_row(
                "SELECT head FROM plan_scope WHERE scope=?1",
                [&command.scope.0],
                |r| r.get(0),
            )
            .map_err(err)?;
        if head != command.expected_head {
            return Err("stale plan head".into());
        }
        available?;
        let count: u64 = tx
            .query_row("SELECT count(*) FROM plan_decision", [], |r| r.get(0))
            .map_err(err)?;
        if count >= 4096 {
            return Err("plan decision capacity exhausted".into());
        }
        let Data::JsonLog { fields } = &observation.data else {
            return Err("invalid proposal carrier".into());
        };
        if fields.len() != 1 {
            return Err("invalid proposal carrier".into());
        }
        let raw = fields
            .get(canonical::PROPOSAL_MARKER)
            .and_then(|v| v.as_str())
            .ok_or("invalid proposal carrier")?;
        if raw.len() > 4096 {
            return Err("proposal carrier too large".into());
        }
        let proposal: PlanProposal = serde_json::from_str(raw).map_err(err)?;
        if proposal.version != 1
            || proposal.scope != command.scope
            || proposal.proposal_id.len() != 64
            || !proposal
                .proposal_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("invalid proposal carrier".into());
        }
        let record: Option<(String,u64)> = tx.query_row("SELECT raw_hash,raw_len FROM records WHERE origin=?1 AND source_id=?2 AND producer=?3 AND generation=?4 AND sequence=?5", params![origin,command.source_id,observation.identity.producer,observation.identity.generation,observation.identity.sequence.to_string()], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(err)?;
        if record
            != Some((
                digest(command.proposal_raw.as_bytes()),
                command.proposal_raw.len() as u64,
            ))
        {
            return Err("canonical proposal mismatch".into());
        }
        let next: i64 = tx
            .query_row("SELECT coalesce(max(id),0)+1 FROM plan_decision", [], |r| {
                r.get(0)
            })
            .map_err(err)?;
        let result = serde_json::to_string(&PlanDecision {
            version: 1,
            authority,
            scope: command.scope.clone(),
            command_id: command.command_id.clone(),
            writer: writer.into(),
            previous_head: head,
            head: next,
            proposal_id: proposal.proposal_id.clone(),
        })
        .map_err(err)?;
        tx.execute("INSERT INTO plan_decision(id,scope,writer,command_id,request_hash,previous_head,proposal_id,result) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![next,command.scope.0,writer,command.command_id,request_hash,head,proposal.proposal_id,result]).map_err(err)?;
        let updated = tx
            .execute(
                "UPDATE plan_scope SET head=?2 WHERE scope=?1 AND owner=?3 AND head IS ?4",
                params![command.scope.0, next, writer, head],
            )
            .map_err(err)?;
        if updated != 1 {
            return Err("stale plan head".into());
        }
        tx.commit().map_err(err)?;
        Ok(result)
    }

    pub fn ingest(&mut self, origin: &str, records: &[ForwardRecord]) -> Result<u64, String> {
        if origin.is_empty()
            || origin.len() > 128
            || records.is_empty()
            || records.len() > MAX_BATCH
        {
            return Err("invalid origin/batch".into());
        }
        let bytes = records
            .iter()
            .try_fold(0usize, |a, r| a.checked_add(r.raw.len()))
            .ok_or("batch overflow")?;
        if bytes > MAX_BATCH_BYTES {
            return Err("batch byte limit".into());
        }
        for r in records {
            if r.source_id == 0 || r.source_id > i64::MAX as u64 {
                return Err("source ID outside storage domain".into());
            }
            observation::decode_versioned(r.raw.as_bytes())?;
        }
        // An unknown engine write remains reserved until reconciled. Never reuse
        // its canonical/source IDs, including for a conflicting caller retry.
        self.recover_pending()?;
        let initial = self.frontiers()?.get(origin).copied().unwrap_or(0);
        // Materialize confirmed retries once per bounded request. In engine
        // mode, a large lost-ack retry must not perform one HTTP round trip per
        // record while holding the writer admission slot.
        let retry_ids = records
            .iter()
            .filter(|r| r.source_id <= initial)
            .map(|r| r.source_id)
            .collect::<std::collections::BTreeSet<_>>();
        let retries = if retry_ids.is_empty() {
            BTreeMap::new()
        } else {
            const MAX_IN_CLAUSE_IDS: usize = 500;
            let all_ids: Vec<u64> = retry_ids.iter().copied().collect();
            let mut loaded = BTreeMap::new();
            let mut loaded_bytes = 0usize;
            for chunk in all_ids.chunks(MAX_IN_CLAUSE_IDS) {
                let ids = chunk
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                let remaining_bytes = MAX_BATCH_BYTES.saturating_sub(loaded_bytes);
                let rows = self.access().rows(
                    &format!("SELECT {COLUMNS} FROM records WHERE published=1 AND origin=?1 AND source_id IN ({ids}) ORDER BY id"),
                    [origin],
                    remaining_bytes,
                )?;
                for row in rows {
                    loaded_bytes = loaded_bytes.saturating_add(row.raw.len());
                    loaded.insert(row.source_id, row);
                }
            }
            if loaded.len() != retry_ids.len() {
                return Err("canonical retry history unavailable within byte budget".into());
            }
            loaded
        };
        let mut through = initial;
        let mut additions: Vec<&ForwardRecord> = Vec::new();
        for r in records {
            if r.source_id <= initial {
                let old = retries
                    .get(&r.source_id)
                    .ok_or("canonical prefix missing record")?;
                if old.raw != r.raw {
                    return Err("identity/source conflict".into());
                }
            } else if r.source_id <= through {
                if !additions
                    .iter()
                    .any(|old| old.source_id == r.source_id && old.raw == r.raw)
                {
                    return Err("batch source conflict".into());
                }
            } else {
                through = admission_kernel::handoff::advance(through, r.source_id)
                    .ok_or("source prefix gap")?;
                additions.push(r);
            }
        }
        if additions.is_empty() {
            return Ok(through);
        }
        let bytes: usize = additions.iter().map(|r| r.raw.len()).sum();
        let explicit_quota = self.node_quota_bytes.get(origin).copied();
        let quota = explicit_quota.or(self.default_origin_quota);
        let engine_mode = self.engine.is_some();
        let ceiling = if explicit_quota.is_some() {
            self.quota - self.quota / NODE_QUOTA_SHARE_DIVISOR
        } else {
            self.quota
        };
        self.room_for(bytes, ceiling)?;
        let tx = self.conn.transaction().map_err(err)?;
        if let Some(limit) = quota {
            let used: u64 = tx
                .query_row(
                    "SELECT raw_bytes FROM node_quota_usage WHERE origin=?1",
                    [origin],
                    |row| row.get(0),
                )
                .optional()
                .map_err(err)?
                .unwrap_or(0);
            let next = used
                .checked_add(bytes as u64)
                .filter(|&total| total <= limit)
                .ok_or("canonical node payload quota exhausted")?;
            tx.execute(
                "INSERT OR REPLACE INTO node_quota_usage VALUES(?1,?2,?3)",
                params![origin, next, through],
            )
            .map_err(err)?;
        }
        tx.execute("INSERT OR IGNORE INTO origins VALUES(?1,0)", [origin])
            .map_err(err)?;
        for r in additions {
            let parsed = observation::decode_versioned(r.raw.as_bytes())?;
            let i = parsed.identity();
            tx.execute("INSERT INTO records(origin,source_id,producer,generation,sequence,raw,raw_len,raw_hash,published) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![origin,r.source_id,i.producer,i.generation,i.sequence.to_string(),r.raw.as_bytes(),r.raw.len(),digest(r.raw.as_bytes()),!engine_mode]).map_err(err)?;
        }
        if !engine_mode {
            tx.execute(
                "UPDATE origins SET through=?2 WHERE origin=?1",
                params![origin, through],
            )
            .map_err(err)?;
        }
        tx.commit().map_err(err)?;
        self.recover_pending()?;
        Ok(through)
    }
    fn recover_pending(&mut self) -> Result<(), String> {
        let count: u64 = self
            .conn
            .query_row("SELECT count(*) FROM records WHERE published=0", [], |r| {
                r.get(0)
            })
            .map_err(err)?;
        if count == 0 {
            return Ok(());
        }
        if count > MAX_BATCH as u64 {
            return Err("corrupt pending batch bound".into());
        }
        let engine = self
            .engine
            .as_ref()
            .ok_or("pending engine batch without engine")?;
        let mut stmt = self
            .conn
            .prepare("SELECT id,origin,source_id,raw FROM records WHERE published=0 ORDER BY id")
            .map_err(err)?;
        let pending = stmt
            .query_map([], Self::row)
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        drop(stmt);
        if pending.iter().map(|r| r.raw.len()).sum::<usize>() > MAX_BATCH_BYTES {
            return Err("corrupt pending bytes".into());
        }
        engine.write_reserved(&pending)?;
        let keys = pending
            .iter()
            .map(|r| (r.origin.clone(), r.source_id))
            .collect::<Vec<_>>();
        let stored = engine.get_rows(&keys)?;
        if stored.len() != pending.len() || pending.iter().any(|p| !stored.iter().any(|s| s == p)) {
            return Err("engine publication readback mismatch".into());
        }
        let tx = self.conn.transaction().map_err(err)?;
        let mut prefixes: BTreeMap<String, u64> = BTreeMap::new();
        for r in &pending {
            let prefix = match prefixes.get(&r.origin) {
                Some(p) => *p,
                None => tx
                    .query_row(
                        "SELECT through FROM origins WHERE origin=?1",
                        [&r.origin],
                        |r| r.get(0),
                    )
                    .map_err(err)?,
            };
            let next = admission_kernel::handoff::advance(prefix, r.source_id)
                .ok_or("pending source prefix gap")?;
            prefixes.insert(r.origin.clone(), next);
            tx.execute(
                "UPDATE records SET published=1,raw=X'' WHERE id=?1 AND published=0",
                [r.canonical_id],
            )
            .map_err(err)?;
        }
        for (origin, through) in prefixes {
            tx.execute(
                "UPDATE origins SET through=?2 WHERE origin=?1",
                params![origin, through],
            )
            .map_err(err)?;
        }
        tx.commit().map_err(err)?;
        Ok(())
    }
    fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<CanonicalRow> {
        let raw = r.get_ref(3)?.as_blob()?;
        if raw.len() > observation::MAX_FRAME {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(CanonicalRow {
            canonical_id: r.get(0)?,
            origin: r.get(1)?,
            source_id: r.get(2)?,
            raw: String::from_utf8(raw.to_vec()).map_err(|_| rusqlite::Error::InvalidQuery)?,
        })
    }

    fn access(&self) -> Access<'_> {
        Access {
            conn: &self.conn,
            engine: self.engine.as_ref(),
        }
    }
    pub(crate) fn reader(&self) -> Result<Reader, String> {
        let conn = Connection::open_with_flags(
            self.dir.join("canonical.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(err)?;
        conn.busy_timeout(std::time::Duration::from_millis(100))
            .map_err(err)?;
        conn.execute_batch(
            "PRAGMA query_only=ON; PRAGMA cache_size=-512; PRAGMA temp_store=MEMORY;",
        )
        .map_err(err)?;
        Ok(Reader {
            conn,
            engine: self.engine.clone(),
        })
    }
    pub fn count(&self) -> Result<u64, String> {
        self.conn
            .query_row("SELECT count(*) FROM records WHERE published=1", [], |r| {
                r.get(0)
            })
            .map_err(err)
    }

    pub fn replay(
        &self,
        after: u64,
        limit: usize,
        byte_limit: usize,
    ) -> Result<Vec<CanonicalRow>, String> {
        self.access().replay(after, limit, byte_limit)
    }
    pub fn read(
        &self,
        origin: &str,
        after: u64,
        through: u64,
        limit: usize,
    ) -> Result<Vec<CanonicalRow>, String> {
        self.access().read(origin, after, through, limit)
    }
    pub fn lookup(
        &self,
        origin: &str,
        identity: &Identity,
    ) -> Result<Option<CanonicalRow>, String> {
        self.access().lookup(origin, identity)
    }
    pub fn evidence(&self, id: u64) -> Result<Option<CanonicalRow>, String> {
        self.access().evidence(id)
    }
}

/// Independent read-only SQLite connection: engine I/O never owns the writer.
pub(crate) struct Reader {
    conn: Connection,
    engine: Option<crate::engine::Greptime>,
}
impl Reader {
    fn access(&self) -> Access<'_> {
        Access {
            conn: &self.conn,
            engine: self.engine.as_ref(),
        }
    }

    pub fn read(
        &self,
        origin: &str,
        after: u64,
        through: u64,
        limit: usize,
    ) -> Result<Vec<CanonicalRow>, String> {
        self.access().read(origin, after, through, limit)
    }
    pub fn lookup(
        &self,
        origin: &str,
        identity: &Identity,
    ) -> Result<Option<CanonicalRow>, String> {
        self.access().lookup(origin, identity)
    }
    /// One bounded SQLite query; failures/byte exhaustion are per requested source.
    pub(crate) fn batch_read(
        &self,
        origin: &str,
        ids: &[u64],
        deadline: std::time::Instant,
    ) -> Result<BTreeMap<u64, Result<Option<CanonicalRow>, ()>>, String> {
        use rusqlite::types::Value;
        if self.engine.is_some() || ids.len() > 256 {
            return Err("unsupported batch read".into());
        }
        let mut result: BTreeMap<u64, Result<Option<CanonicalRow>, ()>> =
            ids.iter().map(|id| (*id, Err(()))).collect();
        if result.is_empty() {
            return Ok(result);
        }
        let placeholders = vec!["?"; result.len()].join(",");
        let sql = format!(
            "SELECT {COLUMNS} FROM records WHERE published=1 AND origin=? AND source_id IN ({placeholders}) ORDER BY source_id"
        );
        let mut values = vec![Value::Text(origin.to_owned())];
        values.extend(result.keys().map(|id| Value::Integer(*id as i64)));
        let mut stmt = self.conn.prepare(&sql).map_err(err)?;
        let mut query = stmt
            .query(rusqlite::params_from_iter(values))
            .map_err(err)?;
        let mut bytes = 0usize;
        let mut complete = false;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            if std::time::Instant::now() >= deadline {
                break;
            }
            let row = match query.next() {
                Ok(Some(row)) => row,
                Ok(None) => {
                    complete = true;
                    break;
                }
                Err(_) => break,
            };
            let id: u64 = match row.get(2) {
                Ok(id) => id,
                Err(_) => break,
            };
            seen.insert(id);
            let len: usize = match row.get(4) {
                Ok(len) => len,
                Err(_) => continue,
            };
            if len > observation::MAX_FRAME || bytes.saturating_add(len) > 4 * 1024 * 1024 {
                continue;
            }
            bytes += len;
            let value = match Ledger::row(row) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let hash: String = match row.get(5) {
                Ok(hash) => hash,
                Err(_) => continue,
            };
            if value.raw.len() == len && digest(value.raw.as_bytes()) == hash {
                result.insert(id, Ok(Some(value)));
            }
        }
        if complete {
            for (id, value) in &mut result {
                if !seen.contains(id) {
                    *value = Ok(None);
                }
            }
        }
        Ok(result)
    }
    pub fn evidence(&self, id: u64) -> Result<Option<CanonicalRow>, String> {
        self.access().evidence(id)
    }
}
struct Access<'a> {
    conn: &'a Connection,
    engine: Option<&'a crate::engine::Greptime>,
}
impl Access<'_> {
    fn rows<P: rusqlite::Params>(
        &self,
        sql: &str,
        args: P,
        byte_limit: usize,
    ) -> Result<Vec<CanonicalRow>, String> {
        let mut stmt = self.conn.prepare(sql).map_err(err)?;
        let mut rows = stmt.query(args).map_err(err)?;
        let mut selected = Vec::new();
        let mut bytes = 0;
        while let Some(r) = rows.next().map_err(err)? {
            let len: usize = r.get(4).map_err(err)?;
            if len > observation::MAX_FRAME {
                return Err("corrupt canonical payload size".into());
            }
            if bytes + len > byte_limit {
                if selected.is_empty() {
                    return Err("canonical read budget cannot fit next record".into());
                }
                break;
            }
            bytes += len;
            selected.push((
                Ledger::row(r).map_err(err)?,
                len,
                r.get::<_, String>(5).map_err(err)?,
            ));
        }
        drop(rows);
        drop(stmt);
        if let Some(engine) = &self.engine {
            if selected.len() > MAX_BATCH {
                return Err("engine materialization batch bound".into());
            }
            let keys = selected
                .iter()
                .map(|(r, _, _)| (r.origin.clone(), r.source_id))
                .collect::<Vec<_>>();
            let remote = engine.get_rows(&keys)?;
            if remote.len() != selected.len() {
                return Err("canonical engine history unavailable".into());
            }
            for (row, len, hash) in &mut selected {
                let raw = remote
                    .iter()
                    .find(|r| {
                        r.origin == row.origin
                            && r.source_id == row.source_id
                            && r.canonical_id == row.canonical_id
                    })
                    .ok_or("canonical engine identity mismatch")?;
                if raw.raw.len() != *len || digest(raw.raw.as_bytes()) != *hash {
                    return Err("canonical engine raw mismatch".into());
                }
                row.raw.clone_from(&raw.raw);
            }
        } else if selected
            .iter()
            .any(|(r, len, hash)| r.raw.len() != *len || digest(r.raw.as_bytes()) != *hash)
        {
            return Err("canonical raw mismatch".into());
        }
        Ok(selected.into_iter().map(|(r, _, _)| r).collect())
    }
    pub fn replay(
        &self,
        after: u64,
        limit: usize,
        byte_limit: usize,
    ) -> Result<Vec<CanonicalRow>, String> {
        let limit = limit.min(if self.engine.is_some() {
            MAX_BATCH
        } else {
            8192
        });
        self.rows(
            &format!(
                "SELECT {COLUMNS} FROM records WHERE published=1 AND id>?1 ORDER BY id LIMIT ?2"
            ),
            params![after, limit],
            byte_limit.min(if self.engine.is_some() {
                MAX_BATCH_BYTES
            } else {
                8 * 1024 * 1024
            }),
        )
    }
    pub fn read(
        &self,
        origin: &str,
        after: u64,
        through: u64,
        limit: usize,
    ) -> Result<Vec<CanonicalRow>, String> {
        if after > i64::MAX as u64 || through > i64::MAX as u64 {
            return Err("source ID limit".into());
        }
        let available: u64 = self
            .conn
            .query_row(
                "SELECT through FROM origins WHERE origin=?1",
                [origin],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?
            .unwrap_or(0);
        if after > through || through > available || limit == 0 {
            return Err("requested canonical read fence unavailable".into());
        }
        let out=self.rows(&format!("SELECT {COLUMNS} FROM records WHERE published=1 AND origin=?1 AND source_id>?2 AND source_id<=?3 ORDER BY source_id LIMIT ?4"),params![origin,after,through,limit.min(MAX_BATCH)],MAX_BATCH_BYTES)?;
        if out
            .iter()
            .any(|r| !admission_kernel::handoff::within(through, r.source_id))
        {
            return Err("snapshot violation".into());
        }
        Ok(out)
    }
    pub fn lookup(&self, origin: &str, i: &Identity) -> Result<Option<CanonicalRow>, String> {
        Ok(self.rows(&format!("SELECT {COLUMNS} FROM records WHERE published=1 AND origin=?1 AND producer=?2 AND generation=?3 AND sequence=?4"),params![origin,i.producer,i.generation,i.sequence.to_string()],observation::MAX_FRAME)?.pop())
    }
    pub fn evidence(&self, id: u64) -> Result<Option<CanonicalRow>, String> {
        Ok(self
            .rows(
                &format!("SELECT {COLUMNS} FROM records WHERE published=1 AND id=?1"),
                [id],
                observation::MAX_FRAME,
            )?
            .pop())
    }
}
