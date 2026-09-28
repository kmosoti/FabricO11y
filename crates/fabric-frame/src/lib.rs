//! Adapter support shared by the Spindle's Spool and the server journal: the
//! `FAB1` rotating frame log with its two-sync commit, and the version-one
//! `Batch` envelope. Both are infrastructure and protocol formats, not domain
//! semantics; the Strand and delivery rules live in `fabric-core`.

pub mod envelope;
pub mod frame;
