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
const RESERVE: u64 = 128 * 1024;
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
            fs::create_dir(dir).map_err(err)?;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(err)?;
            if let Some(p) = dir.parent() {
                File::open(p).and_then(|f| f.sync_all()).map_err(err)?;
            }
        }
        Err(e) => return Err(err(e)),
    }
    Ok(())
}
impl Ledger {
    pub fn open(dir: &Path, quota: u64) -> Result<Self, String> {
        Self::open_with_engine(dir, quota, None)
    }
    pub fn open_with_engine(
        dir: &Path,
        quota: u64,
        endpoint: Option<&str>,
    ) -> Result<Self, String> {
        Self::open_with_node_quotas(dir, quota, endpoint, BTreeMap::new())
    }
    pub(crate) fn open_with_node_quotas(
        dir: &Path,
        quota: u64,
        endpoint: Option<&str>,
        node_quota_bytes: BTreeMap<String, u64>,
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
            let engine = crate::engine::Greptime::open_with_cursor_key(endpoint, key)?;
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
        if self.node_quota_bytes.is_empty() {
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
        for origin in self.node_quota_bytes.keys() {
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
            let ids = retry_ids
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let rows = self.access().rows(
                &format!("SELECT {COLUMNS} FROM records WHERE published=1 AND origin=?1 AND source_id IN ({ids}) ORDER BY id"),
                [origin], MAX_BATCH_BYTES,
            )?;
            if rows.len() != retry_ids.len() {
                return Err("canonical retry history unavailable within byte budget".into());
            }
            rows.into_iter()
                .map(|r| (r.source_id, r))
                .collect::<BTreeMap<_, _>>()
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
        let quota = self.node_quota_bytes.get(origin).copied();
        let engine_mode = self.engine.is_some();
        let ceiling = if quota.is_some() {
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
                .map_err(err)?;
            let next = used
                .checked_add(bytes as u64)
                .filter(|&total| total <= limit)
                .ok_or("canonical node payload quota exhausted")?;
            tx.execute(
                "UPDATE node_quota_usage SET raw_bytes=?2, source_through=?3 WHERE origin=?1",
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
