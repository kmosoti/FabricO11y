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
use std::io::{ErrorKind, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ENTITIES_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("entities");
const ADJ_OUT_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("adj_out");
const ADJ_IN_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("adj_in");
const UNITS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("units");
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
            let mut builder = Builder::new();
            builder.set_cache_size(REDB_CACHE_BYTES);
            match builder.create_file(file) {
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
            // Live-snapshot tables reflect only active entities/units/edges; replace cleanly each cycle.
            let _ = write_txn.delete_table(ENTITIES_TABLE);
            let mut entities = write_txn
                .open_table(ENTITIES_TABLE)
                .map_err(|e| e.to_string())?;
            for (id, node) in graph.nodes() {
                let bytes = serde_json::to_vec(node).map_err(|e| e.to_string())?;
                entities
                    .insert(id.as_str(), bytes.as_slice())
                    .map_err(|e| e.to_string())?;
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
        let staging_dir =
            crate::olap::ensure_durable_layout_subdir(&self.layout, &self.layout.staging_dir())?;
        let state_dir =
            crate::olap::ensure_durable_layout_subdir(&self.layout, &self.layout.state_dir())?;
        let staging_meta_before = fs::symlink_metadata(&staging_dir).map_err(|e| {
            format!(
                "failed inspecting staging dir {}: {e}",
                staging_dir.display()
            )
        })?;
        let state_meta_before = fs::symlink_metadata(&state_dir)
            .map_err(|e| format!("failed inspecting state dir {}: {e}", state_dir.display()))?;
        let live_state_path = self.layout.live_state_json();
        crate::olap::reject_symlinks_below_stop(Some(self.layout.data_dir()), &live_state_path)?;
        let validated_live_state =
            crate::validate_non_tmp_path_on_disk(&live_state_path).map_err(|e| e.to_string())?;

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let tmp_file = staging_dir.join(format!("live_state.{}.{stamp}.json", std::process::id()));
        let bytes = serde_json::to_vec(state).map_err(|e| e.to_string())?;
        {
            let mut f = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&tmp_file)
                .map_err(|e| format!("failed creating {}: {e}", tmp_file.display()))?;
            if let Err(e) = f.write_all(&bytes).and_then(|()| f.sync_all()) {
                drop(f);
                let _ = fs::remove_file(&tmp_file);
                return Err(e.to_string());
            }
        }
        let dir_identity_check = || -> Result<(), String> {
            let staging_meta_now = fs::symlink_metadata(&staging_dir).map_err(|e| {
                format!(
                    "failed re-inspecting staging dir {}: {e}",
                    staging_dir.display()
                )
            })?;
            let state_meta_now = fs::symlink_metadata(&state_dir).map_err(|e| {
                format!(
                    "failed re-inspecting state dir {}: {e}",
                    state_dir.display()
                )
            })?;
            if staging_meta_now.file_type().is_symlink()
                || !staging_meta_now.is_dir()
                || staging_meta_now.dev() != staging_meta_before.dev()
                || staging_meta_now.ino() != staging_meta_before.ino()
            {
                return Err(format!(
                    "staging dir {} changed concurrently during live state write",
                    staging_dir.display()
                ));
            }
            if state_meta_now.file_type().is_symlink()
                || !state_meta_now.is_dir()
                || state_meta_now.dev() != state_meta_before.dev()
                || state_meta_now.ino() != state_meta_before.ino()
            {
                return Err(format!(
                    "state dir {} changed concurrently during live state write",
                    state_dir.display()
                ));
            }
            Ok(())
        };
        if let Err(e) =
            crate::olap::reject_symlinks_below_stop(Some(self.layout.data_dir()), &live_state_path)
                .and_then(|()| {
                    crate::olap::reject_symlinks_below_stop(
                        Some(self.layout.data_dir()),
                        &self.layout.staging_dir(),
                    )
                })
                .and_then(|()| {
                    crate::validate_non_tmp_path_on_disk(&live_state_path)
                        .map_err(|err| err.to_string())
                })
                .and_then(|_| dir_identity_check())
        {
            let _ = fs::remove_file(&tmp_file);
            return Err(e);
        }
        if let Err(e) = fs::rename(&tmp_file, &validated_live_state) {
            let _ = fs::remove_file(&tmp_file);
            return Err(format!("failed renaming live state: {e}"));
        }
        dir_identity_check()?;
        crate::olap::sync_directory(&state_dir)?;
        Ok(())
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
}
