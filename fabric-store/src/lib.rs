//! Pure-Rust converged telemetry engine:
//! - `CatalogStore`: `redb` ACID MVCC OLTP/graph catalog + atomic `live_state.json`
//! - `HotMetricRing`: `<= 32 MiB` Gorilla delta/XOR compressed hot metric ring
//! - `OlapParquetStore`: ZSTD date-partitioned Arrow/Parquet store
//! - `RetentionJanitor`: bounded disk cap (`4 GiB` default), compaction, and downsampling

pub mod catalog;
pub mod janitor;
pub mod olap;
pub mod ring;

pub use catalog::{CatalogStore, PersistedLiveState};
pub use janitor::{JanitorConfig, JanitorStats, RetentionJanitor};
pub use olap::{OLAP_DOMAINS, OlapParquetStore};
pub use ring::{DEFAULT_RING_BUDGET_BYTES, HotMetricRing, RingStats};
