//! Sans-I/O core domain models, path policy enforcement, statistical sketches,
//! Five Data Observability Pillar evaluators, and Arrow IPC schemas.

pub mod anomaly;
pub mod arrow_schemas;
pub mod evidence_manifest;
pub mod guide;
pub mod model;
pub mod paths;
pub mod query_spec;
pub mod ring;
pub mod sketch;
pub mod terminal;

pub use anomaly::{
    AnomalyScore, DriftContext, DriftReport, EwmaMadDetector, PillarFinding, PillarKind, Severity,
    evaluate_five_pillars,
};
pub use arrow_schemas::{
    BLAME_SCHEMA, FS_EVENTS_SCHEMA, GRAPH_EDGES_SCHEMA, LINEAGE_NODES_SCHEMA, LOGS_SCHEMA,
    METRICS_SCHEMA, PILLAR_FINDINGS_SCHEMA, SNAPSHOT_SLICES_SCHEMA, TRACES_SCHEMA, UNITS_SCHEMA,
    agents_to_record_batch, blame_to_record_batch, doctor_to_record_batch, drift_to_record_batch,
    encode_batches_to_ipc_stream, fs_churn_to_record_batch, fs_events_to_record_batch,
    graph_edges_to_record_batch, logs_to_record_batch, metrics_to_record_batch,
    snapshot_to_record_batch, traces_to_record_batch, units_to_record_batch,
};
pub use evidence_manifest::{
    ArtifactContentDigest, ArtifactKind, ArtifactReference, AttemptManifest, AttemptManifestDigest,
    AttemptOverride, CanonicalRecordDigest, ClockProvenance, DigestAlgorithm, DigestClaim,
    DigestRepresentation, Disclosure, ImmutableReference, MANIFEST_SCHEMA_VERSION,
    MAX_MANIFEST_WIRE_BYTES, ManifestEnvelope, ManifestError, ObservationReference,
    PhysicalFileDigest, ProvenancedTime, RunManifest, RunManifestDigest,
};
pub use model::{
    AgentSessionRecord, BlameEntry, BlameReport, BlameResource, CgroupSample, CliEnvelope,
    DoctorCheck, DoctorReport, FsChurnEntry, FsEventRecord, GpuDeviceStats, GraphEdgeRecord,
    HostSnapshot, LogRecord, MemorySnapshot, MetricPoint, MountUsage, ProcessSample, PsiMetrics,
    PsiResource, SCHEMA_VERSION, SPINDLE_PROVENANCE_COLUMN, SelfHealthSummary, SliceSummary,
    SpindleProvenance, TraceSpanRecord, UnitRecord, ZramStats,
};
pub use paths::{
    DEFAULT_DATA_DIR, DEFAULT_TMP_DIR, PathPolicyError, StorageLayout, normalize_lexical_path,
    validate_non_tmp_path,
};
pub use query_spec::{QueryDomain, UnifiedQuerySpec, build_unified_sql, parse_duration_ms};
pub use ring::{
    DEFAULT_RING_BUDGET_BYTES, HotMetricRing, ROLLUP_BUCKET_MS, RingStats, align_rollup_bucket_ms,
    roundtrip_series_block,
};
pub use sketch::DDSketch;
pub use terminal::sanitize_terminal_text;
