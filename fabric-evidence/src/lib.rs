//! Lightweight Sans-I/O evidence contracts shared by producers and consumers.
pub mod application_result;
pub mod attempt_observation;
pub mod capture_policy;
pub mod durable_snapshot;
pub mod evidence_manifest;
pub mod frontier;
pub mod snapshot;
pub mod worker_segment;

pub use application_result::{
    ApplicationPayloadDigest, ApplicationResult, ApplicationResultError, ReportScope,
};
