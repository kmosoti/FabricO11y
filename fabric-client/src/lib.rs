#![forbid(unsafe_code)]
//! Zero-UI typed client and `fabric.cli.v1` JSON schemas for `FabricO11y`.
//!
//! Consumers can depend on `fabric-client` with `default-features = false` for
//! pure schema deserialization and helpers, or keep the default `cli` feature
//! for non-blocking [`FabricClient`] (with persistent [`FabricTransport::DirectSocket`]
//! and CLI fallback) and [`FabricQueryClient`] wrappers sharing canonical `fabric`
//! wire types.

#[cfg(feature = "cli")]
mod cli_exec;
#[cfg(feature = "cli")]
pub mod client;
#[cfg(feature = "cli")]
pub mod query_client;
pub mod schema;
pub mod schema_ext;
#[cfg(feature = "cli")]
pub mod transport;

#[cfg(feature = "cli")]
pub use client::FabricClient;
#[cfg(feature = "cli")]
pub use fabric::canonical::{
    CanonicalRow, ProjectionReadiness, ProjectionReason, ProjectionStatus,
    Request as CanonicalRequest, Response as CanonicalResponse,
};
#[cfg(feature = "cli")]
pub use fabric::query::{
    Budgets, Coverage, Filter, Operation, QueryPlan, QueryRequest, QueryResponse,
};
#[cfg(feature = "cli")]
pub use fabric::spindle::{
    Health as SpindleHealth, Reply as SpindleReply, Request as SpindleRequest,
};
#[cfg(feature = "cli")]
pub use query_client::FabricQueryClient;
pub use schema::{
    EXPECTED_SCHEMA_VERSION, FabricAgent, FabricBlame, FabricBlameEntry, FabricBundle,
    FabricCustody, FabricDoctor, FabricDoctorCheck, FabricDrift, FabricDriftFinding,
    FabricEnvelope, FabricGpu, FabricLineage, FabricLineageEdge, FabricLineageNode, FabricMemory,
    FabricMetricQuery, FabricMetricRow, FabricMount, FabricPsi, FabricSelfHealth, FabricSlice,
    FabricSnapshot, FabricZram, PsiStat, custody_lag, format_bytes_iec, mem_used_pct,
    parse_envelope,
};
#[cfg(feature = "cli")]
pub use transport::FabricTransport;
