//! The Spindle: host-resident collection, the durable Spool and delivery to
//! Fabric Server. The FOL2 demonstration in the crate root stays independent.
pub mod host;
pub mod log_source;
pub mod runtime;
pub mod sender;
pub mod spool;
