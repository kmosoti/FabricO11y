//! One-owner SQLite custody with durable retry identities and source checkpoints.
use crate::observation::{self, Observation};
use hmac::{Hmac, Mac};
use rusqlite::{Connection, OptionalExtension, params};
use rustix::fs::{FlockOperation, flock};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::io::Read;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::time::{Duration, Instant};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

pub const MIN_QUOTA: u64 = 1024 * 1024;
const RESERVE: u64 = 128 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub id: u64,
    pub identity: observation::Identity,
    pub duplicate: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub source: String,
    pub position: String,
}
pub struct Store {
    conn: Connection,
    dir: PathBuf,
    quota: u64,
    admission_full: bool,
    connection_batch_armed: bool,
    connection_batch_active: bool,
    _lock: File,
    origin: String,
    lease_key: Vec<u8>,
    forwarding: Option<super::ForwardConfig>,
}
impl Store {
    pub fn open(dir: &Path, quota: u64) -> Result<Self, String> {
        if quota < MIN_QUOTA {
            return Err("quota must be at least 1 MiB".into());
        }
        let existed = match fs::symlink_metadata(dir) {
            Ok(m) => {
                if !m.is_dir()
                    || m.file_type().is_symlink()
                    || std::os::unix::fs::MetadataExt::uid(&m) != rustix::process::getuid().as_raw()
                    || m.permissions().mode() & 0o077 != 0
                {
                    return Err("storage must be a private owned directory, not a symlink".into());
                }
                true
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(err(e)),
        };
        if !existed {
            fs::create_dir(dir).map_err(err)?;
        }
        if !existed {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(err)?;
            if let Some(parent) = dir.parent() {
                File::open(parent).and_then(|f| f.sync_all()).map_err(err)?;
            }
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(dir.join("owner.lock"))
            .map_err(err)?;
        lock.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(err)?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive).map_err(err)?;
        let db_path = dir.join("custody.sqlite");
        let conn = Connection::open(&db_path).map_err(err)?;
        fs::set_permissions(&db_path, fs::Permissions::from_mode(0o600)).map_err(err)?;
        conn.busy_timeout(std::time::Duration::from_millis(100))
            .map_err(err)?;
        // DELETE journal and no auto-vacuum: quota accounts for worst-case rollback
        // journal plus the entire database. No unbounded WAL or temporary files.
        conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA; PRAGMA temp_store=MEMORY; PRAGMA cache_size=-256; PRAGMA locking_mode=EXCLUSIVE; PRAGMA auto_vacuum=NONE;
CREATE TABLE IF NOT EXISTS records(id INTEGER PRIMARY KEY,producer TEXT NOT NULL,generation TEXT NOT NULL,sequence TEXT NOT NULL,payload BLOB NOT NULL,UNIQUE(producer,generation,sequence));
CREATE TABLE IF NOT EXISTS checkpoints(source TEXT PRIMARY KEY,position TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS registries(name TEXT PRIMARY KEY,descriptor TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS custody_state(key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS generations(producer TEXT NOT NULL,generation TEXT NOT NULL,expires INTEGER NOT NULL,sequence TEXT NOT NULL,last_id INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(producer,generation));
CREATE TABLE IF NOT EXISTS legacy_retries(producer TEXT NOT NULL,generation TEXT NOT NULL,sequence TEXT NOT NULL,id INTEGER NOT NULL,PRIMARY KEY(producer,generation,sequence));
INSERT OR IGNORE INTO custody_state VALUES('highest',CAST((SELECT coalesce(max(id),0) FROM records) AS TEXT));
INSERT OR IGNORE INTO custody_state VALUES('forwarded','0');
INSERT OR IGNORE INTO custody_state SELECT 'reclaimed',value FROM custody_state WHERE key='forwarded';
INSERT OR IGNORE INTO custody_state VALUES('clock_floor','0');").map_err(err)?;
        let has_last_id:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('generations') WHERE name='last_id')",[],|r|r.get(0)).map_err(err)?;
        if !has_last_id {
            conn.execute(
                "ALTER TABLE generations ADD COLUMN last_id INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(err)?;
        }
        let page_size: u64 = conn
            .pragma_query_value(None, "page_size", |r| r.get(0))
            .map_err(err)?;
        let journal: String = conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .map_err(err)?;
        let sync: u64 = conn
            .pragma_query_value(None, "synchronous", |r| r.get(0))
            .map_err(err)?;
        if journal != "delete" || sync != 3 || !(512..=65536).contains(&page_size) {
            return Err("unsupported SQLite durability configuration".into());
        }
        let max_pages = (quota - RESERVE) / 2 / page_size;
        conn.pragma_update(None, "max_page_count", max_pages)
            .map_err(err)?;
        let pages: u64 = conn
            .pragma_query_value(None, "page_count", |r| r.get(0))
            .map_err(err)?;
        if pages > max_pages {
            return Err("existing database exceeds quota".into());
        }
        let descriptor: Option<String> = conn
            .query_row(
                "SELECT descriptor FROM registries WHERE name=?1",
                [observation::REGISTRY],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        if descriptor
            .as_deref()
            .is_some_and(|d| d != observation::DESCRIPTOR)
        {
            return Err("registry descriptor conflict".into());
        }
        conn.execute(
            "INSERT OR IGNORE INTO registries VALUES(?1,?2)",
            params![observation::REGISTRY, observation::DESCRIPTOR],
        )
        .map_err(err)?;
        let mut random = [0u8; 32];
        File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut random))
            .map_err(err)?;
        let origin = format!("spindle-{}", hex(&random[..16]));
        conn.execute(
            "INSERT OR IGNORE INTO custody_state VALUES('origin',?1)",
            [&origin],
        )
        .map_err(err)?;
        conn.execute(
            "INSERT OR IGNORE INTO custody_state VALUES('lease_key',?1)",
            [hex(&random)],
        )
        .map_err(err)?;
        let origin = conn
            .query_row(
                "SELECT value FROM custody_state WHERE key='origin'",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        let key: String = conn
            .query_row(
                "SELECT value FROM custody_state WHERE key='lease_key'",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        let lease_key = unhex(&key)?;
        if lease_key.len() != 32 {
            return Err("invalid persisted lease key".into());
        }
        retain_descriptor(&conn, observation::REGISTRY_V3, observation::DESCRIPTOR_V3)?;
        File::open(dir).and_then(|f| f.sync_all()).map_err(err)?;
        Ok(Self {
            conn,
            dir: dir.to_owned(),
            quota,
            admission_full: false,
            connection_batch_armed: false,
            connection_batch_active: false,
            _lock: lock,
            origin,
            lease_key,
            forwarding: None,
        })
    }
    // The daemon owns one synchronous connection batch, bounded by its existing
    // connection/frame budgets. RELEASE is the durability boundary; callers must
    // not publish prepared receipts until it succeeds. No remote I/O spans it.
    pub(crate) fn arm_connection_batch(&mut self) -> Result<(), String> {
        if self.connection_batch_active || !self.conn.is_autocommit() {
            return Err("custody connection batch already active".into());
        }
        self.connection_batch_armed = true;
        Ok(())
    }
    fn ensure_connection_batch(&mut self) -> Result<(), String> {
        if self.connection_batch_armed && !self.connection_batch_active {
            self.begin_connection_batch()?;
        }
        Ok(())
    }
    pub(crate) fn begin_connection_batch(&mut self) -> Result<(), String> {
        if self.connection_batch_active || !self.conn.is_autocommit() {
            return Err("custody connection batch already active".into());
        }
        self.conn
            .execute_batch("SAVEPOINT daemon_connections")
            .map_err(err)?;
        self.connection_batch_active = true;
        Ok(())
    }
    pub(crate) fn commit_connection_batch_if_active(&mut self) -> Result<bool, String> {
        self.connection_batch_armed = false;
        if !self.connection_batch_active {
            return Ok(false);
        }
        self.commit_connection_batch()?;
        Ok(true)
    }
    pub(crate) fn commit_connection_batch(&mut self) -> Result<(), String> {
        if !self.connection_batch_active {
            return Err("custody connection batch absent".into());
        }
        self.conn
            .execute_batch("RELEASE daemon_connections")
            .map_err(err)?;
        self.connection_batch_active = false;
        self.connection_batch_armed = false;
        Ok(())
    }
    pub(crate) fn rollback_connection_batch(&mut self) -> Result<(), String> {
        self.connection_batch_armed = false;
        if self.connection_batch_active {
            // SQLite can automatically roll back the whole transaction on I/O or
            // disk-full failures; do not mask that original failure with no-such-savepoint.
            if !self.conn.is_autocommit() {
                self.conn
                    .execute_batch("ROLLBACK TO daemon_connections; RELEASE daemon_connections")
                    .map_err(err)?;
            }
            self.connection_batch_active = false;
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn inject_batch_commit_failure(&self) {
        // Native deferred constraints fail at the outermost RELEASE, after each
        // inner submission has successfully released its own savepoint.
        self.conn.execute_batch("PRAGMA foreign_keys=ON;
CREATE TABLE commit_parent(id INTEGER PRIMARY KEY);
CREATE TABLE commit_child(id INTEGER REFERENCES commit_parent(id) DEFERRABLE INITIALLY DEFERRED);
CREATE TRIGGER fail_batch_commit AFTER INSERT ON records BEGIN INSERT INTO commit_child VALUES(1); END;").unwrap();
    }
    pub fn submit(
        &mut self,
        bytes: &[u8],
        checkpoint: Option<&Checkpoint>,
    ) -> Result<Receipt, String> {
        let observation = observation::decode_versioned(bytes)?;
        let is_v3 = observation.context().is_some();
        self.validate_generation(&observation)?;
        if let Some(cp) = checkpoint
            && (cp.source.is_empty() || cp.source.len() > 256 || cp.position.len() > 8192)
        {
            return Err("invalid checkpoint".into());
        }
        let i = observation.identity();
        let existing:Option<(u64,Vec<u8>)>=self.conn.query_row("SELECT id,payload FROM records WHERE producer=?1 AND generation=?2 AND sequence=?3",params![i.producer,i.generation,i.sequence.to_string()],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(err)?;
        if let Some((id, body)) = existing {
            if body != bytes {
                return Err("identity conflict".into());
            }
            // Checkpoints are daemon-owned; duplicate retries never move them.
            return Ok(Receipt {
                id,
                identity: i.clone(),
                duplicate: true,
            });
        }
        if is_v3 {
            let sequence: String = self
                .conn
                .query_row(
                    "SELECT sequence FROM generations WHERE producer=?1 AND generation=?2",
                    params![i.producer, i.generation],
                    |r| r.get(0),
                )
                .optional()
                .map_err(err)?
                .ok_or("unknown generation")?;
            let frontier: u64 = sequence.parse().map_err(err)?;
            if i.sequence <= frontier {
                return self.reclaimed_retry(i, bytes, None);
            }
            if admission_kernel::handoff::advance(frontier, i.sequence).is_none() {
                return Err("generation sequence gap".into());
            }
        } else {
            let reclaimed: Option<u64> = self.conn.query_row("SELECT id FROM legacy_retries WHERE producer=?1 AND generation=?2 AND sequence=?3",params![i.producer,i.generation,i.sequence.to_string()],|r|r.get(0)).optional().map_err(err)?;
            if let Some(id) = reclaimed {
                return self.reclaimed_retry(i, bytes, Some(id));
            }
        }
        if self.admission_full {
            return Err("storage full".into());
        }
        let page_size: u64 = self
            .conn
            .pragma_query_value(None, "page_size", |r| r.get(0))
            .map_err(err)?;
        let free_pages: u64 = self
            .conn
            .pragma_query_value(None, "freelist_count", |r| r.get(0))
            .map_err(err)?;
        let growth =
            (bytes.len() as u64 + 32768).saturating_sub(free_pages.saturating_mul(page_size));
        let pages: u64 = self
            .conn
            .pragma_query_value(None, "page_count", |r| r.get(0))
            .map_err(err)?;
        let physical_database_bytes = fs::metadata(self.dir.join("custody.sqlite"))
            .map_err(err)?
            .len();
        let cached_growth = pages
            .saturating_mul(page_size)
            .saturating_sub(physical_database_bytes);
        if self
            .disk_bytes()?
            .saturating_add(cached_growth)
            .saturating_add(growth * 2)
            .saturating_add(RESERVE)
            > self.quota
        {
            self.admission_full = true;
            return Err("storage full".into());
        }
        self.ensure_connection_batch()?;
        let tx = self.conn.savepoint().map_err(err)?;
        let previous: String = tx
            .query_row(
                "SELECT value FROM custody_state WHERE key='highest'",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        let id = previous
            .parse::<u64>()
            .map_err(err)?
            .checked_add(1)
            .filter(|id| *id <= i64::MAX as u64)
            .ok_or("record IDs exhausted")?;
        if let Err(error) = tx.execute(
            "INSERT INTO records(id,producer,generation,sequence,payload) VALUES(?1,?2,?3,?4,?5)",
            params![id, i.producer, i.generation, i.sequence.to_string(), bytes],
        ) {
            if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull) {
                self.admission_full = true;
            }
            return Err(err(error));
        }
        tx.execute(
            "UPDATE custody_state SET value=?1 WHERE key='highest'",
            [id.to_string()],
        )
        .map_err(err)?;
        if is_v3 {
            tx.execute(
                "UPDATE generations SET sequence=?1,last_id=?4 WHERE producer=?2 AND generation=?3",
                params![i.sequence.to_string(), i.producer, i.generation, id],
            )
            .map_err(err)?;
        }
        if let Some(cp) = checkpoint {
            tx.execute("INSERT INTO checkpoints VALUES(?1,?2) ON CONFLICT(source) DO UPDATE SET position=excluded.position",params![cp.source,cp.position]).map_err(err)?;
        }
        tx.commit().map_err(err)?;
        Ok(Receipt {
            id,
            identity: i.clone(),
            duplicate: false,
        })
    }
    pub fn checkpoint(&self, source: &str) -> Result<Option<String>, String> {
        self.conn
            .query_row(
                "SELECT position FROM checkpoints WHERE source=?1",
                [source],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)
    }
    pub fn highest_record_id(&self) -> Result<u64, String> {
        self.conn
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM custody_state WHERE key='highest'",
                [],
                |r| r.get(0),
            )
            .map_err(err)
    }
    pub fn count(&self) -> Result<u64, String> {
        self.conn
            .query_row("SELECT count(*) FROM records", [], |r| r.get(0))
            .map_err(err)
    }
    pub fn disk_bytes(&self) -> Result<u64, String> {
        let mut total = 0u64;
        for entry in fs::read_dir(&self.dir).map_err(err)? {
            total = total.saturating_add(entry.map_err(err)?.metadata().map_err(err)?.len())
        }
        Ok(total)
    }
    /// Original validated payload bytes; packed up to `MAX_READ_PAGE` records within `MAX_REPLY`.
    pub fn read_raw(&self, after: u64, limit: usize) -> Result<Vec<(u64, Vec<u8>)>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        if after < self.reclaimed()? {
            return self.read_canonical(after);
        }
        let capped = limit.min(super::wire::MAX_READ_PAGE);
        let mut stmt = self
            .conn
            .prepare("SELECT id,payload FROM records WHERE id>?1 ORDER BY id LIMIT ?2")
            .map_err(err)?;
        let rows = stmt
            .query_map(params![after, capped], |r| {
                let raw = r.get_ref(1)?.as_blob()?;
                if raw.len() > crate::observation::MAX_FRAME {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                Ok((r.get::<_, u64>(0)?, raw.to_vec()))
            })
            .map_err(err)?;
        let mut out = Vec::new();
        let mut budget_used = super::wire::RECORD_ENVELOPE_BYTES;
        for row in rows {
            let (id, bytes) = row.map_err(err)?;
            let entry_bytes = bytes
                .len()
                .saturating_add(super::wire::RECORD_ENTRY_OVERHEAD);
            if !out.is_empty() && budget_used.saturating_add(entry_bytes) > super::MAX_REPLY {
                break;
            }
            observation::decode_versioned(&bytes)?;
            budget_used = budget_used.saturating_add(entry_bytes);
            out.push((id, bytes));
        }
        Ok(out)
    }
    pub fn read(&self, after: u64, limit: usize) -> Result<Vec<(u64, Observation)>, String> {
        let mut records = Vec::new();
        let mut cursor = after;
        let max_total = limit.min(1000);
        while records.len() < max_total {
            let want = (max_total - records.len()).min(super::wire::MAX_READ_PAGE);
            let page = self.read_raw(cursor, want)?;
            if page.is_empty() {
                break;
            }
            for (id, bytes) in page {
                records.push((id, observation::decode(&bytes)?));
                cursor = id;
                if records.len() >= max_total {
                    break;
                }
            }
        }
        Ok(records)
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        let _ = flock(&self._lock, FlockOperation::Unlock);
    }
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bytes() -> Vec<u8> {
        br#"{"version":2,"registry":"fabrico11y/2","identity":{"producer":"fault","generation":"g","sequence":1},"timestamp_ns":1,"resource":"fixture","data":{"kind":"event","name":"fixture","message":"fault injection"}}"#.to_vec()
    }
    #[test]
    fn lease_clock_writes_only_on_advance_and_preserves_floor_after_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store");
        let mut store = Store::open(&path, MIN_QUOTA).unwrap();
        let initial_changes = store.conn.total_changes();
        assert_eq!(store.lease_clock_at(0).unwrap(), 0);
        assert_eq!(store.conn.total_changes(), initial_changes);
        assert_eq!(store.lease_clock_at(42).unwrap(), 42);
        assert_eq!(store.conn.total_changes(), initial_changes + 1);
        for _ in 0..1000 {
            assert_eq!(store.lease_clock_at(42).unwrap(), 42);
        }
        assert_eq!(store.conn.total_changes(), initial_changes + 1);
        drop(store);

        let mut store = Store::open(&path, MIN_QUOTA).unwrap();
        let reopened_changes = store.conn.total_changes();
        assert_eq!(store.lease_clock_at(42).unwrap(), 42);
        assert!(
            store
                .lease_clock_at(41)
                .unwrap_err()
                .contains("clock behind")
        );
        assert_eq!(store.conn.total_changes(), reopened_changes);
        assert_eq!(store.lease_clock_at(43).unwrap(), 43);
        assert_eq!(store.conn.total_changes(), reopened_changes + 1);
        drop(store);

        let mut store = Store::open(&path, MIN_QUOTA).unwrap();
        let reopened_changes = store.conn.total_changes();
        assert!(
            store
                .lease_clock_at(42)
                .unwrap_err()
                .contains("clock behind")
        );
        assert_eq!(store.lease_clock_at(43).unwrap(), 43);
        assert_eq!(store.conn.total_changes(), reopened_changes);
    }
    #[test]
    fn checkpoint_failure_rolls_back_record_and_retry_identity() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store");
        let mut store = Store::open(&path, MIN_QUOTA).unwrap();
        store.conn.execute_batch("CREATE TRIGGER reject_checkpoint BEFORE INSERT ON checkpoints BEGIN SELECT RAISE(ABORT,'injected checkpoint failure'); END;").unwrap();
        let cp = Checkpoint {
            source: "fault".into(),
            position: "uncommitted".into(),
        };
        assert!(store.submit(&bytes(), Some(&cp)).is_err());
        assert_eq!(store.count().unwrap(), 0);
        assert_eq!(store.checkpoint("fault").unwrap(), None);
        store
            .conn
            .execute_batch("DROP TRIGGER reject_checkpoint")
            .unwrap();
        assert!(!store.submit(&bytes(), Some(&cp)).unwrap().duplicate);
        assert_eq!(store.count().unwrap(), 1);
    }
    #[test]
    fn source_initial_checkpoint_failure_retries_cleanly() {
        use crate::spindle::sources::{FileConfig, FileFormat, FileSource};
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temp.path().join("store"), MIN_QUOTA).unwrap();
        let source_path = temp.path().join("source");
        fs::write(&source_path, "hello\n").unwrap();
        store.conn.execute_batch("CREATE TRIGGER reject_checkpoint BEFORE INSERT ON checkpoints BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        let mut source = FileSource::new(FileConfig {
            id: "fixture".into(),
            path: source_path,
            format: FileFormat::Text,
            replay: true,
        });
        source.poll(&mut store);
        assert!(source.status.errors > 0);
        assert_eq!(store.count().unwrap(), 0);
        assert_eq!(store.checkpoint("file:fixture").unwrap(), None);
        store
            .conn
            .execute_batch("DROP TRIGGER reject_checkpoint")
            .unwrap();
        source.poll(&mut store);
        assert_eq!(store.count().unwrap(), 2);
    }
    #[test]
    fn retained_descriptor_cannot_change_under_same_registry() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store");
        let store = Store::open(&path, MIN_QUOTA).unwrap();
        store
            .conn
            .execute("UPDATE registries SET descriptor='conflict'", [])
            .unwrap();
        drop(store);
        assert!(
            Store::open(&path, MIN_QUOTA)
                .err()
                .unwrap()
                .contains("descriptor conflict")
        );
    }
    #[test]
    fn directory_parent_must_exist_and_quota_configuration_is_checked() {
        let temp = tempfile::tempdir().unwrap();
        assert!(Store::open(&temp.path().join("missing").join("nested"), MIN_QUOTA).is_err());
        assert!(Store::open(&temp.path().join("store"), MIN_QUOTA - 1).is_err());
    }
}

fn retain_descriptor(conn: &Connection, name: &str, descriptor: &str) -> Result<(), String> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT descriptor FROM registries WHERE name=?1",
            [name],
            |r| r.get(0),
        )
        .optional()
        .map_err(err)?;
    if existing.as_deref().is_some_and(|value| value != descriptor) {
        return Err("registry descriptor conflict".into());
    }
    conn.execute(
        "INSERT OR IGNORE INTO registries VALUES(?1,?2)",
        params![name, descriptor],
    )
    .map_err(err)?;
    Ok(())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2)
        || !value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err("invalid hex".into());
    }
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).map_err(err))
        .collect()
}
impl Store {
    pub fn origin(&self) -> &str {
        &self.origin
    }
    pub fn configure_forwarding(
        &mut self,
        config: Option<super::ForwardConfig>,
    ) -> Result<(), String> {
        if config.is_none() && self.reclaimed()? > 0 {
            return Err("canonical access cannot be disabled after payload reclamation".into());
        }
        if let Some(config) = &config {
            config.validate()?;
            let existing: Option<String> = self
                .conn
                .query_row(
                    "SELECT value FROM custody_state WHERE key='authority'",
                    [],
                    |r| r.get(0),
                )
                .optional()
                .map_err(err)?;
            if existing
                .as_ref()
                .is_some_and(|authority| authority != &config.authority)
            {
                return Err("canonical authority cannot change after binding".into());
            }
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO custody_state VALUES('authority',?1)",
                    [&config.authority],
                )
                .map_err(err)?;
        }
        self.forwarding = config;
        Ok(())
    }
    fn lease_clock(&mut self) -> Result<u64, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(err)?
            .as_secs();
        self.lease_clock_at(now)
    }
    fn lease_clock_at(&mut self, now: u64) -> Result<u64, String> {
        let floor: String = self
            .conn
            .query_row(
                "SELECT value FROM custody_state WHERE key='clock_floor'",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        let floor = floor.parse::<u64>().map_err(err)?;
        if now < floor {
            return Err("clock behind persisted lease floor".into());
        }
        if now > floor {
            self.ensure_connection_batch()?;
            self.conn
                .execute(
                    "UPDATE custody_state SET value=?1 WHERE key='clock_floor'",
                    [now.to_string()],
                )
                .map_err(err)?;
        }
        Ok(now)
    }
    fn lease_mac(&self, producer: &str, prefix: &str) -> Hmac<Sha256> {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.lease_key).expect("HMAC accepts all key lengths");
        mac.update(b"FabricO11y generation lease v3\0");
        mac.update(self.origin.as_bytes());
        mac.update(&[0]);
        mac.update(producer.as_bytes());
        mac.update(&[0]);
        mac.update(prefix.as_bytes());
        mac
    }
    pub fn generation_capacity(&self) -> Result<super::GenerationCapacity, String> {
        let used = self
            .conn
            .query_row("SELECT count(*) FROM generations", [], |row| row.get(0))
            .map_err(err)?;
        Ok(super::GenerationCapacity { used, limit: 128 })
    }
    pub fn issue_lease(&mut self, producer: &str) -> Result<(String, u64), String> {
        if producer.is_empty()
            || producer.len() > 128
            || producer.contains('\0')
            || observation::reserved_producer(producer)
        {
            return Err("invalid lease producer".into());
        }
        let now = self.lease_clock()?;
        self.ensure_connection_batch()?;
        // Expiry and confirmed handoff jointly retire generation metadata; pending
        // generations continue to consume a bounded slot until their custody ends.
        let forwarded = self.forwarded()?;
        let candidates = {
            let mut statement = self
                .conn
                .prepare("SELECT producer,generation,expires,last_id FROM generations")
                .map_err(err)?;
            let rows = statement
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, u64>(2)?,
                        r.get::<_, u64>(3)?,
                    ))
                })
                .map_err(err)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(err)?
        };
        for (producer, generation, expires, admitted) in candidates {
            if admission_kernel::handoff::retire(expires <= now, admitted, forwarded) {
                self.conn
                    .execute(
                        "DELETE FROM generations WHERE producer=?1 AND generation=?2",
                        params![producer, generation],
                    )
                    .map_err(err)?;
            }
        }
        let capacity = self.generation_capacity()?;
        if capacity.used >= capacity.limit {
            return Err("active generation limit".into());
        }
        let expires = now.checked_add(86400).ok_or("lease expiry overflow")?;
        let mut nonce = [0u8; 16];
        File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut nonce))
            .map_err(err)?;
        let prefix = format!("g3.{}.{expires:016x}", hex(&nonce));
        let generation = format!(
            "{prefix}.{}",
            hex(&self.lease_mac(producer, &prefix).finalize().into_bytes())
        );
        self.conn
            .execute(
                "INSERT INTO generations(producer,generation,expires,sequence,last_id) VALUES(?1,?2,?3,'0',0)",
                params![producer, generation, expires],
            )
            .map_err(err)?;
        Ok((generation, expires))
    }
    /// Authentication is independent of time and retained generation metadata.
    /// Legacy v2 strings only denote leases when the full producer/origin-bound
    /// token authenticates; a reserved-looking spelling alone is insufficient.
    fn authenticated_lease_expiry(&self, producer: &str, generation: &str) -> Result<u64, String> {
        let parts: Vec<&str> = generation.split('.').collect();
        if parts.len() != 4
            || parts[0] != "g3"
            || parts[1].len() != 32
            || parts[2].len() != 16
            || parts[3].len() != 64
        {
            return Err("invalid authenticated generation".into());
        }
        unhex(parts[1])?;
        let signature = unhex(parts[3])?;
        let prefix = &generation[..generation.len() - 65];
        self.lease_mac(producer, prefix)
            .verify_slice(&signature)
            .map_err(|_| "generation authentication failed")?;
        u64::from_str_radix(parts[2], 16).map_err(err)
    }
    fn validate_generation(
        &mut self,
        record: &observation::StoredObservation,
    ) -> Result<(), String> {
        let identity = record.identity();
        if record.context().is_some() {
            self.validate_lease(&identity.producer, &identity.generation)
        } else if self
            .authenticated_lease_expiry(&identity.producer, &identity.generation)
            .is_ok()
        {
            Err("leased generation requires v3".into())
        } else {
            Ok(())
        }
    }
    fn validate_lease(&mut self, producer: &str, generation: &str) -> Result<(), String> {
        let expires = self.authenticated_lease_expiry(producer, generation)?;
        if self.lease_clock()? >= expires {
            return Err("generation expired".into());
        }
        Ok(())
    }
    pub fn forwarded(&self) -> Result<u64, String> {
        let value: String = self
            .conn
            .query_row(
                "SELECT value FROM custody_state WHERE key='forwarded'",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        value.parse().map_err(err)
    }
    pub fn pending_batch(&self) -> Result<Vec<crate::canonical::ForwardRecord>, String> {
        let after = self.forwarded()?;
        let mut statement = self
            .conn
            .prepare("SELECT id,payload FROM records WHERE id>?1 ORDER BY id LIMIT ?2")
            .map_err(err)?;
        let rows = statement
            .query_map(params![after, crate::canonical::MAX_BATCH as u64], |r| {
                Ok((r.get::<_, u64>(0)?, r.get::<_, Vec<u8>>(1)?))
            })
            .map_err(err)?;
        let mut batch = Vec::new();
        let mut size = 0;
        for row in rows {
            let (source_id, bytes) = row.map_err(err)?;
            if size + bytes.len() > crate::canonical::MAX_BATCH_BYTES {
                break;
            }
            observation::decode_versioned(&bytes)?;
            size += bytes.len();
            batch.push(crate::canonical::ForwardRecord {
                source_id,
                raw: String::from_utf8(bytes).map_err(err)?,
            });
        }
        Ok(batch)
    }
    pub fn acknowledge(
        &mut self,
        authority: &str,
        origin: &str,
        through: u64,
        sent_through: u64,
    ) -> Result<(), String> {
        if self.connection_batch_active || !self.conn.is_autocommit() {
            return Err("cannot acknowledge during custody transaction".into());
        }
        let expected = self.forwarding.as_ref().ok_or("forwarding disabled")?;
        if !admission_kernel::handoff::reclaimable(
            authority == expected.authority,
            origin == self.origin,
            through,
            through,
        ) || through > sent_through
            || sent_through > self.highest_record_id()?
        {
            return Err("mismatched canonical receipt".into());
        }
        let previous = self.forwarded()?;
        if through > previous {
            let admitted: u64 = self
                .conn
                .query_row(
                    "SELECT count(*) FROM records WHERE id>?1 AND id<=?2",
                    params![previous, through],
                    |r| r.get(0),
                )
                .map_err(err)?;
            if admitted > crate::canonical::MAX_BATCH as u64
                || u128::from(admitted) != u128::from(through) - u128::from(previous)
            {
                return Err("canonical acknowledgement exceeds offered contiguous batch".into());
            }
        }
        // Confirmation and the first bounded GC share one durability boundary.
        // Independent GC still drains later batches while forwarding waits.
        let admission_full = self.admission_full;
        self.conn
            .execute_batch("SAVEPOINT canonical_ack")
            .map_err(err)?;
        let result = (|| {
            if through > previous {
                self.conn
                    .execute(
                        "UPDATE custody_state SET value=?1 WHERE key='forwarded'",
                        [through.to_string()],
                    )
                    .map_err(err)?;
            }
            self.reclaim_confirmed()?;
            self.conn
                .execute_batch("RELEASE canonical_ack")
                .map_err(err)
        })();
        if let Err(error) = result {
            self.admission_full = admission_full;
            // SQLite may itself roll back on I/O/full failures. Otherwise undo
            // both the confirmation and any released inner GC savepoint.
            if !self.conn.is_autocommit()
                && let Err(rollback) = self
                    .conn
                    .execute_batch("ROLLBACK TO canonical_ack; RELEASE canonical_ack")
            {
                return Err(format!(
                    "{error}; acknowledgement rollback failed: {rollback}"
                ));
            }
            return Err(error);
        }
        Ok(())
    }
    /// Highest contiguous payload prefix actually reclaimed, independent of
    /// canonical confirmation during shadow collection.
    pub fn reclaimed(&self) -> Result<u64, String> {
        let value: String = self
            .conn
            .query_row(
                "SELECT value FROM custody_state WHERE key='reclaimed'",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        value.parse().map_err(err)
    }
    /// Reclaims at most one bounded batch, including previously confirmed shadow
    /// rows after an operator explicitly enables reclamation on restart.
    pub fn reclaim_confirmed(&mut self) -> Result<bool, String> {
        if self.connection_batch_active {
            return Err("cannot reclaim during custody connection batch".into());
        }
        if !self
            .forwarding
            .as_ref()
            .is_some_and(|config| config.reclaim_confirmed)
        {
            return Ok(false);
        }
        let confirmed = self.forwarded()?;
        let reclaimed = self.reclaimed()?;
        if confirmed <= reclaimed {
            return Ok(false);
        }
        // Outermost when invoked independently; nested under canonical_ack when
        // its first batch is reclaimed together with a validated receipt.
        self.ensure_connection_batch()?;
        let tx = self.conn.savepoint().map_err(err)?;
        let rows = {
            let mut statement=tx.prepare("SELECT id,producer,generation,sequence,payload FROM records WHERE id>?1 AND id<=?2 ORDER BY id LIMIT 32").map_err(err)?;
            let rows = statement
                .query_map(params![reclaimed, confirmed], |r| {
                    Ok((
                        r.get::<_, u64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Vec<u8>>(4)?,
                    ))
                })
                .map_err(err)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(err)?
        };
        let Some(last) = rows.last() else {
            return Err("confirmed local payload prefix missing before reclamation".into());
        };
        let through = last.0;
        if u128::from(through) - u128::from(reclaimed) != rows.len() as u128
            || !admission_kernel::handoff::reclaimable(true, true, confirmed, through)
        {
            return Err("noncontiguous reclamation batch".into());
        }
        let legacy = rows
            .into_iter()
            .filter_map(|(id, producer, generation, sequence, bytes)| {
                match observation::decode_versioned(&bytes) {
                    Ok(observation::StoredObservation::V2(_)) => {
                        Some(Ok((id, producer, generation, sequence)))
                    }
                    Ok(observation::StoredObservation::V3(_)) => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        // Free pages first so quota-full stores can retain legacy retry metadata
        // inside the same atomic transaction without growing the database.
        tx.execute(
            "DELETE FROM records WHERE id>?1 AND id<=?2",
            params![reclaimed, through],
        )
        .map_err(err)?;
        for (id, producer, generation, sequence) in legacy {
            tx.execute(
                "INSERT OR IGNORE INTO legacy_retries VALUES(?1,?2,?3,?4)",
                params![producer, generation, sequence, id],
            )
            .map_err(err)?;
        }
        tx.execute(
            "UPDATE custody_state SET value=?1 WHERE key='reclaimed'",
            [through.to_string()],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        self.admission_full = false;
        Ok(true)
    }

    fn exchange(
        &self,
        request: &crate::canonical::Request,
    ) -> Result<crate::canonical::Response, String> {
        let config = self
            .forwarding
            .as_ref()
            .ok_or("canonical payload unavailable: forwarding disabled")?;
        crate::canonical::exchange(
            &config.endpoint,
            &config.credential()?,
            request,
            Instant::now() + Duration::from_millis(500),
        )
    }
    fn read_canonical(&self, after: u64) -> Result<Vec<(u64, Vec<u8>)>, String> {
        match self.exchange(&crate::canonical::Request::Read {
            origin: self.origin.clone(),
            after,
            through: self.forwarded()?,
            limit: 1,
        })? {
            crate::canonical::Response::Rows { rows } => {
                let row = rows
                    .into_iter()
                    .next()
                    .ok_or("canonical payload unavailable: missing acknowledged record")?;
                if row.origin != self.origin || row.source_id != after + 1 {
                    return Err("canonical read prefix mismatch".into());
                }
                observation::decode_versioned(row.raw.as_bytes())?;
                Ok(vec![(row.source_id, row.raw.into_bytes())])
            }
            crate::canonical::Response::Error { error } => Err(error),
            _ => Err("unexpected canonical read response".into()),
        }
    }
    fn reclaimed_retry(
        &self,
        identity: &observation::Identity,
        bytes: &[u8],
        expected_id: Option<u64>,
    ) -> Result<Receipt, String> {
        match self.exchange(&crate::canonical::Request::Lookup {
            origin: self.origin.clone(),
            identity: identity.clone(),
        })? {
            crate::canonical::Response::Found { row: Some(row) }
                if row.origin == self.origin
                    && row.raw.as_bytes() == bytes
                    && expected_id.is_none_or(|id| id == row.source_id) =>
            {
                Ok(Receipt {
                    id: row.source_id,
                    identity: identity.clone(),
                    duplicate: true,
                })
            }
            crate::canonical::Response::Found { row: Some(_) } => Err("identity conflict".into()),
            crate::canonical::Response::Error { error } => Err(error),
            _ => Err("reclaimed retry unavailable".into()),
        }
    }
}

#[cfg(test)]
mod handoff_tests {
    use super::*;
    fn event(version: u32, generation: &str, sequence: u64) -> Vec<u8> {
        let mut value = serde_json::json!({"version":version,"registry":format!("fabrico11y/{version}"),"identity":{"producer":"lease-test","generation":generation,"sequence":sequence},"timestamp_ns":u64::MAX,"resource":"fixture","data":{"kind":"event","name":"test","message":"retained exact bytes"}});
        if version == 3 {
            value["context"] = serde_json::json!({"run_id":"f404b128-2c19-412f-bcd0-823f9c1f1b31"});
        }
        serde_json::to_vec(&value).unwrap()
    }
    fn forwarding(temp: &Path) -> super::super::ForwardConfig {
        let path = temp.join("credential");
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        fs::write(&path, "fixture-secret").unwrap();
        super::super::ForwardConfig {
            endpoint: "127.0.0.1:9".into(),
            credential_file: path,
            authority: "test-authority".into(),
            reclaim_confirmed: true,
        }
    }
    #[test]
    fn lease_authentication_order_expiry_and_pending_retention() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temp.path().join("store"), MIN_QUOTA).unwrap();
        let (generation, _) = store.issue_lease("lease-test").unwrap();
        assert_eq!(generation.len(), 117);
        assert!(
            store
                .submit(&event(3, &generation, 2), None)
                .unwrap_err()
                .contains("sequence gap")
        );
        let raw = event(3, &generation, 1);
        assert!(!store.submit(&raw, None).unwrap().duplicate);
        assert!(store.submit(&raw, None).unwrap().duplicate);
        let changed = String::from_utf8(raw.clone())
            .unwrap()
            .replace("retained exact bytes", "conflict");
        assert!(
            store
                .submit(changed.as_bytes(), None)
                .unwrap_err()
                .contains("conflict")
        );
        assert!(
            store
                .submit(&event(2, &generation, 2), None)
                .unwrap_err()
                .contains("requires v3")
        );
        assert!(
            store
                .submit(&event(3, &generation.replace("g3.", "g4."), 2), None)
                .is_err()
        );
        assert!(store.validate_lease("wrong-producer", &generation).is_err());
        let prefix = format!("g3.{}.0000000000000001", "0".repeat(32));
        let expired = format!(
            "{prefix}.{}",
            hex(&store
                .lease_mac("lease-test", &prefix)
                .finalize()
                .into_bytes())
        );
        assert!(
            store
                .submit(&event(3, &expired, 1), None)
                .unwrap_err()
                .contains("expired")
        );
        store.conn.execute("DELETE FROM generations", []).unwrap();
        assert!(
            store
                .submit(&event(3, &expired, 1), None)
                .unwrap_err()
                .contains("expired")
        );
        assert_eq!(store.pending_batch().unwrap()[0].raw.as_bytes(), raw);
        assert_eq!(store.count().unwrap(), 1);
    }
    #[test]
    fn bound_generation_metadata_and_persist_clock_floor() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store");
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        for _ in 0..128 {
            store.issue_lease("lease-test").unwrap();
        }
        assert!(
            store
                .issue_lease("lease-test")
                .unwrap_err()
                .contains("generation limit")
        );
        let future = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        store
            .conn
            .execute(
                "UPDATE custody_state SET value=?1 WHERE key='clock_floor'",
                [future.to_string()],
            )
            .unwrap();
        drop(store);
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        assert!(
            store
                .issue_lease("lease-test")
                .unwrap_err()
                .contains("clock behind")
        );
    }
    #[test]
    fn receipt_validation_reclamation_and_ids_survive_empty_restart() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store");
        let config = forwarding(temp.path());
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        store.configure_forwarding(Some(config.clone())).unwrap();
        let origin = store.origin().to_owned();
        let raw = event(2, "legacy", 1);
        assert_eq!(store.submit(&raw, None).unwrap().id, 1);
        for (authority, node, through, sent) in [
            ("wrong", origin.as_str(), 1, 1),
            ("test-authority", "wrong", 1, 1),
            ("test-authority", origin.as_str(), 2, 1),
        ] {
            assert!(store.acknowledge(authority, node, through, sent).is_err());
            assert_eq!(store.count().unwrap(), 1);
        }
        store.acknowledge("test-authority", &origin, 1, 1).unwrap();
        assert_eq!(store.count().unwrap(), 0);
        assert_eq!(store.highest_record_id().unwrap(), 1);
        assert!(
            store.read_raw(0, 1).is_err(),
            "missing canonical data must not report EOF"
        );
        assert!(
            store.submit(&raw, None).is_err(),
            "reclaimed retry needs canonical byte verification"
        );
        drop(store);
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        assert_eq!(store.origin(), origin);
        assert_eq!(store.highest_record_id().unwrap(), 1);
        assert_eq!(store.submit(&event(2, "legacy", 2), None).unwrap().id, 2);
        let mut wrong = config;
        wrong.authority = "other".into();
        assert!(store.configure_forwarding(Some(wrong)).is_err());
    }
    #[test]
    fn migration_preserves_original_descriptor_origin_and_legacy_records() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let conn = Connection::open(path.join("custody.sqlite")).unwrap();
        conn.execute_batch("CREATE TABLE records(id INTEGER PRIMARY KEY,producer TEXT NOT NULL,generation TEXT NOT NULL,sequence TEXT NOT NULL,payload BLOB NOT NULL,UNIQUE(producer,generation,sequence));").unwrap();
        let raw = event(2, "old", 1);
        conn.execute(
            "INSERT INTO records VALUES(41,'lease-test','old','1',?1)",
            [&raw],
        )
        .unwrap();
        drop(conn);
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        assert_eq!(store.highest_record_id().unwrap(), 41);
        assert!(store.submit(&raw, None).unwrap().duplicate);
        assert_eq!(store.submit(&event(2, "old", 2), None).unwrap().id, 42);
        assert_eq!(store.read_raw(0, 1).unwrap()[0].1, raw);
        let origin = store.origin().to_owned();
        let (lease, _) = store.issue_lease("lease-test").unwrap();
        drop(store);
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        assert_eq!(store.origin(), origin);
        store.validate_lease("lease-test", &lease).unwrap();
    }
}

/// Owned bounded remote resolution work; never borrows the SQLite owner.
pub struct RemoteJob {
    config: super::ForwardConfig,
    origin: String,
    work: RemoteWork,
}
enum RemoteWork {
    Read {
        after: u64,
        through: u64,
    },
    Retry {
        identity: observation::Identity,
        bytes: Vec<u8>,
        expected_id: Option<u64>,
    },
}
impl RemoteJob {
    pub fn execute(self, deadline: Instant) -> Result<Vec<u8>, String> {
        let request = match &self.work {
            RemoteWork::Read { after, through } => crate::canonical::Request::Read {
                origin: self.origin.clone(),
                after: *after,
                through: *through,
                limit: 1,
            },
            RemoteWork::Retry { identity, .. } => crate::canonical::Request::Lookup {
                origin: self.origin.clone(),
                identity: identity.clone(),
            },
        };
        let response = crate::canonical::exchange(
            &self.config.endpoint,
            &self.config.credential()?,
            &request,
            deadline,
        )?;
        match (self.work, response) {
            (RemoteWork::Read { after, .. }, crate::canonical::Response::Rows { rows }) => {
                let row = rows
                    .into_iter()
                    .next()
                    .ok_or("canonical payload unavailable: missing acknowledged record")?;
                if row.origin != self.origin || row.source_id != after + 1 {
                    return Err("canonical read prefix mismatch".into());
                }
                super::wire::encode_records(&[(row.source_id, row.raw.into_bytes())])
            }
            (
                RemoteWork::Retry {
                    identity,
                    bytes,
                    expected_id,
                },
                crate::canonical::Response::Found { row: Some(row) },
            ) => {
                if row.origin != self.origin
                    || row.raw.as_bytes() != bytes
                    || expected_id.is_some_and(|id| id != row.source_id)
                {
                    return Err("identity conflict".into());
                }
                serde_json::to_vec(&super::Reply::Receipt {
                    receipt: Receipt {
                        id: row.source_id,
                        identity,
                        duplicate: true,
                    },
                })
                .map_err(err)
            }
            (_, crate::canonical::Response::Error { error }) => Err(error),
            _ => Err("canonical payload unavailable".into()),
        }
    }
}
impl Store {
    fn remote_job(&self, work: RemoteWork) -> Result<RemoteJob, String> {
        Ok(RemoteJob {
            config: self
                .forwarding
                .clone()
                .ok_or("canonical payload unavailable: forwarding disabled")?,
            origin: self.origin.clone(),
            work,
        })
    }
    pub fn prepare_remote_read(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<Option<RemoteJob>, String> {
        if limit == 0 || after >= self.reclaimed()? {
            return Ok(None);
        }
        self.remote_job(RemoteWork::Read {
            after,
            through: self.forwarded()?,
        })
        .map(Some)
    }
    pub fn prepare_remote_retry(&mut self, bytes: &[u8]) -> Result<Option<RemoteJob>, String> {
        let record = observation::decode_versioned(bytes)?;
        let identity = record.identity();
        self.validate_generation(&record)?;
        let present:bool=self.conn.query_row("SELECT EXISTS(SELECT 1 FROM records WHERE producer=?1 AND generation=?2 AND sequence=?3)",params![identity.producer,identity.generation,identity.sequence.to_string()],|r|r.get(0)).map_err(err)?;
        if present {
            return Ok(None);
        }
        if record.context().is_some() {
            let frontier: Option<String> = self
                .conn
                .query_row(
                    "SELECT sequence FROM generations WHERE producer=?1 AND generation=?2",
                    params![identity.producer, identity.generation],
                    |r| r.get(0),
                )
                .optional()
                .map_err(err)?;
            if frontier
                .map(|s| s.parse::<u64>().map_err(err))
                .transpose()?
                .is_some_and(|s| identity.sequence <= s)
            {
                return self
                    .remote_job(RemoteWork::Retry {
                        identity: identity.clone(),
                        bytes: bytes.to_vec(),
                        expected_id: None,
                    })
                    .map(Some);
            }
        } else {
            let id:Option<u64>=self.conn.query_row("SELECT id FROM legacy_retries WHERE producer=?1 AND generation=?2 AND sequence=?3",params![identity.producer,identity.generation,identity.sequence.to_string()],|r|r.get(0)).optional().map_err(err)?;
            if let Some(id) = id {
                return self
                    .remote_job(RemoteWork::Retry {
                        identity: identity.clone(),
                        bytes: bytes.to_vec(),
                        expected_id: Some(id),
                    })
                    .map(Some);
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod retirement_tests {
    use super::*;
    #[test]
    fn expired_generation_metadata_survives_until_all_payloads_are_forwarded() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store");
        let credential = temp.path().join("key");
        fs::write(&credential, "test").unwrap();
        fs::set_permissions(&credential, fs::Permissions::from_mode(0o600)).unwrap();
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        store
            .configure_forwarding(Some(super::super::ForwardConfig {
                endpoint: "127.0.0.1:9".into(),
                credential_file: credential,
                authority: "authority".into(),
                reclaim_confirmed: true,
            }))
            .unwrap();
        let (generation, _) = store.issue_lease("pending").unwrap();
        let raw=serde_json::to_vec(&serde_json::json!({"version":3,"registry":"fabrico11y/3","context":{"run_id":"f404b128-2c19-412f-bcd0-823f9c1f1b31"},"identity":{"producer":"pending","generation":generation,"sequence":1},"timestamp_ns":1,"resource":"fixture","data":{"kind":"event","name":"test","message":"pending"}})).unwrap();
        store.submit(&raw, None).unwrap();
        store
            .conn
            .execute(
                "UPDATE generations SET expires=1 WHERE generation=?1",
                [&generation],
            )
            .unwrap();
        store.issue_lease("other").unwrap();
        let retained: bool = store
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM generations WHERE generation=?1)",
                [&generation],
                |r| r.get(0),
            )
            .unwrap();
        assert!(retained);
        let origin = store.origin().to_owned();
        store.acknowledge("authority", &origin, 1, 1).unwrap();
        store.issue_lease("other").unwrap();
        let retained: bool = store
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM generations WHERE generation=?1)",
                [&generation],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!retained);
    }
}

#[cfg(test)]
mod shadow_catchup_tests {
    use super::*;
    #[test]
    fn confirmed_shadow_backlog_reclaims_in_bounded_batches_and_old_gc_state_migrates() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store");
        let key = temp.path().join("key");
        fs::write(&key, "fixture").unwrap();
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
        let mut config = super::super::ForwardConfig {
            endpoint: "127.0.0.1:9".into(),
            credential_file: key,
            authority: "authority".into(),
            reclaim_confirmed: false,
        };
        let bytes = |sequence: u64| {
            serde_json::to_vec(&serde_json::json!({"version":2,"registry":"fabrico11y/2","identity":{"producer":"shadow","generation":"g","sequence":sequence},"timestamp_ns":1,"resource":"fixture","data":{"kind":"event","name":"test","message":"shadow"}})).unwrap()
        };
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        store.configure_forwarding(Some(config.clone())).unwrap();
        for sequence in 1..=64 {
            assert_eq!(store.submit(&bytes(sequence), None).unwrap().id, sequence);
        }
        let origin = store.origin().to_owned();
        store.acknowledge("authority", &origin, 32, 32).unwrap();
        store.acknowledge("authority", &origin, 64, 64).unwrap();
        assert_eq!(store.count().unwrap(), 64);
        assert_eq!(store.forwarded().unwrap(), 64);
        assert_eq!(store.reclaimed().unwrap(), 0);
        drop(store);
        config.reclaim_confirmed = true;
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        store.configure_forwarding(Some(config.clone())).unwrap();
        assert!(store.pending_batch().unwrap().is_empty());
        assert!(store.reclaim_confirmed().unwrap());
        assert_eq!(store.reclaimed().unwrap(), 32);
        assert_eq!(store.count().unwrap(), 32);
        assert!(store.reclaim_confirmed().unwrap());
        assert_eq!(store.reclaimed().unwrap(), 64);
        assert_eq!(store.count().unwrap(), 0);
        assert!(!store.reclaim_confirmed().unwrap());
        store
            .conn
            .execute("DELETE FROM custody_state WHERE key='reclaimed'", [])
            .unwrap();
        drop(store);
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        assert_eq!(store.reclaimed().unwrap(), 64);
        assert_eq!(store.highest_record_id().unwrap(), 64);
        assert!(store.configure_forwarding(None).is_err());
        store.configure_forwarding(Some(config)).unwrap();
        assert_eq!(store.submit(&bytes(65), None).unwrap().id, 65);
    }
}

#[cfg(test)]
mod legacy_generation_compatibility_tests {
    use super::*;
    fn legacy(producer: &str, generation: &str, sequence: u64) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"version":2,"registry":"fabrico11y/2","identity":{"producer":producer,"generation":generation,"sequence":sequence},"timestamp_ns":1,"resource":"fixture","data":{"kind":"event","name":"legacy","message":"original v2 generation spelling"}})).unwrap()
    }
    #[test]
    fn historical_g3_spellings_and_unauthenticated_lookalikes_roundtrip_after_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store");
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        let (authentic, _) = store.issue_lease("legacy").unwrap();
        let mut invalid_mac = authentic.clone().into_bytes();
        let last = invalid_mac.last_mut().unwrap();
        *last = if *last == b'0' { b'1' } else { b'0' };
        let invalid_mac = String::from_utf8(invalid_mac).unwrap();
        let lookalikes = vec![
            "g3.legacy".to_owned(),
            "g3.".to_owned(),
            "g3.historical.extra.parts".to_owned(),
            format!(
                "g3.{}.{}.{}",
                "z".repeat(32),
                "0".repeat(16),
                "0".repeat(64)
            ),
            format!(
                "g3.{}.{}.{}",
                "0".repeat(32),
                "x".repeat(16),
                "0".repeat(64)
            ),
            invalid_mac,
        ];
        let mut accepted = Vec::new();
        for (index, generation) in lookalikes.into_iter().enumerate() {
            let bytes = legacy("legacy", &generation, u64::MAX - index as u64);
            assert!(store.prepare_remote_retry(&bytes).unwrap().is_none());
            let receipt = store.submit(&bytes, None).unwrap();
            assert!(!receipt.duplicate);
            assert_eq!(receipt.id, index as u64 + 1);
            assert!(store.submit(&bytes, None).unwrap().duplicate);
            accepted.push((receipt.id, bytes));
        }
        // Authentication is producer-bound: an otherwise genuine token spelling
        // remains an ordinary historical generation in an unrelated v2 producer.
        let unrelated = legacy("different-producer", &authentic, 1);
        assert!(store.prepare_remote_retry(&unrelated).unwrap().is_none());
        let receipt = store.submit(&unrelated, None).unwrap();
        accepted.push((receipt.id, unrelated));
        drop(store);
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        for (id, bytes) in accepted {
            assert!(store.prepare_remote_retry(&bytes).unwrap().is_none());
            let receipt = store.submit(&bytes, None).unwrap();
            assert!(receipt.duplicate);
            assert_eq!(receipt.id, id);
            assert_eq!(store.read_raw(id - 1, 1).unwrap()[0], (id, bytes));
        }
    }
    #[test]
    fn authentic_expired_pruned_lease_cannot_be_downgraded_to_v2_after_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store");
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        let (current, _) = store.issue_lease("leased").unwrap();
        let current_v2 = legacy("leased", &current, 1);
        assert!(
            store
                .submit(&current_v2, None)
                .unwrap_err()
                .contains("requires v3")
        );
        assert!(
            store
                .prepare_remote_retry(&current_v2)
                .err()
                .unwrap()
                .contains("requires v3")
        );
        // A historical issuance fixture uses this origin's actual private HMAC
        // key, then deletes its generation row. No time/metadata lookup may turn
        // this authentic expired lease into an unleased legacy generation.
        let prefix = format!("g3.{}.0000000000000001", "0".repeat(32));
        let expired = format!(
            "{prefix}.{}",
            hex(&store.lease_mac("leased", &prefix).finalize().into_bytes())
        );
        store.conn.execute("INSERT INTO generations(producer,generation,expires,sequence,last_id) VALUES('leased',?1,1,'0',0)",[&expired]).unwrap();
        assert_eq!(
            store
                .authenticated_lease_expiry("leased", &expired)
                .unwrap(),
            1
        );
        store.conn.execute("DELETE FROM generations", []).unwrap();
        let expired_v2 = legacy("leased", &expired, 1);
        assert!(
            store
                .submit(&expired_v2, None)
                .unwrap_err()
                .contains("requires v3")
        );
        assert!(
            store
                .prepare_remote_retry(&expired_v2)
                .err()
                .unwrap()
                .contains("requires v3")
        );
        drop(store);
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        assert_eq!(
            store
                .authenticated_lease_expiry("leased", &expired)
                .unwrap(),
            1
        );
        assert!(
            store
                .submit(&expired_v2, None)
                .unwrap_err()
                .contains("requires v3")
        );
        assert!(
            store
                .prepare_remote_retry(&expired_v2)
                .err()
                .unwrap()
                .contains("requires v3")
        );
        assert_eq!(store.count().unwrap(), 0);
    }
}

#[cfg(test)]
mod connection_batch_tests {
    use super::*;
    fn raw(sequence: u64) -> Vec<u8> {
        format!(r#"{{"version":2,"registry":"fabrico11y/2","identity":{{"producer":"batch","generation":"legacy","sequence":{sequence}}},"timestamp_ns":1,"resource":"fixture","data":{{"kind":"event","name":"batch","message":"exact bytes"}}}}"#).into_bytes()
    }
    #[test]
    fn peers_commit_in_order_while_failed_inner_submission_is_rolled_back() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store");
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        store.conn.execute_batch("CREATE TRIGGER reject_batch_checkpoint BEFORE INSERT ON checkpoints WHEN NEW.source='reject' BEGIN SELECT RAISE(ABORT,'injected checkpoint rejection'); END;").unwrap();
        store.begin_connection_batch().unwrap();
        assert!(store.begin_connection_batch().is_err());
        let first = store.submit(&raw(1), None).unwrap();
        let checkpoint = Checkpoint {
            source: "reject".into(),
            position: "uncommitted".into(),
        };
        assert!(store.submit(&raw(2), Some(&checkpoint)).is_err());
        assert_eq!(store.count().unwrap(), 1);
        assert_eq!(store.highest_record_id().unwrap(), first.id);
        assert!(store.checkpoint("reject").unwrap().is_none());
        let second = store.submit(&raw(3), None).unwrap();
        assert_eq!((first.id, second.id), (1, 2));
        store.commit_connection_batch().unwrap();
        drop(store);
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        assert_eq!(store.count().unwrap(), 2);
        assert_eq!(store.read_raw(0, 1).unwrap()[0], (1, raw(1)));
        assert_eq!(store.read_raw(1, 1).unwrap()[0], (2, raw(3)));
        assert!(store.submit(&raw(1), None).unwrap().duplicate);
        assert_eq!(store.submit(&raw(4), None).unwrap().id, 3);
    }
    #[test]
    fn outer_rollback_discards_records_checkpoints_and_unpublished_ids_after_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store");
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        assert_eq!(store.submit(&raw(1), None).unwrap().id, 1);
        store.begin_connection_batch().unwrap();
        let checkpoint = Checkpoint {
            source: "batch".into(),
            position: "uncommitted".into(),
        };
        assert_eq!(store.submit(&raw(2), Some(&checkpoint)).unwrap().id, 2);
        assert_eq!(store.submit(&raw(3), None).unwrap().id, 3);
        store.rollback_connection_batch().unwrap();
        drop(store);
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        assert_eq!(store.count().unwrap(), 1);
        assert_eq!(store.highest_record_id().unwrap(), 1);
        assert!(store.checkpoint("batch").unwrap().is_none());
        assert_eq!(store.submit(&raw(2), None).unwrap().id, 2);
    }
    #[test]
    fn native_outer_commit_failure_rolls_back_successful_inner_releases() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store");
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        store.inject_batch_commit_failure();
        store.begin_connection_batch().unwrap();
        assert_eq!(store.submit(&raw(1), None).unwrap().id, 1);
        assert_eq!(store.submit(&raw(2), None).unwrap().id, 2);
        assert!(
            store
                .commit_connection_batch()
                .unwrap_err()
                .contains("FOREIGN KEY")
        );
        store.rollback_connection_batch().unwrap();
        assert!(store.conn.is_autocommit());
        drop(store);
        let store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        assert_eq!(store.count().unwrap(), 0);
        assert_eq!(store.highest_record_id().unwrap(), 0);
    }
}

#[cfg(test)]
mod forwarding_batch_bounds_tests {
    use super::*;
    fn raw(sequence: u64, message: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"version":2,"registry":"fabrico11y/2","identity":{"producer":"batch-bounds","generation":"legacy","sequence":sequence},"timestamp_ns":1,"resource":"fixture","data":{"kind":"event","name":"batch","message":message}})).unwrap()
    }
    fn configured(root: &Path) -> Store {
        let key = root.join("credential");
        fs::write(&key, "fixture-key").unwrap();
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
        let mut store = Store::open(&root.join("store"), 2 * MIN_QUOTA).unwrap();
        store
            .configure_forwarding(Some(super::super::ForwardConfig {
                endpoint: "127.0.0.1:9".into(),
                credential_file: key,
                authority: "authority".into(),
                reclaim_confirmed: true,
            }))
            .unwrap();
        store
    }
    #[test]
    fn forwarding_offers_512_contiguous_records_and_acknowledgement_reclaims_only_32() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = configured(temp.path());
        store.begin_connection_batch().unwrap();
        for sequence in 1..=513 {
            assert_eq!(
                store.submit(&raw(sequence, "small"), None).unwrap().id,
                sequence
            );
        }
        store.commit_connection_batch().unwrap();
        let offered = store.pending_batch().unwrap();
        assert_eq!(offered.len(), 512);
        for (index, record) in offered.iter().enumerate() {
            let expected = index as u64 + 1;
            assert_eq!(record.source_id, expected);
            assert_eq!(record.raw.as_bytes(), raw(expected, "small"));
        }
        let origin = store.origin().to_owned();
        assert!(store.acknowledge("authority", &origin, 513, 512).is_err());
        assert!(store.acknowledge("authority", &origin, 513, 513).is_err());
        assert_eq!(store.forwarded().unwrap(), 0);
        assert_eq!(store.reclaimed().unwrap(), 0);
        store.acknowledge("authority", &origin, 512, 512).unwrap();
        assert_eq!(store.forwarded().unwrap(), 512);
        assert_eq!(store.reclaimed().unwrap(), 32);
        assert_eq!(store.count().unwrap(), 481);
        let next = store.pending_batch().unwrap();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].source_id, 513);
        assert!(store.reclaim_confirmed().unwrap());
        assert_eq!(store.reclaimed().unwrap(), 64);
        assert_eq!(store.read_raw(512, 1).unwrap()[0], (513, raw(513, "small")));
    }
    #[test]
    fn decoded_byte_cap_stops_before_count_cap_and_receipt_cannot_exceed_offer() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = configured(temp.path());
        let message = "x".repeat(16 * 1024);
        store.begin_connection_batch().unwrap();
        for sequence in 1..=20 {
            store.submit(&raw(sequence, &message), None).unwrap();
        }
        store.commit_connection_batch().unwrap();
        let offered = store.pending_batch().unwrap();
        assert!(!offered.is_empty() && offered.len() < 20);
        let bytes: usize = offered.iter().map(|record| record.raw.len()).sum();
        assert!(bytes <= 256 * 1024);
        let through = offered.last().unwrap().source_id;
        assert!(bytes + raw(through + 1, &message).len() > 256 * 1024);
        for (index, record) in offered.iter().enumerate() {
            let expected = index as u64 + 1;
            assert_eq!(record.source_id, expected);
            assert_eq!(record.raw.as_bytes(), raw(expected, &message));
        }
        let origin = store.origin().to_owned();
        assert!(
            store
                .acknowledge("authority", &origin, through + 1, through)
                .is_err()
        );
        assert_eq!(store.forwarded().unwrap(), 0);
        store
            .acknowledge("authority", &origin, through, through)
            .unwrap();
        assert_eq!(store.forwarded().unwrap(), through);
        assert_eq!(store.reclaimed().unwrap(), through);
        let next = store.pending_batch().unwrap();
        assert_eq!(next[0].source_id, through + 1);
        assert_eq!(next.last().unwrap().source_id, 20);
    }
}

#[cfg(test)]
mod atomic_ack_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn raw(sequence: u64) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"version":2,"registry":"fabrico11y/2","identity":{"producer":"atomic-ack","generation":"legacy","sequence":sequence},"timestamp_ns":1,"resource":"fixture","data":{"kind":"event","name":"ack","message":"original bytes"}})).unwrap()
    }
    fn configured(root: &Path) -> Store {
        let key = root.join("credential");
        fs::write(&key, "fixture-key").unwrap();
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
        let mut store = Store::open(&root.join("store"), 2 * MIN_QUOTA).unwrap();
        store
            .configure_forwarding(Some(super::super::ForwardConfig {
                endpoint: "127.0.0.1:9".into(),
                credential_file: key,
                authority: "authority".into(),
                reclaim_confirmed: true,
            }))
            .unwrap();
        store.begin_connection_batch().unwrap();
        for sequence in 1..=40 {
            store.submit(&raw(sequence), None).unwrap();
        }
        store.commit_connection_batch().unwrap();
        store
    }
    unsafe extern "C" fn committed(context: *mut std::ffi::c_void) -> std::ffi::c_int {
        // The test declares this counter before Store and unregisters the hook;
        // its address remains live through every possible connection callback.
        unsafe { &*context.cast::<AtomicUsize>() }.fetch_add(1, Ordering::SeqCst);
        0
    }
    #[test]
    fn ack_and_first_gc_have_one_durable_commit() {
        let commits = AtomicUsize::new(0);
        let temp = tempfile::tempdir().unwrap();
        let mut store = configured(temp.path());
        assert_eq!(
            store
                .conn
                .pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))
                .unwrap(),
            "delete"
        );
        assert_eq!(
            store
                .conn
                .pragma_query_value(None, "synchronous", |r| r.get::<_, u32>(0))
                .unwrap(),
            3
        );
        unsafe {
            rusqlite::ffi::sqlite3_commit_hook(
                store.conn.handle(),
                Some(committed),
                (&commits as *const AtomicUsize).cast_mut().cast(),
            );
        }
        let origin = store.origin().to_owned();
        store.acknowledge("authority", &origin, 40, 40).unwrap();
        unsafe {
            rusqlite::ffi::sqlite3_commit_hook(store.conn.handle(), None, std::ptr::null_mut());
        }
        assert_eq!(store.forwarded().unwrap(), 40);
        assert_eq!(store.reclaimed().unwrap(), 32);
        assert_eq!(store.count().unwrap(), 8);
        assert_eq!(
            commits.load(Ordering::SeqCst),
            1,
            "ACK and first bounded GC must share one native SQLite commit"
        );
    }

    fn failed_ack_is_atomic(fault: &str, expected_error: &str) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store");
        let mut store = configured(temp.path());
        let config = store.forwarding.clone().unwrap();
        let origin = store.origin().to_owned();
        store.conn.execute_batch(fault).unwrap();
        store.admission_full = true;
        assert!(
            store
                .acknowledge("authority", &origin, 40, 40)
                .unwrap_err()
                .contains(expected_error)
        );
        assert!(
            store.admission_full,
            "failed GC must not clear the admission-full cache"
        );
        assert!(store.conn.is_autocommit());
        assert_eq!(store.forwarded().unwrap(), 0);
        assert_eq!(store.reclaimed().unwrap(), 0);
        assert_eq!(store.highest_record_id().unwrap(), 40);
        assert_eq!(store.count().unwrap(), 40);
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM legacy_retries", [], |r| r
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
        let retry = store.submit(&raw(1), None).unwrap();
        assert!(retry.duplicate);
        assert_eq!(retry.id, 1);
        let mut conflicting = raw(1);
        conflicting.push(b' ');
        assert!(
            store
                .submit(&conflicting, None)
                .unwrap_err()
                .contains("identity conflict")
        );
        drop(store);

        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        store.configure_forwarding(Some(config)).unwrap();
        assert_eq!(store.origin(), origin);
        assert_eq!(store.forwarded().unwrap(), 0);
        assert_eq!(store.reclaimed().unwrap(), 0);
        let pending = store.pending_batch().unwrap();
        assert_eq!(pending.len(), 40);
        for (index, row) in pending.iter().enumerate() {
            assert_eq!(row.source_id, index as u64 + 1);
            assert_eq!(row.raw.as_bytes(), raw(row.source_id));
        }
        store
            .conn
            .execute_batch("DROP TRIGGER fail_ack_gc")
            .unwrap();
        store.acknowledge("authority", &origin, 40, 40).unwrap();
        assert_eq!(store.forwarded().unwrap(), 40);
        assert_eq!(store.reclaimed().unwrap(), 32);
        assert_eq!(store.count().unwrap(), 8);
        assert!(store.reclaim_confirmed().unwrap());
        assert_eq!(store.reclaimed().unwrap(), 40);
        assert_eq!(store.count().unwrap(), 0);
        assert_eq!(store.highest_record_id().unwrap(), 40);
        drop(store);
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        assert_eq!(store.origin(), origin);
        assert_eq!(store.forwarded().unwrap(), 40);
        assert_eq!(store.reclaimed().unwrap(), 40);
        assert_eq!(store.submit(&raw(41), None).unwrap().id, 41);
    }
    #[test]
    fn delete_failure_rolls_back_ack_and_retains_exact_retry_after_reopen() {
        failed_ack_is_atomic(
            "CREATE TRIGGER fail_ack_gc BEFORE DELETE ON records BEGIN SELECT RAISE(ABORT, 'injected GC delete failure'); END;",
            "injected GC delete failure",
        );
    }
    #[test]
    fn outer_release_failure_rolls_back_ack_gc_and_admission_full_cache() {
        failed_ack_is_atomic(
            "PRAGMA foreign_keys=ON;
CREATE TABLE ack_parent(id INTEGER PRIMARY KEY);
CREATE TABLE ack_child(id INTEGER REFERENCES ack_parent(id) DEFERRABLE INITIALLY DEFERRED);
CREATE TRIGGER fail_ack_gc AFTER DELETE ON records BEGIN INSERT INTO ack_child VALUES(1); END;",
            "FOREIGN KEY constraint failed",
        );
    }
    #[test]
    fn ack_refuses_an_uncommitted_connection_batch() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = configured(temp.path());
        let origin = store.origin().to_owned();
        store.begin_connection_batch().unwrap();
        assert!(
            store
                .acknowledge("authority", &origin, 40, 40)
                .unwrap_err()
                .contains("custody transaction")
        );
        assert_eq!(store.forwarded().unwrap(), 0);
        assert_eq!(store.reclaimed().unwrap(), 0);
        store.rollback_connection_batch().unwrap();
        store.acknowledge("authority", &origin, 40, 40).unwrap();
        assert_eq!(store.reclaimed().unwrap(), 32);
    }
    #[test]
    fn gc_refuses_a_connection_batch_without_clearing_admission_full() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store");
        let mut store = configured(temp.path());
        let config = store.forwarding.clone().unwrap();
        let origin = store.origin().to_owned();
        store.acknowledge("authority", &origin, 40, 40).unwrap();
        store.admission_full = true;
        store.begin_connection_batch().unwrap();
        assert!(
            store
                .reclaim_confirmed()
                .unwrap_err()
                .contains("custody connection batch")
        );
        store.rollback_connection_batch().unwrap();
        assert!(store.admission_full);
        assert_eq!(store.reclaimed().unwrap(), 32);
        assert_eq!(store.read_raw(32, 1).unwrap()[0], (33, raw(33)));
        drop(store);
        let mut store = Store::open(&path, 2 * MIN_QUOTA).unwrap();
        store.configure_forwarding(Some(config)).unwrap();
        assert_eq!(store.forwarded().unwrap(), 40);
        assert_eq!(store.reclaimed().unwrap(), 32);
        assert_eq!(store.count().unwrap(), 8);
        assert!(store.reclaim_confirmed().unwrap());
        assert_eq!(store.reclaimed().unwrap(), 40);
    }
}
