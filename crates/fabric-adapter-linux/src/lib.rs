//! The Spindle's Linux collection adapter: bounded reads of `/proc`,
//! `statvfs` and selected newline log files. What a counter sample or a log
//! cursor means is decided by `fabric_core::collection`; this crate performs
//! the reads and checksums.

pub mod host;
pub mod log_source;
pub mod operational_log;
