//! Pure-Rust converged telemetry engine:
//! - `CatalogStore`: `redb` ACID MVCC OLTP/graph catalog + atomic `live_state.json`
//! - `HotMetricRing`: `<= 32 MiB` Gorilla delta/XOR compressed hot metric ring
//! - `OlapParquetStore`: ZSTD date-partitioned Arrow/Parquet store
//! - `RetentionJanitor`: bounded disk cap (`4 GiB` default), compaction, and downsampling

mod atomic_fd;
pub mod catalog;
pub mod janitor;
pub mod olap;
pub mod parquet_guard;
pub mod paths;
pub mod restored_evidence;
pub mod ring;
pub mod worker_capture;

pub use catalog::{CatalogStore, PersistedLiveState, SpindleProjectionFrontier};
pub use janitor::{JanitorConfig, JanitorStats, RetentionJanitor};
pub use olap::{
    MAX_SNAPSHOT_METADATA_BYTES, MAX_SNAPSHOT_PARQUET_FILES, OLAP_DOMAINS, OlapFlushPlan,
    OlapParquetStore, PinnedParquetFile, WriterLockGuard, clear_committed_uncommitted,
    ensure_durable_layout_subdir, ensure_durable_non_tmp_dir, merge_uncommitted_partitions,
    save_uncommitted_partitions, snapshot_partitions_retained_heap_bytes,
    snapshot_retained_heap_bytes, try_acquire_writer_lock,
};
pub use parquet_guard::MAX_PARQUET_FOOTER_SERIALIZED_BYTES;
pub use paths::{validate_non_tmp_path_on_disk, validated_storage_layout};
pub use ring::{DEFAULT_RING_BUDGET_BYTES, HotMetricRing, RingStats};
pub use worker_capture::{
    ArchiveError as WorkerCaptureArchiveError, ArchiveReceipt as WorkerCaptureArchiveReceipt,
    ArchiveReplay as WorkerCaptureArchiveReplay, AttemptKind as WorkerCaptureAttemptKind,
    AttemptOutcome as WorkerCaptureAttemptOutcome, AttemptedEvent as WorkerCaptureAttemptedEvent,
    MAX_WORKER_ARCHIVE_ATTEMPTS, MAX_WORKER_ARCHIVE_BYTES, MAX_WORKER_ARCHIVE_EVENT_BYTES,
    replay_archive as replay_worker_capture_archive, write_archive as write_worker_capture_archive,
};
