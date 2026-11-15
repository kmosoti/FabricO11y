//! ACID MVCC OLTP & hot adjacency graph storage backed by `redb` (`3.1.3`),
//! paired with lock-free atomic JSON live state snapshots for concurrent CLI reads.

use chrono::Utc;
use fabric_core::{
    DriftReport, HostSnapshot, PathPolicyError, SCHEMA_VERSION, StorageLayout,
    validate_non_tmp_path,
};
use fabric_graph::CausalGraph;
use redb::{Builder, Database, DatabaseError, ReadableDatabase, TableDefinition};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ENTITIES_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("entities");
const ADJ_OUT_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("adj_out");
const ADJ_IN_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("adj_in");
const UNITS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("units");
const WATERMARKS_TABLE: TableDefinition<&str, u64> = TableDefinition::new("watermarks");
const ANOMALIES_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("anomalies");
const META_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");

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

/// Converged `redb` OLTP + adjacency catalog rooted under a validated non-tmpfs directory.
#[derive(Debug, Clone)]
pub struct CatalogStore {
    layout: StorageLayout,
}

impl CatalogStore {
    /// Initializes storage directories beneath `data_dir` (refusing `/tmp` or `/dev/shm`).
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, String> {
        let layout = StorageLayout::new(data_dir).map_err(|e: PathPolicyError| e.to_string())?;
        Self::ensure_layout(&layout)?;
        Ok(Self { layout })
    }

    #[must_use]
    pub fn layout(&self) -> &StorageLayout {
        &self.layout
    }

    fn ensure_layout(layout: &StorageLayout) -> Result<(), String> {
        for dir in [
            layout.data_dir().to_path_buf(),
            layout.state_dir(),
            layout.staging_dir(),
            layout.spill_dir(),
            layout.olap_dir(),
        ] {
            validate_non_tmp_path(&dir).map_err(|e| e.to_string())?;
            fs::create_dir_all(&dir)
                .map_err(|e| format!("failed creating {}: {e}", dir.display()))?;
        }
        for domain in ["metrics", "logs", "traces", "fs_events", "edges"] {
            let domain_dir = layout.olap_domain_dir(domain);
            fs::create_dir_all(&domain_dir)
                .map_err(|e| format!("failed creating {}: {e}", domain_dir.display()))?;
        }
        Ok(())
    }

    fn open_db_with_retry(&self) -> Result<Database, String> {
        let db_path = self.layout.catalog_redb();
        for attempt in 0..OPEN_RETRY_ATTEMPTS {
            let mut builder = Builder::new();
            builder.set_cache_size(REDB_CACHE_BYTES);
            match builder.create(&db_path) {
                Ok(db) => return Ok(db),
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
            let mut entities = write_txn
                .open_table(ENTITIES_TABLE)
                .map_err(|e| e.to_string())?;
            for (id, node) in graph.nodes() {
                let bytes = serde_json::to_vec(node).map_err(|e| e.to_string())?;
                entities
                    .insert(id.as_str(), bytes.as_slice())
                    .map_err(|e| e.to_string())?;
            }

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
        if !self.layout.catalog_redb().exists() {
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

    /// Reads the latest persisted snapshot, drift report, and causal graph.
    pub fn load_live_state(&self) -> Result<Option<PersistedLiveState>, String> {
        let json_path = self.layout.live_state_json();
        if json_path.is_file() {
            let raw = fs::read_to_string(&json_path)
                .map_err(|e| format!("failed reading {}: {e}", json_path.display()))?;
            let state: PersistedLiveState =
                serde_json::from_str(&raw).map_err(|e| e.to_string())?;
            return Ok(Some(state));
        }

        if !self.layout.catalog_redb().exists() {
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
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let tmp_file = self
            .layout
            .staging_dir()
            .join(format!("live_state.{}.{stamp}.json", std::process::id()));
        let bytes = serde_json::to_vec(state).map_err(|e| e.to_string())?;
        let mut f = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp_file)
            .map_err(|e| format!("failed creating {}: {e}", tmp_file.display()))?;
        f.write_all(&bytes).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&tmp_file, self.layout.live_state_json())
            .map_err(|e| format!("failed renaming live state: {e}"))?;
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
    fn rejects_tmp_and_roundtrips_catalog_state() {
        assert!(CatalogStore::open("/tmp/bad-fabric-store").is_err());
        assert!(CatalogStore::open("/dev/shm/bad-fabric-store").is_err());

        let dir = non_tmp_dir();
        let store = CatalogStore::open(dir.path()).unwrap();
        let snap = HostSnapshot {
            timestamp_unix_ms: 1_700_000_000_000,
            hostname: "fedora-blade".into(),
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
        assert_eq!(loaded.snapshot.hostname, "fedora-blade");
    }
}
