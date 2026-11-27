//! Lightweight Sans-I/O evidence contracts shared by producers and consumers.
pub mod application_result;
pub mod durable_snapshot;
pub mod evidence_manifest;
pub mod snapshot;

pub use application_result::{
    ApplicationPayloadDigest, ApplicationResult, ApplicationResultError, ReportScope,
};
