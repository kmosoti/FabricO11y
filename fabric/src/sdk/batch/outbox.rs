//! One-owner storage for the authenticated lease, one pending frame, and receipts.
use super::{BatchError, Receipt, observation};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};
use rustix::fs::{FlockOperation, OFlags, StatFs, flock, fstatfs};
use std::{
    fs::{self, DirBuilder, File, Metadata, OpenOptions},
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
    time::Duration,
};

const APPLICATION_ID: u32 = u32::from_be_bytes(*b"FBOX");
const SCHEMA_VERSION: u32 = 1;
const MAX_REPORTS: u64 = 4096;
const MAX_DB_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RECEIPT: usize = 4096;

const SCHEMA: &str = "
    CREATE TABLE batch_state(
        singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
        producer TEXT NOT NULL,
        resource TEXT NOT NULL,
        origin TEXT,
        generation TEXT,
        expires INTEGER,
        expired INTEGER NOT NULL DEFAULT 0 CHECK(expired IN (0,1)),
        sequence INTEGER NOT NULL DEFAULT 0 CHECK(sequence BETWEEN 0 AND 4096),
        CHECK((origin IS NULL AND generation IS NULL AND expires IS NULL AND sequence = 0 AND expired = 0)
           OR (origin IS NOT NULL AND generation IS NOT NULL AND expires IS NOT NULL))
    ) STRICT;
    CREATE TABLE reports(
        ordinal INTEGER PRIMARY KEY CHECK(ordinal BETWEEN 1 AND 4096),
        report_id TEXT NOT NULL UNIQUE CHECK(length(report_id) = 36),
        digest BLOB NOT NULL CHECK(length(digest) = 32),
        payload BLOB CHECK(length(payload) BETWEEN 1 AND 65536),
        receipt BLOB CHECK(length(receipt) BETWEEN 1 AND 4096),
        CHECK((payload IS NULL) <> (receipt IS NULL))
    ) STRICT;
    CREATE UNIQUE INDEX one_pending ON reports((1)) WHERE payload IS NOT NULL;
";

#[derive(Clone)]
pub(super) struct Lease {
    pub origin: String,
    pub generation: String,
    pub expires: u64,
    pub expired: bool,
    pub sequence: u64,
}

pub(super) struct Pending {
    pub report_id: String,
    pub digest: [u8; 32],
    pub bytes: Vec<u8>,
}

pub(super) enum SavedReport {
    Pending(Pending),
    Confirmed([u8; 32], Receipt),
}

pub(super) struct Outbox {
    conn: Connection,
    // Field order closes SQLite before releasing the directory lock.
    _owner: File,
}
impl Outbox {
    pub fn open(path: &Path, producer: &str, resource: &str) -> Result<Self, BatchError> {
        let (path, owner) = open_directory(path)?;
        let db = path.join("outbox.sqlite");
        let journal = path.join("outbox.sqlite-journal");
        validate_file(&journal, MAX_DB_BYTES * 2)?;
        for name in ["outbox.sqlite-wal", "outbox.sqlite-shm"] {
            if validate_file(&path.join(name), 0)? {
                return Err(BatchError::Storage("outbox has a WAL sidecar".into()));
            }
        }
        if !validate_file(&db, MAX_DB_BYTES)? {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(OFlags::NOFOLLOW.bits() as i32)
                .open(&db)
                .map_err(storage)?;
        }
        let mut conn = Connection::open_with_flags(
            &db,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(storage)?;
        conn.busy_timeout(Duration::from_millis(100))
            .map_err(storage)?;
        let application_id: u32 = conn
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .map_err(storage)?;
        let version: u32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(storage)?;
        let initialize = application_id == 0
            && version == 0
            && !conn
                .query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema)", [], |row| {
                    row.get::<_, bool>(0)
                })
                .map_err(storage)?;
        if !initialize && (application_id != APPLICATION_ID || version != SCHEMA_VERSION) {
            return Err(BatchError::Storage("unsupported outbox database".into()));
        }
        conn.execute_batch(
            "PRAGMA journal_mode=DELETE;
             PRAGMA synchronous=EXTRA;
             PRAGMA temp_store=MEMORY;
             PRAGMA cache_size=-256;
             PRAGMA locking_mode=EXCLUSIVE;",
        )
        .map_err(storage)?;
        let page_size: u64 = conn
            .pragma_query_value(None, "page_size", |row| row.get(0))
            .map_err(storage)?;
        let journal_mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .map_err(storage)?;
        let sync: u32 = conn
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .map_err(storage)?;
        if journal_mode != "delete" || sync != 3 || !(512..=65536).contains(&page_size) {
            return Err(BatchError::Storage(
                "unsupported outbox durability settings".into(),
            ));
        }
        let max_pages = MAX_DB_BYTES / page_size;
        let configured_pages: u64 = conn
            .pragma_update_and_check(None, "max_page_count", max_pages, |row| row.get(0))
            .map_err(storage)?;
        if configured_pages != max_pages {
            return Err(BatchError::Storage(
                "outbox database exceeds its bound".into(),
            ));
        }
        if initialize {
            let transaction = conn.transaction().map_err(storage)?;
            transaction.execute_batch(SCHEMA).map_err(storage)?;
            transaction
                .execute(
                    "INSERT INTO batch_state(singleton,producer,resource) VALUES(1,?1,?2)",
                    params![producer, resource],
                )
                .map_err(storage)?;
            transaction
                .pragma_update(None, "application_id", APPLICATION_ID)
                .map_err(storage)?;
            transaction
                .pragma_update(None, "user_version", SCHEMA_VERSION)
                .map_err(storage)?;
            transaction.commit().map_err(storage)?;
        }
        let bound: bool = conn
            .query_row(
                "SELECT producer=?1 AND resource=?2 FROM batch_state WHERE singleton=1",
                params![producer, resource],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if !bound {
            return Err(BatchError::Storage(
                "outbox is bound to another producer or resource".into(),
            ));
        }
        owner.sync_all().map_err(storage)?;
        Ok(Self {
            conn,
            _owner: owner,
        })
    }

    pub fn lease(&self) -> Result<Option<Lease>, BatchError> {
        self.conn
            .query_row(
                "SELECT origin,generation,expires,expired,sequence
                 FROM batch_state WHERE singleton=1 AND generation IS NOT NULL",
                [],
                |row| {
                    Ok(Lease {
                        origin: row.get(0)?,
                        generation: row.get(1)?,
                        expires: row.get(2)?,
                        expired: row.get(3)?,
                        sequence: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(storage)
    }

    pub fn pending_lease(&self) -> Result<Lease, BatchError> {
        self.lease()?
            .filter(|lease| lease.sequence > 0)
            .ok_or_else(|| BatchError::Storage("pending report has no lease".into()))
    }

    pub fn save_lease(&self, lease: &Lease) -> Result<(), BatchError> {
        self.update_one(
            "UPDATE batch_state SET origin=?1,generation=?2,expires=?3,expired=0,sequence=0
             WHERE singleton=1 AND NOT EXISTS(SELECT 1 FROM reports WHERE payload IS NOT NULL)",
            params![lease.origin, lease.generation, lease.expires],
        )
    }

    pub fn mark_expired(&self) -> Result<(), BatchError> {
        self.update_one("UPDATE batch_state SET expired=1 WHERE singleton=1", [])
    }

    pub fn check_capacity(&self) -> Result<(), BatchError> {
        let total: u64 = self
            .conn
            .query_row("SELECT coalesce(max(ordinal),0) FROM reports", [], |row| {
                row.get(0)
            })
            .map_err(storage)?;
        if total >= MAX_REPORTS {
            return Err(BatchError::Full);
        }
        Ok(())
    }

    pub fn stage(&mut self, pending: &Pending, sequence: u64) -> Result<(), BatchError> {
        let transaction = self.conn.transaction().map_err(storage)?;
        transaction
            .execute(
                "INSERT INTO reports(report_id,digest,payload) VALUES(?1,?2,?3)",
                params![pending.report_id, pending.digest.as_slice(), pending.bytes],
            )
            .map_err(|error| {
                if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DiskFull) {
                    BatchError::Full
                } else {
                    storage(error)
                }
            })?;
        let updated = transaction.execute(
            "UPDATE batch_state SET sequence=?1 WHERE singleton=1 AND sequence=?1-1 AND expired=0",
            [sequence],
        ).map_err(storage)?;
        require_one(updated)?;
        transaction.commit().map_err(storage)
    }

    pub fn confirm(&self, report_id: &str, receipt: &Receipt) -> Result<(), BatchError> {
        let receipt = serde_json::to_vec(receipt).map_err(storage)?;
        self.update_one(
            "UPDATE reports SET receipt=?1,payload=NULL WHERE report_id=?2 AND payload IS NOT NULL",
            params![receipt, report_id],
        )
    }

    fn update_one(&self, sql: &str, params: impl rusqlite::Params) -> Result<(), BatchError> {
        require_one(self.conn.execute(sql, params).map_err(storage)?)
    }

    pub fn find(&self, report_id: &str) -> Result<Option<SavedReport>, BatchError> {
        self.conn
            .query_row(
                "SELECT report_id,digest,payload,receipt FROM reports WHERE report_id=?1",
                [report_id],
                |row| {
                    if row.get_ref(2)?.as_blob_or_null()?.is_some() {
                        return read_pending(row).map(SavedReport::Pending);
                    }
                    Ok(SavedReport::Confirmed(
                        read_digest(row)?,
                        read_receipt(row, 3)?,
                    ))
                },
            )
            .optional()
            .map_err(storage)
    }

    pub fn pending(&self) -> Result<Option<Pending>, BatchError> {
        self.conn
            .query_row(
                "SELECT report_id,digest,payload FROM reports WHERE payload IS NOT NULL",
                [],
                read_pending,
            )
            .optional()
            .map_err(storage)
    }

    pub fn pending_id(&self) -> Result<Option<String>, BatchError> {
        self.conn
            .query_row(
                "SELECT report_id FROM reports WHERE payload IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)
    }

    pub fn confirmed_count(&self) -> Result<u64, BatchError> {
        self.conn
            .query_row(
                "SELECT count(*) FROM reports WHERE receipt IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .map_err(storage)
    }

    pub fn latest_receipt(&self) -> Result<Option<Receipt>, BatchError> {
        self.conn.query_row(
            "SELECT receipt FROM reports WHERE receipt IS NOT NULL ORDER BY ordinal DESC LIMIT 1",
            [], |row| read_receipt(row, 0),
        ).optional().map_err(storage)
    }
}

fn require_one(updated: usize) -> Result<(), BatchError> {
    if updated != 1 {
        return Err(BatchError::Storage(
            "outbox state transition affected the wrong rows".into(),
        ));
    }
    Ok(())
}

fn read_digest(row: &Row<'_>) -> rusqlite::Result<[u8; 32]> {
    row.get_ref(1)?
        .as_blob()?
        .try_into()
        .map_err(|_| rusqlite::Error::InvalidQuery)
}

fn read_pending(row: &Row<'_>) -> rusqlite::Result<Pending> {
    let bytes = row.get_ref(2)?.as_blob()?;
    if bytes.is_empty() || bytes.len() > observation::MAX_FRAME {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(Pending {
        report_id: row.get(0)?,
        digest: read_digest(row)?,
        bytes: bytes.to_vec(),
    })
}

fn read_receipt(row: &Row<'_>, index: usize) -> rusqlite::Result<Receipt> {
    let bytes = row.get_ref(index)?.as_blob()?;
    if bytes.is_empty() || bytes.len() > MAX_RECEIPT {
        return Err(rusqlite::Error::InvalidQuery);
    }
    serde_json::from_slice(bytes).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Blob,
            Box::new(error),
        )
    })
}

fn open_directory(path: &Path) -> Result<(PathBuf, File), BatchError> {
    let name = path
        .file_name()
        .ok_or_else(|| BatchError::Storage("invalid outbox path".into()))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = std::path::absolute(parent).map_err(storage)?;
    let owner = rustix::process::geteuid().as_raw();
    // SQLite needs a stable pathname for its rollback journal. Check the presented
    // ancestors from root to leaf so an indirect path cannot switch outbox history.
    let mut ancestor = PathBuf::new();
    for component in parent.components() {
        if component == Component::ParentDir {
            return Err(storage("outbox path must not contain '..'"));
        }
        ancestor.push(component);
        let metadata = fs::symlink_metadata(&ancestor).map_err(storage)?;
        if !metadata.is_dir()
            || (metadata.uid() != owner && metadata.uid() != 0)
            || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
        {
            return Err(BatchError::Storage(
                "outbox parent must have trusted ownership and permissions".into(),
            ));
        }
    }
    let parent_file = File::open(&parent).map_err(storage)?;
    let path = parent.join(name);
    require_disk(&path, fstatfs(&parent_file).map_err(storage)?)?;
    match DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(storage(error)),
    }
    let directory = File::from(
        rustix::fs::open(
            &path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(storage)?,
    );
    let metadata = directory.metadata().map_err(storage)?;
    require_disk(&path, fstatfs(&directory).map_err(storage)?)?;
    if !private_owner(&metadata) {
        return Err(BatchError::Storage(
            "outbox must be an owner-only directory".into(),
        ));
    }
    flock(&directory, FlockOperation::NonBlockingLockExclusive)
        .map_err(|error| storage(format!("could not lock outbox: {error}")))?;
    parent_file.sync_all().map_err(storage)?;
    Ok((path, directory))
}

fn require_disk(path: &Path, stat: StatFs) -> Result<(), BatchError> {
    // Linux tmpfs and ramfs magic numbers, including alternate mount locations.
    if path.starts_with("/tmp")
        || path.starts_with("/dev/shm")
        || matches!(stat.f_type as u64, 0x0102_1994 | 0x8584_58f6)
    {
        return Err(BatchError::Storage(
            "outbox requires a disk-backed directory".into(),
        ));
    }
    Ok(())
}

fn validate_file(path: &Path, max_bytes: u64) -> Result<bool, BatchError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file()
                || !private_owner(&metadata)
                || metadata.nlink() != 1
                || metadata.len() > max_bytes
            {
                return Err(storage(format!("invalid outbox file {}", path.display())));
            }
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(storage(error)),
    }
}

fn private_owner(metadata: &Metadata) -> bool {
    metadata.uid() == rustix::process::geteuid().as_raw() && metadata.mode() & 0o077 == 0
}

fn storage(error: impl std::fmt::Display) -> BatchError {
    BatchError::Storage(error.to_string())
}
