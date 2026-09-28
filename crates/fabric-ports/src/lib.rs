//! Ports: the effect boundaries application use cases cross.
//!
//! A port exists only where an effect crosses a semantically meaningful
//! boundary and the application must not know how it is performed. This
//! crate holds contracts, not implementations; adapters implement them.

use fabric_core::StrandId;
use fabric_core::delivery::{BindingState, CommittedStrand};
use fabric_core::retention::SegmentFacts;
use fabric_core::strand::SpindleId;

/// The server's durable journal of committed Batches, with the Strand and
/// binding state that replaying it yields.
pub trait DurableJournal {
    /// The durably committed state of `strand`, if any Batch was committed.
    fn committed(&self, strand: &StrandId) -> Option<CommittedStrand>;

    /// Durable binding state for this credential and Spindle.
    fn binding(&self, credential: &str, spindle: &SpindleId) -> BindingState;

    /// A reported write or sync failure quarantined the journal; it commits
    /// nothing until rebuilt.
    fn is_quarantined(&self) -> bool;

    /// Append the group's accepted offers (indexes into the group, in order)
    /// as one durable commit. Returning `Ok` is the durability promise: every
    /// accepted Batch has completed its data and marker syncs.
    fn commit(&mut self, accepted: &[usize], received_unix_nano: u64) -> Result<(), CommitFailed>;
}

/// Nothing in the group became durable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitFailed;

/// Wall-clock time, read by the application and passed to core decisions as
/// an explicit input.
pub trait Clock {
    fn now_unix_nano(&self) -> u64;
}

/// The server's sealed history, as retention sees it.
pub trait SegmentStore {
    /// Sealed Segments, oldest first: their label and what retention needs.
    fn sealed(&self) -> Result<Vec<(u64, SegmentFacts)>, StoreFailed>;

    /// Delete one whole Segment durably. Called oldest first.
    fn delete(&mut self, label: u64) -> Result<(), StoreFailed>;
}

/// An effect on stored history failed; the message says which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreFailed(pub String);
