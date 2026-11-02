//! One-owner SQLite custody with durable retry identities and source checkpoints.
use crate::observation::{self, Observation};
use rusqlite::{Connection, OptionalExtension, params};
use rustix::fs::{FlockOperation, flock};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
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
    _lock: File,
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
        flock(&lock, FlockOperation::NonBlockingLockExclusive).map_err(err)?;
        let conn = Connection::open(dir.join("custody.sqlite")).map_err(err)?;
        conn.busy_timeout(std::time::Duration::from_millis(100))
            .map_err(err)?;
        // DELETE journal and no auto-vacuum: quota accounts for worst-case rollback
        // journal plus the entire database. No unbounded WAL or temporary files.
        conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA; PRAGMA temp_store=MEMORY; PRAGMA cache_size=-256; PRAGMA locking_mode=EXCLUSIVE; PRAGMA auto_vacuum=NONE;
CREATE TABLE IF NOT EXISTS records(id INTEGER PRIMARY KEY,producer TEXT NOT NULL,generation TEXT NOT NULL,sequence TEXT NOT NULL,payload BLOB NOT NULL,UNIQUE(producer,generation,sequence));
CREATE TABLE IF NOT EXISTS checkpoints(source TEXT PRIMARY KEY,position TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS registries(name TEXT PRIMARY KEY,descriptor TEXT NOT NULL);").map_err(err)?;
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
        File::open(dir).and_then(|f| f.sync_all()).map_err(err)?;
        Ok(Self {
            conn,
            dir: dir.to_owned(),
            quota,
            _lock: lock,
        })
    }
    pub fn submit(
        &mut self,
        bytes: &[u8],
        checkpoint: Option<&Checkpoint>,
    ) -> Result<Receipt, String> {
        let observation = observation::decode(bytes)?;
        if let Some(cp) = checkpoint
            && (cp.source.is_empty() || cp.source.len() > 256 || cp.position.len() > 8192)
        {
            return Err("invalid checkpoint".into());
        }
        let i = &observation.identity;
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
        if self
            .disk_bytes()?
            .saturating_add((bytes.len() as u64 + 32768) * 2)
            .saturating_add(RESERVE)
            > self.quota
        {
            return Err("storage full".into());
        }
        let tx = self.conn.transaction().map_err(err)?;
        tx.execute(
            "INSERT INTO records(producer,generation,sequence,payload) VALUES(?1,?2,?3,?4)",
            params![i.producer, i.generation, i.sequence.to_string(), bytes],
        )
        .map_err(err)?;
        let id = tx.last_insert_rowid() as u64;
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
            .query_row("SELECT coalesce(max(id),0) FROM records", [], |r| r.get(0))
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
    pub fn read(&self, after: u64, limit: usize) -> Result<Vec<(u64, Observation)>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT id,payload FROM records WHERE id>?1 ORDER BY id LIMIT ?2")
            .map_err(err)?;
        let rows = stmt
            .query_map(params![after, limit.min(1000)], |r| {
                Ok((r.get::<_, u64>(0)?, r.get::<_, Vec<u8>>(1)?))
            })
            .map_err(err)?;
        rows.map(|r| {
            let (id, b) = r.map_err(err)?;
            Ok((id, observation::decode(&b)?))
        })
        .collect()
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
