//! ACID MVCC OLTP & hot adjacency graph storage backed by `redb` (`3.1.3`),
//! paired with lock-free atomic JSON live state snapshots for concurrent CLI reads.

use chrono::Utc;
use fabric_core::{DriftReport, HostSnapshot, SCHEMA_VERSION, StorageLayout};
use fabric_graph::CausalGraph;
use redb::{
    Builder, Database, DatabaseError, ReadableDatabase, ReadableTable, TableDefinition, TableError,
};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ENTITIES_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("entities");
const ADJ_OUT_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("adj_out");
const ADJ_IN_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("adj_in");
const UNITS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("units");

#[cfg(test)]
mod sync_fault {
    use redb::{StorageBackend, backends::FileBackend};
    use std::{
        cell::RefCell,
        io,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    #[derive(Debug, Default)]
    pub(super) struct Control {
        pub armed: AtomicBool,
        pub fired: AtomicUsize,
    }
    thread_local! { pub(super) static SELECTED: RefCell<Option<Arc<Control>>> = const { RefCell::new(None) }; }
    #[derive(Debug)]
    pub(super) struct Backend {
        pub inner: FileBackend,
        pub control: Arc<Control>,
    }
    impl StorageBackend for Backend {
        fn len(&self) -> io::Result<u64> {
            self.inner.len()
        }
        fn read(&self, offset: u64, out: &mut [u8]) -> io::Result<()> {
            self.inner.read(offset, out)
        }
        fn set_len(&self, len: u64) -> io::Result<()> {
            self.inner.set_len(len)
        }
        fn write(&self, offset: u64, data: &[u8]) -> io::Result<()> {
            self.inner.write(offset, data)
        }
        fn sync_data(&self) -> io::Result<()> {
            if self.control.armed.swap(false, Ordering::SeqCst) {
                self.control.fired.fetch_add(1, Ordering::SeqCst);
                return Err(io::Error::other("injected catalog commit sync failure"));
            }
            self.inner.sync_data()
        }
        fn close(&self) -> io::Result<()> {
            self.inner.close()
        }
    }
}

const WATERMARKS_TABLE: TableDefinition<&str, u64> = TableDefinition::new("watermarks");
const ANOMALIES_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("anomalies");
const META_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
const SPINDLE_PROJECTION_TABLE: TableDefinition<&str, (&str, u64)> =
    TableDefinition::new("spindle_projection");
const SPINDLE_PROJECTION_BINDING: &str = "binding";
const SPINDLE_WATERMARK: &str = "spindle_watermark_id";

const REDB_CACHE_BYTES: usize = 4 * 1024 * 1024;
const OPEN_RETRY_ATTEMPTS: usize = 40;
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(10);

/// Serializable live snapshot mirror read quickly by CLI queries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedLiveState {
    pub schema_version: String,
    pub updated_at_unix_ms: i64,
    pub snapshot: HostSnapshot,
    pub drift: DriftReport,
    pub graph: CausalGraph,
}

/// Durable collector progress and the origin to which future projection commits belong.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpindleProjectionFrontier {
    pub origin: Option<String>,
    /// Records through this ID predate origin binding and lack verified provenance.
    pub legacy_through: u64,
    pub record_id: u64,
}

impl SpindleProjectionFrontier {
    fn from_binding(record_id: u64, binding: Option<(&str, u64)>) -> Result<Self, String> {
        let Some((origin, legacy_through)) = binding else {
            return Ok(Self {
                origin: None,
                legacy_through: record_id,
                record_id,
            });
        };
        validate_spindle_projection_origin(origin)?;
        if legacy_through > record_id {
            return Err("Spindle projection watermark precedes its legacy boundary".to_owned());
        }
        Ok(Self {
            origin: Some(origin.to_owned()),
            legacy_through,
            record_id,
        })
    }
}

fn validate_spindle_projection_origin(origin: &str) -> Result<(), String> {
    if origin.is_empty() || origin.len() > 128 || origin.chars().any(char::is_control) {
        return Err("invalid Spindle projection origin".to_owned());
    }
    Ok(())
}

/// Converged `redb` OLTP + adjacency catalog rooted under a validated non-tmpfs directory.
#[derive(Debug, Clone)]
pub struct CatalogStore {
    layout: StorageLayout,
}

impl CatalogStore {
    /// Initializes storage directories beneath `data_dir` (refusing `/tmp` or `/dev/shm`).
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, String> {
        let layout = crate::validated_storage_layout(data_dir)?;
        Self::ensure_layout(&layout)?;
        Ok(Self { layout })
    }

    #[must_use]
    pub fn layout(&self) -> &StorageLayout {
        &self.layout
    }

    fn ensure_layout(layout: &StorageLayout) -> Result<(), String> {
        crate::olap::reject_symlink_path(layout.data_dir())?;
        crate::olap::ensure_durable_non_tmp_dir(None, layout.data_dir())?;
        for dir in [
            layout.state_dir(),
            layout.staging_dir(),
            layout.spill_dir(),
            layout.olap_dir(),
        ] {
            crate::olap::ensure_durable_non_tmp_dir(Some(layout.data_dir()), &dir)?;
        }
        for domain in crate::olap::OLAP_DOMAINS {
            let domain_dir = layout.olap_domain_dir(domain);
            crate::olap::ensure_durable_non_tmp_dir(Some(layout.data_dir()), &domain_dir)?;
        }
        crate::olap::reject_symlink_path(&layout.catalog_redb())?;
        crate::validate_non_tmp_path_on_disk(&layout.catalog_redb()).map_err(|e| e.to_string())?;
        crate::olap::reject_symlink_path(&layout.live_state_json())?;
        crate::validate_non_tmp_path_on_disk(&layout.live_state_json())
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn open_db_with_retry(&self) -> Result<Database, String> {
        let state_dir =
            crate::olap::ensure_durable_layout_subdir(&self.layout, &self.layout.state_dir())?;
        crate::olap::reject_symlinks_below_stop(
            Some(self.layout.data_dir()),
            &self.layout.catalog_redb(),
        )?;
        let db_path = crate::validate_non_tmp_path_on_disk(&state_dir.join("catalog.redb"))
            .map_err(|e| e.to_string())?;
        for attempt in 0..OPEN_RETRY_ATTEMPTS {
            let data_fd = crate::olap::open_anchor_data_dir(&self.layout)?;
            let state_fd = crate::olap::open_child_dir_nofollow(&data_fd, "state", "state dir")?
                .ok_or_else(|| format!("missing state dir {}", state_dir.display()))?;
            let state_fd_meta = state_fd
                .metadata()
                .map_err(|e| format!("failed inspecting state fd {}: {e}", state_dir.display()))?;
            let state_path_meta = fs::symlink_metadata(&state_dir)
                .map_err(|e| format!("failed inspecting state dir {}: {e}", state_dir.display()))?;
            if state_path_meta.file_type().is_symlink()
                || !state_path_meta.is_dir()
                || state_path_meta.dev() != state_fd_meta.dev()
                || state_path_meta.ino() != state_fd_meta.ino()
            {
                return Err(format!(
                    "state dir {} changed concurrently during catalog open",
                    state_dir.display()
                ));
            }
            let proc_path =
                crate::olap::proc_fd_child_path(&state_fd, "catalog.redb", "catalog.redb")?;
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(crate::olap::O_NOFOLLOW_FLAG | crate::olap::O_NONBLOCK_FLAG)
                .open(&proc_path)
                .map_err(|e| {
                    if crate::olap::is_eloop_error(&e) {
                        format!("refusing symlinked catalog.redb {}", db_path.display())
                    } else {
                        format!("failed opening catalog.redb {}: {e}", db_path.display())
                    }
                })?;
            let fd_meta = file.metadata().map_err(|e| {
                format!(
                    "failed inspecting catalog.redb fd {}: {e}",
                    db_path.display()
                )
            })?;
            if fd_meta.file_type().is_symlink() {
                return Err(format!(
                    "refusing symlinked catalog.redb {}",
                    db_path.display()
                ));
            }
            if !fd_meta.is_file() {
                return Err(format!("invalid catalog.redb path {}", db_path.display()));
            }
            let proc_meta = fs::symlink_metadata(&proc_path).map_err(|e| {
                format!("failed inspecting catalog.redb {}: {e}", db_path.display())
            })?;
            if proc_meta.file_type().is_symlink()
                || !proc_meta.is_file()
                || proc_meta.dev() != fd_meta.dev()
                || proc_meta.ino() != fd_meta.ino()
            {
                return Err(format!(
                    "catalog.redb {} changed concurrently during open",
                    db_path.display()
                ));
            }
            crate::atomic_fd::enforce_private_file_mode(&file, "catalog.redb")?;
            let mut builder = Builder::new();
            builder.set_cache_size(REDB_CACHE_BYTES);
            #[cfg(not(test))]
            let opened = builder.create_file(file);
            #[cfg(test)]
            let opened = match sync_fault::SELECTED.with(|selected| selected.borrow().clone()) {
                Some(control) => redb::backends::FileBackend::new(file).and_then(|inner| {
                    builder.create_with_backend(sync_fault::Backend { inner, control })
                }),
                None => builder.create_file(file),
            };
            match opened {
                Ok(db) => {
                    state_fd.sync_all().map_err(|e| {
                        format!("failed syncing state dir {}: {e}", state_dir.display())
                    })?;
                    return Ok(db);
                }
                Err(DatabaseError::DatabaseAlreadyOpen) if attempt + 1 < OPEN_RETRY_ATTEMPTS => {
                    thread::sleep(OPEN_RETRY_DELAY);
                }
                Err(err) => {
                    return Err(format!(
                        "failed opening redb catalog {}: {err}",
                        db_path.display()
                    ));
                }
            }
        }
        Err(format!(
            "timed out waiting for redb lock on {}",
            db_path.display()
        ))
    }

    pub(crate) fn read_watermark_for_recovery(
        layout: &StorageLayout,
        key: &str,
    ) -> Result<u64, String> {
        crate::validate_non_tmp_path_on_disk(layout.data_dir()).map_err(|e| e.to_string())?;
        crate::olap::reject_symlinks_below_stop(Some(layout.data_dir()), &layout.catalog_redb())?;
        let db_path = crate::validate_non_tmp_path_on_disk(&layout.catalog_redb())
            .map_err(|e| e.to_string())?;
        for attempt in 0..OPEN_RETRY_ATTEMPTS {
            let data_fd = crate::olap::open_anchor_data_dir(layout)?;
            let Some(state_fd) =
                crate::olap::open_child_dir_nofollow(&data_fd, "state", "state dir")?
            else {
                return Ok(0);
            };
            let proc_path =
                crate::olap::proc_fd_child_path(&state_fd, "catalog.redb", "catalog.redb")?;
            let file = match OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(crate::olap::O_NOFOLLOW_FLAG | crate::olap::O_NONBLOCK_FLAG)
                .open(&proc_path)
            {
                Ok(f) => f,
                Err(e) if e.kind() == ErrorKind::NotFound => return Ok(0),
                Err(e) if crate::olap::is_eloop_error(&e) => {
                    return Err(format!(
                        "refusing symlinked catalog.redb {}",
                        db_path.display()
                    ));
                }
                Err(e) => {
                    return Err(format!(
                        "failed opening catalog.redb {}: {e}",
                        db_path.display()
                    ));
                }
            };
            let fd_meta = file.metadata().map_err(|e| {
                format!(
                    "failed inspecting catalog.redb fd {}: {e}",
                    db_path.display()
                )
            })?;
            if fd_meta.file_type().is_symlink() {
                return Err(format!(
                    "refusing symlinked catalog.redb {}",
                    db_path.display()
                ));
            }
            if !fd_meta.is_file() {
                return Err(format!(
                    "obstructing non-file at catalog.redb {}",
                    db_path.display()
                ));
            }
            if fd_meta.len() == 0 {
                return Ok(0);
            }
            let mut builder = Builder::new();
            builder.set_cache_size(REDB_CACHE_BYTES);
            match builder.create_file(file) {
                Ok(db) => {
                    let read_txn = db.begin_read().map_err(|e| e.to_string())?;
                    let Ok(table) = read_txn.open_table(WATERMARKS_TABLE) else {
                        return Ok(0);
                    };
                    let value = table
                        .get(key)
                        .map_err(|e| e.to_string())?
                        .map(|guard| guard.value())
                        .unwrap_or(0);
                    return Ok(value);
                }
                Err(DatabaseError::DatabaseAlreadyOpen) if attempt + 1 < OPEN_RETRY_ATTEMPTS => {
                    thread::sleep(OPEN_RETRY_DELAY);
                }
                Err(err) => {
                    return Err(format!(
                        "failed reading recovery watermark from {}: {err}",
                        db_path.display()
                    ));
                }
            }
        }
        Err(format!(
            "timed out waiting for redb recovery lock on {}",
            db_path.display()
        ))
    }

    /// Commits entities, adjacency lists, systemd units, watermarks, anomalies, and atomic live state.
    pub fn commit_cycle(
        &self,
        snapshot: &HostSnapshot,
        drift: &DriftReport,
        graph: &CausalGraph,
    ) -> Result<(), String> {
        let now_ms = if snapshot.timestamp_unix_ms > 0 {
            snapshot.timestamp_unix_ms
        } else {
            Utc::now().timestamp_millis()
        };

        let db = self.open_db_with_retry()?;
        let write_txn = db.begin_write().map_err(|e| e.to_string())?;
        {
            // Retain only current entities and avoid rewriting byte-identical nodes.
            let mut entities = write_txn
                .open_table(ENTITIES_TABLE)
                .map_err(|e| e.to_string())?;
            entities
                .retain(|id, _| graph.nodes().contains_key(id))
                .map_err(|e| e.to_string())?;
            for (id, node) in graph.nodes() {
                let bytes = serde_json::to_vec(node).map_err(|e| e.to_string())?;
                let unchanged = entities
                    .get(id.as_str())
                    .map_err(|e| e.to_string())?
                    .is_some_and(|old| old.value() == bytes.as_slice());
                if !unchanged {
                    entities
                        .insert(id.as_str(), bytes.as_slice())
                        .map_err(|e| e.to_string())?;
                }
            }

            let _ = write_txn.delete_table(ADJ_OUT_TABLE);
            let _ = write_txn.delete_table(ADJ_IN_TABLE);
            let mut adj_out = write_txn
                .open_table(ADJ_OUT_TABLE)
                .map_err(|e| e.to_string())?;
            let mut adj_in = write_txn
                .open_table(ADJ_IN_TABLE)
                .map_err(|e| e.to_string())?;
            let mut out_map: std::collections::BTreeMap<&str, Vec<&fabric_graph::GraphEdge>> =
                std::collections::BTreeMap::new();
            let mut in_map: std::collections::BTreeMap<&str, Vec<&fabric_graph::GraphEdge>> =
                std::collections::BTreeMap::new();
            for edge in graph.edges() {
                out_map.entry(edge.src.as_str()).or_default().push(edge);
                in_map.entry(edge.dst.as_str()).or_default().push(edge);
            }
            for (src, list) in out_map {
                let bytes = serde_json::to_vec(&list).map_err(|e| e.to_string())?;
                adj_out
                    .insert(src, bytes.as_slice())
                    .map_err(|e| e.to_string())?;
            }
            for (dst, list) in in_map {
                let bytes = serde_json::to_vec(&list).map_err(|e| e.to_string())?;
                adj_in
                    .insert(dst, bytes.as_slice())
                    .map_err(|e| e.to_string())?;
            }

            let _ = write_txn.delete_table(UNITS_TABLE);
            let mut units = write_txn
                .open_table(UNITS_TABLE)
                .map_err(|e| e.to_string())?;
            for unit in &snapshot.units {
                let bytes = serde_json::to_vec(unit).map_err(|e| e.to_string())?;
                units
                    .insert(unit.name.as_str(), bytes.as_slice())
                    .map_err(|e| e.to_string())?;
            }

            let mut watermarks = write_txn
                .open_table(WATERMARKS_TABLE)
                .map_err(|e| e.to_string())?;
            watermarks
                .insert("collector_last_ms", now_ms.max(0) as u64)
                .map_err(|e| e.to_string())?;

            let mut anomalies = write_txn
                .open_table(ANOMALIES_TABLE)
                .map_err(|e| e.to_string())?;
            let drift_bytes = serde_json::to_vec(drift).map_err(|e| e.to_string())?;
            anomalies
                .insert("latest_drift", drift_bytes.as_slice())
                .map_err(|e| e.to_string())?;

            let mut meta = write_txn
                .open_table(META_TABLE)
                .map_err(|e| e.to_string())?;
            meta.insert("schema_version", SCHEMA_VERSION.as_bytes())
                .map_err(|e| e.to_string())?;
            let snap_bytes = serde_json::to_vec(snapshot).map_err(|e| e.to_string())?;
            meta.insert("latest_snapshot", snap_bytes.as_slice())
                .map_err(|e| e.to_string())?;
            let graph_bytes = serde_json::to_vec(graph).map_err(|e| e.to_string())?;
            meta.insert("latest_graph", graph_bytes.as_slice())
                .map_err(|e| e.to_string())?;
        }
        #[cfg(test)]
        sync_fault::SELECTED.with(|selected| {
            if let Some(control) = selected.borrow().as_ref() {
                control
                    .armed
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
        });
        write_txn.commit().map_err(|e| e.to_string())?;
        drop(db);

        let state = PersistedLiveState {
            schema_version: SCHEMA_VERSION.to_owned(),
            updated_at_unix_ms: now_ms,
            snapshot: snapshot.clone(),
            drift: drift.clone(),
            graph: graph.clone(),
        };
        self.write_live_state_atomic(&state)
    }

    /// Updates a named watermark in `catalog.redb`.
    pub fn set_watermark(&self, key: &str, value: u64) -> Result<(), String> {
        let db = self.open_db_with_retry()?;
        let write_txn = db.begin_write().map_err(|e| e.to_string())?;
        {
            let mut watermarks = write_txn
                .open_table(WATERMARKS_TABLE)
                .map_err(|e| e.to_string())?;
            watermarks.insert(key, value).map_err(|e| e.to_string())?;
        }
        write_txn.commit().map_err(|e| e.to_string())
    }

    /// Reads a watermark from `catalog.redb` if present.
    pub fn read_watermark(&self, key: &str) -> Result<Option<u64>, String> {
        crate::validate_non_tmp_path_on_disk(self.layout.data_dir()).map_err(|e| e.to_string())?;
        crate::olap::reject_symlinks_below_stop(
            Some(self.layout.data_dir()),
            &self.layout.catalog_redb(),
        )?;
        if !crate::olap::is_regular_file_no_symlink(&self.layout.catalog_redb(), "catalog.redb")? {
            return Ok(None);
        }
        let db = self.open_db_with_retry()?;
        let read_txn = db.begin_read().map_err(|e| e.to_string())?;
        let Ok(table) = read_txn.open_table(WATERMARKS_TABLE) else {
            return Ok(None);
        };
        let value = table
            .get(key)
            .map_err(|e| e.to_string())?
            .map(|guard| guard.value());
        Ok(value)
    }

    /// Binds the collector to an origin before staging that origin's records.
    ///
    /// The caller holds the projection writer lock. The legacy scalar watermark
    /// is captured with the binding and remains the recovery watermark.
    pub fn bind_spindle_projection(
        &self,
        origin: &str,
    ) -> Result<SpindleProjectionFrontier, String> {
        validate_spindle_projection_origin(origin)?;
        let db = self.open_db_with_retry()?;
        let txn = db.begin_write().map_err(|e| e.to_string())?;
        let record_id = {
            let table = txn
                .open_table(WATERMARKS_TABLE)
                .map_err(|e| e.to_string())?;
            table
                .get(SPINDLE_WATERMARK)
                .map_err(|e| e.to_string())?
                .map_or(0, |value| value.value())
        };
        let frontier = {
            let mut table = txn
                .open_table(SPINDLE_PROJECTION_TABLE)
                .map_err(|e| e.to_string())?;
            let existing = table
                .get(SPINDLE_PROJECTION_BINDING)
                .map_err(|e| e.to_string())?
                .map(|entry| {
                    let (bound, legacy_through) = entry.value();
                    (bound.to_owned(), legacy_through)
                });
            match existing {
                Some((bound, _)) if bound != origin => {
                    return Err(format!(
                        "Spindle projection origin mismatch: collector bound to {bound}, observed {origin}"
                    ));
                }
                Some((bound, legacy_through)) => {
                    return SpindleProjectionFrontier::from_binding(
                        record_id,
                        Some((&bound, legacy_through)),
                    );
                }
                None => {
                    table
                        .insert(SPINDLE_PROJECTION_BINDING, (origin, record_id))
                        .map_err(|e| e.to_string())?;
                    SpindleProjectionFrontier::from_binding(record_id, Some((origin, record_id)))?
                }
            }
        };
        txn.commit().map_err(|e| e.to_string())?;
        Ok(frontier)
    }

    /// Reads the binding and watermark from a single read-only catalog snapshot.
    ///
    /// A missing catalog has no projection state. An existing but invalid or
    /// corrupt catalog is an error. A writer may briefly hold the redb lock.
    pub fn read_spindle_projection_frontier(
        layout: &StorageLayout,
        deadline: Instant,
    ) -> Result<SpindleProjectionFrontier, String> {
        let mut builder = Builder::new();
        builder.set_cache_size(REDB_CACHE_BYTES);
        loop {
            if Instant::now() >= deadline {
                return Err("Spindle projection frontier deadline exceeded".to_owned());
            }
            let Some(data_fd) = crate::olap::open_anchor_data_dir_if_exists(layout)? else {
                return Ok(SpindleProjectionFrontier::default());
            };
            let Some(state_fd) =
                crate::olap::open_child_dir_nofollow(&data_fd, "state", "state dir")?
            else {
                return Ok(SpindleProjectionFrontier::default());
            };
            let Some((file, _)) = crate::olap::open_regular_file_in_dir_nofollow(
                &state_fd,
                "catalog.redb",
                "catalog",
            )?
            else {
                return Ok(SpindleProjectionFrontier::default());
            };
            // redb's read-only builder takes a path. Reopen the inode we just
            // pinned so it cannot follow a replacement at the original path.
            let pinned_catalog = format!("/proc/self/fd/{}", file.as_raw_fd());
            let db = match builder.open_read_only(pinned_catalog) {
                Ok(db) => db,
                Err(DatabaseError::DatabaseAlreadyOpen) => {
                    thread::sleep(
                        deadline
                            .saturating_duration_since(Instant::now())
                            .min(OPEN_RETRY_DELAY),
                    );
                    continue;
                }
                Err(e) => return Err(format!("failed reading Spindle projection frontier: {e}")),
            };
            let txn = db.begin_read().map_err(|e| e.to_string())?;
            let record_id = match txn.open_table(WATERMARKS_TABLE) {
                Ok(table) => table
                    .get(SPINDLE_WATERMARK)
                    .map_err(|e| e.to_string())?
                    .map_or(0, |value| value.value()),
                Err(TableError::TableDoesNotExist(_)) => 0,
                Err(e) => return Err(e.to_string()),
            };
            let binding_table = match txn.open_table(SPINDLE_PROJECTION_TABLE) {
                Ok(table) => Some(table),
                Err(TableError::TableDoesNotExist(_)) => None,
                Err(e) => return Err(e.to_string()),
            };
            let binding = binding_table
                .as_ref()
                .map(|table| table.get(SPINDLE_PROJECTION_BINDING))
                .transpose()
                .map_err(|e| e.to_string())?
                .flatten();
            if Instant::now() >= deadline {
                return Err("Spindle projection frontier deadline exceeded".to_owned());
            }
            return SpindleProjectionFrontier::from_binding(
                record_id,
                binding.as_ref().map(|entry| entry.value()),
            );
        }
    }

    /// Reads the latest persisted snapshot, drift report, and causal graph.
    pub fn load_live_state(&self) -> Result<Option<PersistedLiveState>, String> {
        crate::validate_non_tmp_path_on_disk(self.layout.data_dir()).map_err(|e| e.to_string())?;
        let json_path = self.layout.live_state_json();
        crate::olap::reject_symlinks_below_stop(Some(self.layout.data_dir()), &json_path)?;
        crate::olap::reject_symlinks_below_stop(
            Some(self.layout.data_dir()),
            &self.layout.catalog_redb(),
        )?;
        if crate::olap::is_regular_file_no_symlink(&json_path, "live_state.json")? {
            let safe_json =
                crate::validate_non_tmp_path_on_disk(&json_path).map_err(|e| e.to_string())?;
            if let Some(bytes) = crate::olap::read_non_symlink_bytes_bounded_if_present(
                &safe_json,
                "live_state.json",
                64 * 1024 * 1024,
            )? {
                let state: PersistedLiveState =
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                return Ok(Some(state));
            }
        }

        if !crate::olap::is_regular_file_no_symlink(&self.layout.catalog_redb(), "catalog.redb")? {
            return Ok(None);
        }
        let db = self.open_db_with_retry()?;
        let read_txn = db.begin_read().map_err(|e| e.to_string())?;
        let Ok(meta) = read_txn.open_table(META_TABLE) else {
            return Ok(None);
        };
        let Some(snap_guard) = meta.get("latest_snapshot").map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let snapshot: HostSnapshot =
            serde_json::from_slice(snap_guard.value()).map_err(|e| e.to_string())?;
        let graph =
            if let Some(graph_guard) = meta.get("latest_graph").map_err(|e| e.to_string())? {
                serde_json::from_slice(graph_guard.value()).map_err(|e| e.to_string())?
            } else {
                CausalGraph::from_snapshot(&snapshot, &[])
            };
        let drift = if let Ok(anom) = read_txn.open_table(ANOMALIES_TABLE)
            && let Some(d_guard) = anom.get("latest_drift").map_err(|e| e.to_string())?
        {
            serde_json::from_slice(d_guard.value()).map_err(|e| e.to_string())?
        } else {
            DriftReport::default()
        };

        Ok(Some(PersistedLiveState {
            schema_version: SCHEMA_VERSION.to_owned(),
            updated_at_unix_ms: snapshot.timestamp_unix_ms,
            snapshot,
            drift,
            graph,
        }))
    }

    fn write_live_state_atomic(&self, state: &PersistedLiveState) -> Result<(), String> {
        let _staging_dir =
            crate::olap::ensure_durable_layout_subdir(&self.layout, &self.layout.staging_dir())?;
        let _state_dir =
            crate::olap::ensure_durable_layout_subdir(&self.layout, &self.layout.state_dir())?;
        let live_state_path = self.layout.live_state_json();
        crate::olap::reject_symlinks_below_stop(Some(self.layout.data_dir()), &live_state_path)?;
        let _validated_live_state =
            crate::validate_non_tmp_path_on_disk(&live_state_path).map_err(|e| e.to_string())?;

        let data_fd = std::os::fd::OwnedFd::from(crate::olap::open_anchor_data_dir(&self.layout)?);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let tmp_name = format!("live_state.{}.{stamp}.json", std::process::id());
        let bytes = serde_json::to_vec(state).map_err(|e| e.to_string())?;

        crate::atomic_fd::write_staged_then_renameat(
            self.layout.data_dir(),
            &data_fd,
            "staging",
            "state",
            &tmp_name,
            "live_state.json",
            &bytes,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_core::evaluate_five_pillars;
    use tempfile::TempDir;

    fn non_tmp_dir() -> TempDir {
        if let Some(env_dir) = std::env::var_os("TMPDIR") {
            let p = std::path::PathBuf::from(env_dir);
            if !p.starts_with("/tmp")
                && !p.starts_with("/dev/shm")
                && fs::create_dir_all(&p).is_ok()
                && let Ok(td) = tempfile::tempdir_in(&p)
            {
                return td;
            }
        }
        if fs::create_dir_all("/data/tmp").is_ok()
            && let Ok(td) = tempfile::tempdir_in("/data/tmp")
        {
            return td;
        }
        tempfile::tempdir_in("/var/tmp").unwrap()
    }

    #[test]
    fn projection_frontier_reads_absent_state_without_initializing_storage() {
        let dir = non_tmp_dir();
        let layout = StorageLayout::new(dir.path().join("never-created")).unwrap();
        let read = || {
            CatalogStore::read_spindle_projection_frontier(
                &layout,
                Instant::now() + Duration::from_secs(1),
            )
            .unwrap()
        };
        assert_eq!(read(), SpindleProjectionFrontier::default());
        assert!(!layout.data_dir().exists());
        fs::create_dir_all(layout.state_dir()).unwrap();
        assert_eq!(read(), SpindleProjectionFrontier::default());
        assert_eq!(fs::read_dir(layout.state_dir()).unwrap().count(), 0);
        assert!(!layout.olap_dir().exists());
    }

    #[test]
    fn projection_binding_preserves_the_legacy_boundary_across_commits() {
        let dir = non_tmp_dir();
        let store = CatalogStore::open(dir.path()).unwrap();
        let read = || {
            CatalogStore::read_spindle_projection_frontier(
                store.layout(),
                Instant::now() + Duration::from_secs(1),
            )
        };
        store.set_watermark(SPINDLE_WATERMARK, 9).unwrap();
        assert_eq!(
            read().unwrap(),
            SpindleProjectionFrontier {
                origin: None,
                legacy_through: 9,
                record_id: 9,
            }
        );
        let bound = store.bind_spindle_projection("origin-a").unwrap();
        assert_eq!(bound.origin.as_deref(), Some("origin-a"));
        assert_eq!(bound.legacy_through, 9);
        assert_eq!(read().unwrap(), bound);

        store.set_watermark(SPINDLE_WATERMARK, 11).unwrap();
        assert!(
            store
                .bind_spindle_projection("origin-b")
                .unwrap_err()
                .contains("origin mismatch")
        );
        let before_read = fs::read(store.layout().catalog_redb()).unwrap();
        let current = read().unwrap();
        assert_eq!(
            current,
            SpindleProjectionFrontier {
                record_id: 11,
                ..bound
            }
        );
        assert_eq!(
            fs::read(store.layout().catalog_redb()).unwrap(),
            before_read
        );
        assert_eq!(store.bind_spindle_projection("origin-a").unwrap(), current);

        store.set_watermark(SPINDLE_WATERMARK, 8).unwrap();
        assert!(read().unwrap_err().contains("legacy boundary"));
    }

    #[test]
    fn projection_frontier_surfaces_catalog_contention_and_corruption() {
        let dir = non_tmp_dir();
        let store = CatalogStore::open(dir.path()).unwrap();
        store.bind_spindle_projection("origin-a").unwrap();
        let writer = store.open_db_with_retry().unwrap();
        let deadline = Instant::now() + Duration::from_millis(25);
        let err =
            CatalogStore::read_spindle_projection_frontier(store.layout(), deadline).unwrap_err();
        assert!(err.contains("deadline"), "{err}");
        assert!(Instant::now() >= deadline);
        drop(writer);

        let catalog = store.layout().catalog_redb();
        let displaced = catalog.with_extension("saved");
        fs::rename(&catalog, &displaced).unwrap();
        std::os::unix::fs::symlink(&displaced, &catalog).unwrap();
        let read = || {
            CatalogStore::read_spindle_projection_frontier(
                store.layout(),
                Instant::now() + Duration::from_secs(1),
            )
        };
        assert!(read().unwrap_err().contains("symlink"));
        fs::remove_file(&catalog).unwrap();
        fs::write(&catalog, b"not a database").unwrap();
        let err = read().unwrap_err();
        assert!(
            err.contains("failed reading Spindle projection frontier"),
            "{err}"
        );
        assert_eq!(fs::read(&catalog).unwrap(), b"not a database");
    }

    #[test]
    fn rejects_tmp_and_roundtrips_catalog_state() {
        assert!(CatalogStore::open("/tmp/bad-fabric-store").is_err());
        assert!(CatalogStore::open("/dev/shm/bad-fabric-store").is_err());

        let dir = non_tmp_dir();
        let store = CatalogStore::open(dir.path()).unwrap();
        let snap = HostSnapshot {
            timestamp_unix_ms: 1_700_000_000_000,
            hostname: "workstation-01".into(),
            ..Default::default()
        };
        let drift = evaluate_five_pillars(&snap, &Default::default(), None);
        let graph = CausalGraph::from_snapshot(&snap, &drift.findings);

        store.commit_cycle(&snap, &drift, &graph).unwrap();
        assert_eq!(
            store.read_watermark("collector_last_ms").unwrap(),
            Some(1_700_000_000_000)
        );
        let loaded = store.load_live_state().unwrap().unwrap();
        assert_eq!(loaded.snapshot.hostname, "workstation-01");
    }

    #[test]
    fn catalog_survives_cold_process_reopen() {
        use fabric_graph::{EdgeKind, GraphEdge, GraphNode, NodeKind};
        use std::{
            collections::BTreeMap,
            process::{Command, Stdio},
            time::{Duration, Instant},
        };

        let mut snapshot = HostSnapshot {
            timestamp_unix_ms: 1_700_000_000_123,
            hostname: "cold-catalog".into(),
            units: vec![fabric_core::UnitRecord {
                name: "example.service".into(),
                slice: "example.slice".into(),
                active_state: "active".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        if std::env::var_os("FABRIC_CATALOG_COLD_NEWER").is_some() {
            snapshot.timestamp_unix_ms += 1;
        }
        let drift = DriftReport::default();
        let mut graph = CausalGraph::new();
        graph.upsert_node("one", NodeKind::Agent, "One", BTreeMap::new());
        graph.upsert_node("two", NodeKind::SystemdUnit, "Two", BTreeMap::new());
        graph.add_edge("one", "two", EdgeKind::Controls, 1.0, "known edge");
        let tmp = std::path::PathBuf::from(
            std::env::var_os("TMPDIR").expect("disk-backed TMPDIR required"),
        );
        assert!(!tmp.starts_with("/tmp") && !tmp.starts_with("/dev/shm"));

        if let Some(root) = std::env::var_os("FABRIC_CATALOG_COLD_ROOT") {
            let store = CatalogStore::open(root).unwrap();
            let db = store.open_db_with_retry().unwrap();
            if std::env::var_os("FABRIC_CATALOG_COLD_ABANDON").is_some() {
                use std::io::Write;
                let txn = db.begin_write().unwrap();
                {
                    let mut entities = txn.open_table(ENTITIES_TABLE).unwrap();
                    entities.insert("one", b"uncommitted".as_slice()).unwrap();
                    let mut watermarks = txn.open_table(WATERMARKS_TABLE).unwrap();
                    watermarks.insert("collector_last_ms", 42).unwrap();
                }
                let marker =
                    std::env::var_os("FABRIC_CATALOG_COLD_MARKER").expect("completion marker");
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(marker)
                    .unwrap()
                    .write_all(b"staged-no-commit\n")
                    .unwrap();
                // Deliberately bypass transaction and database destructors.
                std::process::exit(0);
            }
            let txn = db.begin_read().unwrap();
            let entries = |table: TableDefinition<&str, &[u8]>| {
                txn.open_table(table)
                    .unwrap()
                    .iter()
                    .unwrap()
                    .map(|row| {
                        let (key, value) = row.unwrap();
                        (key.value().to_owned(), value.value().to_vec())
                    })
                    .collect::<BTreeMap<_, _>>()
            };
            let nodes = [
                GraphNode {
                    id: "one".into(),
                    kind: NodeKind::Agent,
                    label: "One".into(),
                    metadata: BTreeMap::new(),
                },
                GraphNode {
                    id: "two".into(),
                    kind: NodeKind::SystemdUnit,
                    label: "Two".into(),
                    metadata: BTreeMap::new(),
                },
            ];
            let expected_nodes = nodes
                .iter()
                .map(|node| (node.id.clone(), serde_json::to_vec(node).unwrap()))
                .collect();
            assert_eq!(entries(ENTITIES_TABLE), expected_nodes);
            let edge = GraphEdge {
                src: "one".into(),
                dst: "two".into(),
                kind: EdgeKind::Controls,
                weight: 1.0,
                note: "known edge".into(),
            };
            let edge_bytes = serde_json::to_vec(&vec![edge]).unwrap();
            assert_eq!(
                entries(ADJ_OUT_TABLE),
                BTreeMap::from([("one".into(), edge_bytes.clone())])
            );
            assert_eq!(
                entries(ADJ_IN_TABLE),
                BTreeMap::from([("two".into(), edge_bytes)])
            );
            assert_eq!(
                entries(UNITS_TABLE),
                BTreeMap::from([(
                    "example.service".into(),
                    serde_json::to_vec(&snapshot.units[0]).unwrap()
                )])
            );
            assert_eq!(
                txn.open_table(WATERMARKS_TABLE)
                    .unwrap()
                    .get("collector_last_ms")
                    .unwrap()
                    .unwrap()
                    .value(),
                snapshot.timestamp_unix_ms as u64
            );
            let live = store.load_live_state().unwrap().unwrap();
            assert_eq!(
                serde_json::to_vec(&live.snapshot).unwrap(),
                serde_json::to_vec(&snapshot).unwrap()
            );
            assert_eq!(live.graph, graph);
            use std::io::Write;
            let marker = std::env::var_os("FABRIC_CATALOG_COLD_MARKER").expect("completion marker");
            let mut marker = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(marker)
                .unwrap();
            marker.write_all(b"cold-reopen-ok\n").unwrap();
            return;
        }
        let dir = tempfile::tempdir_in(&tmp).unwrap();
        let run_child = |name: &str, marker: &str, newer: bool, abandon: bool| {
            let marker = dir.path().join(marker);
            let mut command = Command::new(std::env::current_exe().unwrap());
            if newer {
                command.env("FABRIC_CATALOG_COLD_NEWER", "1");
            }
            if abandon {
                command.env("FABRIC_CATALOG_COLD_ABANDON", "1");
            }
            let mut child = command
                .args(["--exact", name])
                .env("TMPDIR", &tmp)
                .env("FABRIC_CATALOG_COLD_ROOT", dir.path())
                .env("FABRIC_CATALOG_COLD_MARKER", &marker)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success(), "cold reopen failed: {status}");
                    break;
                }
                if Instant::now() >= deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("cold reopen timed out");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            fs::read(marker).is_ok_and(|bytes| {
                bytes
                    == if abandon {
                        b"staged-no-commit\n".as_slice()
                    } else {
                        b"cold-reopen-ok\n".as_slice()
                    }
            })
        };
        for iteration in 0..2 {
            let store = CatalogStore::open(dir.path()).unwrap();
            store.commit_cycle(&snapshot, &drift, &graph).unwrap();
            drop(store);
            if iteration == 0 {
                assert!(!run_child(
                    "catalog::tests::intentionally_missing_cold_test",
                    "absent.marker",
                    false,
                    false
                ));
            }
            assert!(run_child(
                "catalog::tests::catalog_survives_cold_process_reopen",
                &format!("verified-{iteration}.marker"),
                false,
                false,
            ));
            if iteration == 0 {
                assert!(run_child(
                    "catalog::tests::catalog_survives_cold_process_reopen",
                    "abandoned.marker",
                    false,
                    true
                ));
                assert!(run_child(
                    "catalog::tests::catalog_survives_cold_process_reopen",
                    "after-abandon.marker",
                    false,
                    false
                ));
            }
        }
        snapshot.timestamp_unix_ms += 1;
        let store = CatalogStore::open(dir.path()).unwrap();
        store.commit_cycle(&snapshot, &drift, &graph).unwrap();
        drop(store);
        assert!(run_child(
            "catalog::tests::catalog_survives_cold_process_reopen",
            "newer.marker",
            true,
            false
        ));
    }

    #[test]
    fn failed_commit_sync_never_acknowledges_or_mixes_catalog_tables() {
        use fabric_graph::{EdgeKind, NodeKind};
        use std::{
            collections::BTreeMap,
            io::Write,
            process::{Command, Stdio},
            sync::{Arc, atomic::Ordering},
            time::{Duration, Instant},
        };
        fn fixture(newer: bool) -> (HostSnapshot, DriftReport, CausalGraph) {
            let snapshot = HostSnapshot {
                timestamp_unix_ms: if newer {
                    1_700_000_000_124
                } else {
                    1_700_000_000_123
                },
                hostname: if newer { "new" } else { "old" }.into(),
                units: vec![fabric_core::UnitRecord {
                    name: "test.service".into(),
                    active_state: if newer { "failed" } else { "active" }.into(),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let mut graph = CausalGraph::new();
            graph.upsert_node(
                "one",
                NodeKind::Agent,
                if newer { "new" } else { "old" },
                BTreeMap::new(),
            );
            graph.upsert_node("two", NodeKind::SystemdUnit, "two", BTreeMap::new());
            graph.add_edge(
                "one",
                "two",
                EdgeKind::Controls,
                1.0,
                if newer { "new" } else { "old" },
            );
            (snapshot, DriftReport::default(), graph)
        }
        let tmp = std::path::PathBuf::from(std::env::var_os("TMPDIR").expect("disk-backed TMPDIR"));
        assert!(!tmp.starts_with("/tmp") && !tmp.starts_with("/dev/shm"));
        if let Some(root) = std::env::var_os("FABRIC_CATALOG_SYNC_ROOT") {
            let store = CatalogStore::open(root).unwrap();
            let db = store.open_db_with_retry().expect("fresh recovery open");
            let txn = db.begin_read().expect("fresh recovery read transaction");
            let actual_watermark = txn
                .open_table(WATERMARKS_TABLE)
                .unwrap()
                .get("collector_last_ms")
                .unwrap()
                .unwrap()
                .value();
            let newer = match actual_watermark {
                1_700_000_000_123 => false,
                1_700_000_000_124 => true,
                _ => panic!("unexpected recovered watermark: {actual_watermark}"),
            };
            let (snapshot, drift, graph) = fixture(newer);
            let entries = |table: TableDefinition<&str, &[u8]>| {
                txn.open_table(table)
                    .unwrap()
                    .iter()
                    .unwrap()
                    .map(|entry| {
                        let (key, value) = entry.unwrap();
                        (key.value().to_owned(), value.value().to_vec())
                    })
                    .collect::<BTreeMap<_, _>>()
            };
            let expected_entities = graph
                .nodes()
                .iter()
                .map(|(key, node)| (key.clone(), serde_json::to_vec(node).unwrap()))
                .collect();
            assert_eq!(entries(ENTITIES_TABLE), expected_entities);
            assert_eq!(
                entries(UNITS_TABLE),
                BTreeMap::from([(
                    "test.service".into(),
                    serde_json::to_vec(&snapshot.units[0]).unwrap()
                )])
            );
            let edges = serde_json::to_vec(&graph.edges()).unwrap();
            assert_eq!(
                entries(ADJ_OUT_TABLE),
                BTreeMap::from([("one".into(), edges.clone())])
            );
            assert_eq!(
                entries(ADJ_IN_TABLE),
                BTreeMap::from([("two".into(), edges)])
            );
            let meta = txn.open_table(META_TABLE).unwrap();
            assert_eq!(
                meta.get("latest_snapshot").unwrap().unwrap().value(),
                serde_json::to_vec(&snapshot).unwrap()
            );
            assert_eq!(
                meta.get("latest_graph").unwrap().unwrap().value(),
                serde_json::to_vec(&graph).unwrap()
            );
            assert_eq!(
                entries(ANOMALIES_TABLE).get("latest_drift").unwrap(),
                &serde_json::to_vec(&drift).unwrap()
            );
            let marker = std::env::var_os("FABRIC_CATALOG_SYNC_MARKER").expect("completion marker");
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(marker)
                .unwrap()
                .write_all(if newer { b"new\n" } else { b"old\n" })
                .unwrap();
            return;
        }
        let dir = tempfile::tempdir_in(&tmp).unwrap();
        let store = CatalogStore::open(dir.path()).unwrap();
        let (old_snapshot, old_drift, old_graph) = fixture(false);
        store
            .commit_cycle(&old_snapshot, &old_drift, &old_graph)
            .unwrap();
        let mirror = fs::read(store.layout().live_state_json()).unwrap();
        let control = Arc::new(sync_fault::Control::default());
        sync_fault::SELECTED.with(|selected| *selected.borrow_mut() = Some(Arc::clone(&control)));
        let (new_snapshot, new_drift, new_graph) = fixture(true);
        let failed = store.commit_cycle(&new_snapshot, &new_drift, &new_graph);
        sync_fault::SELECTED.with(|selected| *selected.borrow_mut() = None);
        let error = failed.expect_err("failed sync must not acknowledge commit");
        assert!(
            error.contains("injected catalog commit sync failure"),
            "{error}"
        );
        assert_eq!(control.fired.load(Ordering::SeqCst), 1);
        assert!(!control.armed.load(Ordering::SeqCst));
        assert_eq!(fs::read(store.layout().live_state_json()).unwrap(), mirror);
        drop(store);
        let marker = dir.path().join("recovered.marker");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "catalog::tests::failed_commit_sync_never_acknowledges_or_mixes_catalog_tables",
            ])
            .env("TMPDIR", &tmp)
            .env("FABRIC_CATALOG_SYNC_ROOT", dir.path())
            .env("FABRIC_CATALOG_SYNC_MARKER", &marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "fresh recovery failed: {status}");
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("fresh recovery timed out");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(matches!(
            fs::read(marker).unwrap().as_slice(),
            b"old\n" | b"new\n"
        ));
    }

    #[test]
    fn commit_cycle_updates_existing_entity_bytes() {
        let dir = non_tmp_dir();
        let store = CatalogStore::open(dir.path()).unwrap();
        let mut snapshot = HostSnapshot {
            timestamp_unix_ms: 1_700_000_000_000,
            hostname: "fixed".into(),
            units: vec![fabric_core::UnitRecord {
                name: "same.scope".into(),
                slice: "fixed.slice".into(),
                active_state: "active".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let drift = DriftReport::default();
        let first = CausalGraph::from_snapshot(&snapshot, &[]);
        store.commit_cycle(&snapshot, &drift, &first).unwrap();
        snapshot.units[0].active_state = "failed".into();
        let updated = CausalGraph::from_snapshot(&snapshot, &[]);
        assert_ne!(
            first.nodes()["unit:same.scope"],
            updated.nodes()["unit:same.scope"]
        );
        store.commit_cycle(&snapshot, &drift, &updated).unwrap();
        let db = store.open_db_with_retry().unwrap();
        let txn = db.begin_read().unwrap();
        let table = txn.open_table(ENTITIES_TABLE).unwrap();
        let actual = table.get("unit:same.scope").unwrap().unwrap();
        assert_eq!(
            actual.value(),
            serde_json::to_vec(&updated.nodes()["unit:same.scope"]).unwrap()
        );
    }

    #[test]
    fn commit_cycle_prunes_stale_units_and_entities() {
        let dir = non_tmp_dir();
        let store = CatalogStore::open(dir.path()).unwrap();

        let snap1 = HostSnapshot {
            timestamp_unix_ms: 1_700_000_000_000,
            hostname: "workstation-01".into(),
            units: vec![fabric_core::UnitRecord {
                name: "ephemeral-1.scope".into(),
                manager: "user".into(),
                slice: "agent.slice".into(),
                active_state: "failed".into(),
                sub_state: "failed".into(),
                transient: true,
                main_pid: Some(12345),
                memory_current_bytes: 1024,
                ..Default::default()
            }],
            ..Default::default()
        };
        let drift1 = evaluate_five_pillars(&snap1, &Default::default(), None);
        let graph1 = CausalGraph::from_snapshot(&snap1, &drift1.findings);
        store.commit_cycle(&snap1, &drift1, &graph1).unwrap();

        {
            let db = store.open_db_with_retry().unwrap();
            let rtxn = db.begin_read().unwrap();
            assert!(
                rtxn.open_table(UNITS_TABLE)
                    .unwrap()
                    .get("ephemeral-1.scope")
                    .unwrap()
                    .is_some()
            );
            assert!(
                rtxn.open_table(ENTITIES_TABLE)
                    .unwrap()
                    .get("unit:ephemeral-1.scope")
                    .unwrap()
                    .is_some()
            );
        }

        let snap2 = HostSnapshot {
            timestamp_unix_ms: 1_700_000_002_000,
            hostname: "workstation-01".into(),
            ..Default::default()
        };
        let drift2 = evaluate_five_pillars(&snap2, &Default::default(), None);
        let graph2 = CausalGraph::from_snapshot(&snap2, &drift2.findings);
        store.commit_cycle(&snap2, &drift2, &graph2).unwrap();

        let db = store.open_db_with_retry().unwrap();
        let rtxn = db.begin_read().unwrap();
        assert!(
            rtxn.open_table(UNITS_TABLE)
                .unwrap()
                .get("ephemeral-1.scope")
                .unwrap()
                .is_none()
        );
        assert!(
            rtxn.open_table(ENTITIES_TABLE)
                .unwrap()
                .get("unit:ephemeral-1.scope")
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_catalog_redb_and_live_state_into_tmp() {
        use std::os::unix::fs::symlink;

        let dir = non_tmp_dir();
        let layout = StorageLayout::new(dir.path()).unwrap();
        fs::create_dir_all(layout.state_dir()).unwrap();
        symlink("/tmp/fabrico11y-evil-catalog.redb", layout.catalog_redb()).unwrap();

        let Err(err) = CatalogStore::open(dir.path()) else {
            panic!("symlinked catalog.redb into /tmp must be rejected");
        };
        assert!(
            err.contains("refusing") || err.contains("/tmp"),
            "unexpected error: {err}"
        );

        fs::remove_file(layout.catalog_redb()).unwrap();
        symlink("/tmp/fabrico11y-evil-live.json", layout.live_state_json()).unwrap();
        let Err(err) = CatalogStore::open(dir.path()) else {
            panic!("symlinked live_state.json into /tmp must be rejected");
        };
        assert!(
            err.contains("refusing") || err.contains("/tmp"),
            "unexpected error: {err}"
        );

        fs::remove_file(layout.live_state_json()).unwrap();
        let store = CatalogStore::open(dir.path()).unwrap();
        fs::remove_dir_all(layout.staging_dir()).unwrap();
        let evil_staging = std::path::PathBuf::from(format!(
            "/tmp/fabrico11y-evil-staging-{}",
            std::process::id()
        ));
        symlink(&evil_staging, layout.staging_dir()).unwrap();

        let snap = HostSnapshot {
            timestamp_unix_ms: 1_700_000_003_000,
            hostname: "workstation-01".into(),
            ..Default::default()
        };
        let drift = evaluate_five_pillars(&snap, &Default::default(), None);
        let graph = CausalGraph::from_snapshot(&snap, &drift.findings);
        let err = store
            .commit_cycle(&snap, &drift, &graph)
            .expect_err("post-open symlinked staging dir into /tmp must be rejected");
        assert!(
            err.contains("symlink") || err.contains("refusing") || err.contains("/tmp"),
            "unexpected error: {err}"
        );
        assert!(
            !evil_staging.exists(),
            "commit_cycle must never create or write inside symlinked /tmp staging dir"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_staged_then_renameat_rejects_dir_swap_and_symlinked_target() {
        use std::os::unix::fs::symlink;

        let dir = non_tmp_dir();
        let store = CatalogStore::open(dir.path()).unwrap();
        let layout = StorageLayout::new(dir.path()).unwrap();
        let snap = HostSnapshot {
            timestamp_unix_ms: 1_700_000_003_500,
            hostname: "workstation-atomic".into(),
            ..Default::default()
        };
        let drift = evaluate_five_pillars(&snap, &Default::default(), None);
        let graph = CausalGraph::from_snapshot(&snap, &drift.findings);
        store.commit_cycle(&snap, &drift, &graph).unwrap();
        let state = store.load_live_state().unwrap().unwrap();

        // Replace state dir with symlink inside data_dir after layout creation.
        let decoy_dir = dir.path().join("decoy");
        fs::create_dir_all(&decoy_dir).unwrap();
        let data_fd =
            std::os::fd::OwnedFd::from(crate::olap::open_anchor_data_dir(&layout).unwrap());
        let state_bytes = serde_json::to_vec(&state).unwrap();

        fs::remove_dir_all(layout.state_dir()).unwrap();
        symlink(&decoy_dir, layout.state_dir()).unwrap();
        let err = crate::atomic_fd::write_staged_then_renameat(
            layout.data_dir(),
            &data_fd,
            "staging",
            "state",
            "live_state.swap.json",
            "live_state.json",
            &state_bytes,
        )
        .expect_err("symlinked state dir must be refused via openat O_NOFOLLOW");
        assert!(err.contains("refusing symlinked"), "unexpected err: {err}");
        assert!(!decoy_dir.join("live_state.json").exists());

        // Restore real state dir and symlink target live_state.json -> /tmp.
        fs::remove_file(layout.state_dir()).unwrap();
        fs::create_dir_all(layout.state_dir()).unwrap();
        let evil_target = std::path::PathBuf::from(format!(
            "/tmp/fabrico11y-renameat-target-{}",
            std::process::id()
        ));
        symlink(&evil_target, layout.live_state_json()).unwrap();
        let err = store
            .write_live_state_atomic(&state)
            .expect_err("symlinked live_state.json must be refused before renameat");
        assert!(err.contains("refusing"), "unexpected err: {err}");
        assert!(!evil_target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn custody_dirs_and_files_enforce_private_modes_under_permissive_umask() {
        use std::os::unix::fs::PermissionsExt;

        if std::env::var_os("FABRIC_STORE_UMASK_SUBPROCESS").is_none() {
            let exe = std::env::current_exe().expect("test binary path");
            let status = std::process::Command::new("sh")
                .args([
                    "-c",
                    "umask 000 && exec \"$1\" --exact \"catalog::tests::custody_dirs_and_files_enforce_private_modes_under_permissive_umask\"",
                    "sh",
                    &exe.to_string_lossy(),
                ])
                .env("FABRIC_STORE_UMASK_SUBPROCESS", "1")
                .status()
                .expect("spawn umask 000 child");
            assert!(status.success(), "umask 000 child test failed");
            return;
        }

        let dir = non_tmp_dir();
        let custody_root = dir.path().join("custody");
        let layout = StorageLayout::new(&custody_root).unwrap();
        let _lock = crate::olap::try_acquire_writer_lock(&layout).unwrap();
        let store = CatalogStore::open(&custody_root).unwrap();
        let snap = HostSnapshot {
            timestamp_unix_ms: 1_700_000_004_000,
            hostname: "workstation-umask".into(),
            ..Default::default()
        };
        let drift = evaluate_five_pillars(&snap, &Default::default(), None);
        let graph = CausalGraph::from_snapshot(&snap, &drift.findings);
        store.commit_cycle(&snap, &drift, &graph).unwrap();

        let mode_of = |p: &Path| fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode_of(layout.data_dir()), 0o700);
        assert_eq!(mode_of(&layout.state_dir()), 0o700);
        assert_eq!(mode_of(&layout.staging_dir()), 0o700);
        assert_eq!(mode_of(&layout.catalog_redb()), 0o600);
        assert_eq!(mode_of(&layout.live_state_json()), 0o600);
        assert_eq!(mode_of(&layout.state_dir().join("daemon.lock")), 0o600);
    }
}
