//! Property-based tests of the `fabric-core` kernels live in `tests/`.
//!
//! This crate is in the verification layer ([layers.json]): it may depend on
//! any product crate and no product crate may depend on it, so the core's
//! zero-dependency policy is unchanged. Each property states a contract from
//! an ADR or architecture view independently of the kernel's code, and a
//! registered semantic mutant (`xtask/mutants.json`) shows it can fail.
//!
//! [layers.json]: ../../docs/architecture/layers.json
