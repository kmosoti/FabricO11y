//! FabricO11y's semantic core.
//!
//! Functions here decide what a transition means. They take every input
//! explicitly and return decisions as data; they never read a clock, the
//! environment, the filesystem or the network, and they never perform the
//! effects they describe. `no_std` makes the standard library's ambient
//! effects unavailable to this crate; `cargo xtask check-core-purity` checks
//! the dependency set, build script, features and a few source patterns that
//! the compiler alone would not reject. See
//! [ADR-0016](../../../docs/decisions/ADR-0016-keep-a-pure-semantic-core.md).
#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )
)]

extern crate alloc;

pub mod strand;

pub use strand::{SpindleId, StrandId, next_sequence};
