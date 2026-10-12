//! The Spindle: host-resident collection, the durable Spool and delivery to
//! Fabric Server.
pub mod meter;
pub mod otlp;
pub mod runtime;
pub mod sender;
pub mod spool;

// The Linux effects live in the adapter crate; these paths stay stable.
pub use fabric_adapter_linux::{host, log_source};
