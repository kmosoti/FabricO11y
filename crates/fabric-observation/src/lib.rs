//! The version-one **Observation** record and its canonical **FOB1** block
//! encoding, built from the bits up as a tower of nine levels, each with its
//! own contract, tests and negative controls, each using only the levels
//! below, with no dependencies and no standard library beyond `core` and
//! `alloc`:
//!
//! | Level | Module | Gives the level above |
//! | --- | --- | --- |
//! | 0 | [`bits`] | shifts and masks on machine words: seven-bit groups, little-endian pack and unpack |
//! | 1 | [`bytes`], [`crc32`] | a bounded reader whose errors name an offset; fixed-width integers; the IEEE CRC from its polynomial |
//! | 2 | [`varint`] | unsigned integers in exactly one (shortest) form; bounded counts; strings |
//! | 3 | [`zigzag`] | the bijection between signed and unsigned 64-bit values |
//! | 4 | [`delta`] | first- and second-order differences, total and invertible under wrap |
//! | 5 | [`dictionary`] | repeated values written once, in a table with one canonical order |
//! | 6 | [`cells`] | numbers, attribute values and sorted attribute lists, each with one form |
//! | 7 | [`record`] | the Observation: a line, a point or a span under one key |
//! | 8 | [`block`] | FOB1: records as columns, two dictionaries, a CRC; `encode` and `decode` |
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

#[cfg_attr(test, macro_use)]
extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod bits;
pub mod block;
pub mod bytes;
pub mod cells;
pub mod crc32;
pub mod delta;
pub mod dictionary;
pub mod record;
pub mod varint;
pub mod zigzag;

#[cfg(kani)]
mod proofs;

pub use block::{EncodeError, MAGIC, MAX_RECORDS, decode, decode_view, encode};
pub use bytes::DecodeError;
pub use cells::{Bits, Number, Value, ValueRef};
pub use record::{
    Locators, MAX_SEVERITY, Observation, ObservationRef, PointKind, Signal, SignalRef, SpanKind,
    Status, Strand, check,
};

#[cfg(test)]
mod tests;
