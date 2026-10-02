//! The version-one **Observation** record and its canonical **FOB1** block
//! encoding, built as a tower of eight levels, each with its own contract,
//! tests and negative controls, and each using only the level below:
//!
//! | Level | Module | Gives the level above |
//! | --- | --- | --- |
//! | 0 | [`bytes`] | a bounded reader whose errors name an offset; fixed-width integers |
//! | 1 | [`varint`] | unsigned integers in exactly one (shortest) form; bounded counts; strings |
//! | 2 | [`zigzag`] | the bijection between signed and unsigned 64-bit values |
//! | 3 | [`delta`] | first- and second-order differences, total and invertible under wrap |
//! | 4 | [`dictionary`] | repeated values written once, in a table with one canonical order |
//! | 5 | [`cells`] | numbers, attribute values and sorted attribute lists, each with one form |
//! | 6 | [`record`] | the Observation: a line, a point or a span under one key |
//! | 7 | [`block`] | FOB1: records as columns, two dictionaries, a CRC; `encode` and `decode` |
//!
//! The property that holds at every level and therefore at the top: a valid
//! value has exactly one byte string, and an accepted byte string decodes to
//! exactly one value whose re-encoding is the same bytes. That is what lets
//! a hash of the bytes stand for the records (custody) without a second,
//! raw copy. See
//! [ADR-0023](../../../docs/decisions/ADR-0023-define-an-observation-record-with-a-canonical-encoding.md)
//! and the [architecture page](../../../docs/architecture/observation.md).
//!
//! Nothing here performs an effect; the crate is a codec and belongs to
//! adapter support ([layers](../../../docs/architecture/layers.json)).
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

pub mod block;
pub mod bytes;
pub mod cells;
pub mod delta;
pub mod dictionary;
pub mod record;
pub mod varint;
pub mod zigzag;

#[cfg(kani)]
mod proofs;

pub use block::{EncodeError, MAGIC, MAX_RECORDS, decode, encode};
pub use bytes::DecodeError;
pub use cells::{Bits, Number, Value};
pub use record::{
    Locators, MAX_SEVERITY, Observation, PointKind, Signal, SpanKind, Status, Strand, check,
};

#[cfg(test)]
mod tests;
