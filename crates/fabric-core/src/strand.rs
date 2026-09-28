//! Spindle and Strand identity.
//!
//! A Spindle is the host-resident collection role; `SpindleId` is its stable
//! 16-byte identity (the `node_id` field of the version-one envelope, whose
//! name and field number stay as they are). A Strand is one ordered telemetry
//! lineage produced by one Spindle generation: `StrandId = (SpindleId,
//! generation)`. Batches on a Strand carry sequences 1, 2, 3, ... and a new
//! generation starts a new, independent Strand at sequence 1.
//!
//! A Strand is not a connection, session, thread, file, Segment or query
//! result. It is the scope of sequence ordering, duplicate-retry identity,
//! gap detection and acknowledged progress.

use core::num::NonZeroU64;

/// The identity of one Spindle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpindleId([u8; 16]);

impl SpindleId {
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// One ordered lineage: a Spindle identity and a non-zero generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StrandId {
    spindle: SpindleId,
    generation: NonZeroU64,
}

impl StrandId {
    /// `None` for generation 0, which the envelope rejects as invalid.
    pub const fn new(spindle: SpindleId, generation: u64) -> Option<Self> {
        match NonZeroU64::new(generation) {
            Some(generation) => Some(Self {
                spindle,
                generation,
            }),
            None => None,
        }
    }

    pub const fn spindle(&self) -> SpindleId {
        self.spindle
    }

    pub const fn generation(&self) -> u64 {
        self.generation.get()
    }
}

/// The sequence expected after `last` on a Strand (`last = 0` means nothing
/// has been committed). `None` when the Strand is exhausted: no sequence can
/// follow `u64::MAX`, so nothing may be accepted as its successor.
pub const fn next_sequence(last: u64) -> Option<u64> {
    last.checked_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_zero_is_not_a_strand() {
        let spindle = SpindleId::new([7; 16]);
        assert!(StrandId::new(spindle, 0).is_none());
        let strand = StrandId::new(spindle, 3).unwrap();
        assert_eq!((strand.spindle(), strand.generation()), (spindle, 3));
    }

    #[test]
    fn generations_are_distinct_strands_ordered_by_spindle_then_generation() {
        let a = SpindleId::new([1; 16]);
        let b = SpindleId::new([2; 16]);
        let a1 = StrandId::new(a, 1).unwrap();
        let a2 = StrandId::new(a, 2).unwrap();
        let b1 = StrandId::new(b, 1).unwrap();
        assert_ne!(a1, a2);
        assert!(a1 < a2 && a2 < b1);
    }

    #[test]
    fn the_last_sequence_has_no_successor() {
        assert_eq!(next_sequence(0), Some(1));
        assert_eq!(next_sequence(41), Some(42));
        assert_eq!(next_sequence(u64::MAX), None);
    }
}
