//! The delivery decision of [ADR-0013], as a pure function.
//!
//! Given what is durably committed on a Strand, an incoming Batch and the
//! credential binding, decide whether to commit it, acknowledge it again,
//! reject it as a conflict, or report a gap. The decision is data: writing the
//! journal, syncing, answering HTTP and reading the clock belong to the
//! application and adapters.
//!
//! [ADR-0013]: ../../../docs/decisions/ADR-0013-deliver-batches-in-order-with-bounded-dedup.md

use crate::strand::{SpindleId, StrandId, next_sequence};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::num::NonZeroU64;

/// A digest of a Batch's exact encoded bytes. The caller computes it (the
/// server uses SHA-256); the core only compares digests for equality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BatchDigest(pub [u8; 32]);

/// The durable state of one Strand: its last committed sequence (at least 1)
/// and the digest of that Batch's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommittedStrand {
    pub last: u64,
    pub digest: BatchDigest,
}

/// A Batch offered for commit. Sequence 0 does not exist on a Strand, so the
/// type cannot express it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IncomingBatch {
    pub strand: StrandId,
    pub sequence: NonZeroU64,
    pub digest: BatchDigest,
}

/// What durable state says about the submitting credential and Spindle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BindingState {
    /// The Spindle the submitting credential is already bound to, if any.
    pub credential_spindle: Option<SpindleId>,
    /// The submitting Spindle is already bound to a different credential.
    pub spindle_bound_elsewhere: bool,
}

impl BindingState {
    /// A credential is bound to one Spindle and a Spindle to one credential.
    /// First use binds; any later disagreement in either direction forbids.
    pub fn permits(&self, spindle: SpindleId) -> bool {
        self.credential_spindle.is_none_or(|bound| bound == spindle)
            && !self.spindle_bound_elsewhere
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryDecision {
    /// `sequence = last + 1`: commit durably, then acknowledge `sequence`.
    Commit { sequence: u64 },
    /// `sequence = last` with equal bytes: a retry after a lost ACK.
    Duplicate { committed_through: u64 },
    /// `sequence < last`: already committed; acknowledge through `last`.
    Stale { committed_through: u64 },
    /// `sequence = last` with different bytes; nothing is replaced.
    Conflict { committed_through: u64 },
    /// `sequence > last + 1`; the sender resends from `last + 1`.
    Gap { committed_through: u64 },
    /// The credential and Spindle binding disagree in either direction.
    Forbidden,
}

/// Decide one Batch against the Strand's committed state and the binding.
pub fn decide_delivery(
    committed: Option<CommittedStrand>,
    incoming: &IncomingBatch,
    binding: BindingState,
) -> DeliveryDecision {
    if !binding.permits(incoming.strand.spindle()) {
        return DeliveryDecision::Forbidden;
    }
    let last = committed.map_or(0, |c| c.last);
    let sequence = incoming.sequence.get();
    if next_sequence(last) == Some(sequence) {
        DeliveryDecision::Commit { sequence }
    } else if sequence == last {
        // `last >= 1` here because `sequence >= 1`, so `committed` is `Some`.
        match committed {
            Some(c) if c.digest == incoming.digest => DeliveryDecision::Duplicate {
                committed_through: last,
            },
            _ => DeliveryDecision::Conflict {
                committed_through: last,
            },
        }
    } else if sequence < last {
        DeliveryDecision::Stale {
            committed_through: last,
        }
    } else {
        DeliveryDecision::Gap {
            committed_through: last,
        }
    }
}

/// Decides the Batches of one commit group in order. A Batch sees the
/// durable state plus every earlier Batch of the same group that this plan
/// decided to commit, including the credential bindings those create.
#[derive(Debug, Default)]
pub struct GroupPlan<'a> {
    overlay: BTreeMap<StrandId, CommittedStrand>,
    staged: Vec<(&'a str, SpindleId)>,
}

impl<'a> GroupPlan<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decide `incoming` from `credential`, given its Strand's durable state
    /// and the durable binding for this credential and Spindle.
    pub fn decide(
        &mut self,
        credential: &'a str,
        incoming: &IncomingBatch,
        durable: Option<CommittedStrand>,
        durable_binding: BindingState,
    ) -> DeliveryDecision {
        let spindle = incoming.strand.spindle();
        let binding = BindingState {
            credential_spindle: durable_binding.credential_spindle.or_else(|| {
                self.staged
                    .iter()
                    .find(|(c, _)| *c == credential)
                    .map(|(_, s)| *s)
            }),
            spindle_bound_elsewhere: durable_binding.spindle_bound_elsewhere
                || self
                    .staged
                    .iter()
                    .any(|(c, s)| *c != credential && *s == spindle),
        };
        let current = self.overlay.get(&incoming.strand).copied().or(durable);
        let decision = decide_delivery(current, incoming, binding);
        if let DeliveryDecision::Commit { sequence } = decision {
            self.overlay.insert(
                incoming.strand,
                CommittedStrand {
                    last: sequence,
                    digest: incoming.digest,
                },
            );
            self.staged.push((credential, spindle));
        }
        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const X: BatchDigest = BatchDigest([1; 32]);
    const Y: BatchDigest = BatchDigest([2; 32]);

    fn spindle(n: u8) -> SpindleId {
        SpindleId::new([n; 16])
    }

    fn batch(s: u8, generation: u64, sequence: u64, digest: BatchDigest) -> IncomingBatch {
        IncomingBatch {
            strand: StrandId::new(spindle(s), generation).unwrap(),
            sequence: NonZeroU64::new(sequence).unwrap(),
            digest,
        }
    }

    fn at(last: u64, digest: BatchDigest) -> Option<CommittedStrand> {
        Some(CommittedStrand { last, digest })
    }

    const FREE: BindingState = BindingState {
        credential_spindle: None,
        spindle_bound_elsewhere: false,
    };

    /// Every row of the ADR-0013 table.
    #[test]
    fn adr_0013_truth_table() {
        use DeliveryDecision::*;
        let cases = [
            (None, batch(1, 1, 1, X), Commit { sequence: 1 }),
            (
                None,
                batch(1, 1, 2, X),
                Gap {
                    committed_through: 0,
                },
            ),
            (at(3, X), batch(1, 1, 4, Y), Commit { sequence: 4 }),
            (
                at(3, X),
                batch(1, 1, 3, X),
                Duplicate {
                    committed_through: 3,
                },
            ),
            (
                at(3, X),
                batch(1, 1, 3, Y),
                Conflict {
                    committed_through: 3,
                },
            ),
            (
                at(3, X),
                batch(1, 1, 2, X),
                Stale {
                    committed_through: 3,
                },
            ),
            (
                at(3, X),
                batch(1, 1, 1, Y),
                Stale {
                    committed_through: 3,
                },
            ),
            (
                at(3, X),
                batch(1, 1, 5, X),
                Gap {
                    committed_through: 3,
                },
            ),
        ];
        for (committed, incoming, expected) in cases {
            assert_eq!(
                decide_delivery(committed, &incoming, FREE),
                expected,
                "{committed:?} {incoming:?}"
            );
        }
    }

    #[test]
    fn binding_disagreement_in_either_direction_forbids_before_sequencing() {
        let incoming = batch(1, 1, 1, X);
        let other_spindle = BindingState {
            credential_spindle: Some(spindle(2)),
            spindle_bound_elsewhere: false,
        };
        let other_credential = BindingState {
            credential_spindle: None,
            spindle_bound_elsewhere: true,
        };
        let same = BindingState {
            credential_spindle: Some(spindle(1)),
            spindle_bound_elsewhere: false,
        };
        assert_eq!(
            decide_delivery(None, &incoming, other_spindle),
            DeliveryDecision::Forbidden
        );
        assert_eq!(
            decide_delivery(None, &incoming, other_credential),
            DeliveryDecision::Forbidden
        );
        assert_eq!(
            decide_delivery(None, &incoming, same),
            DeliveryDecision::Commit { sequence: 1 }
        );
    }

    /// Counterexample kept from the extraction: at the base, `last + 1`
    /// overflowed for a Strand at `u64::MAX` (a debug panic; in release a
    /// comparison against 0). An exhausted Strand accepts no successor.
    #[test]
    fn an_exhausted_strand_accepts_no_successor() {
        let committed = at(u64::MAX, X);
        assert_eq!(
            decide_delivery(committed, &batch(1, 1, u64::MAX, X), FREE),
            DeliveryDecision::Duplicate {
                committed_through: u64::MAX
            }
        );
        assert_eq!(
            decide_delivery(committed, &batch(1, 1, 1, X), FREE),
            DeliveryDecision::Stale {
                committed_through: u64::MAX
            }
        );
    }

    #[test]
    fn a_new_generation_is_an_independent_strand() {
        let mut plan = GroupPlan::new();
        assert_eq!(
            plan.decide("a", &batch(1, 1, 1, X), None, FREE),
            DeliveryDecision::Commit { sequence: 1 }
        );
        // Generation 2 starts at 1 regardless of generation 1's progress.
        assert_eq!(
            plan.decide("a", &batch(1, 2, 1, X), None, FREE),
            DeliveryDecision::Commit { sequence: 1 }
        );
        assert_eq!(
            plan.decide("a", &batch(1, 2, 3, X), None, FREE),
            DeliveryDecision::Gap {
                committed_through: 1
            }
        );
    }

    #[test]
    fn later_batches_in_a_group_see_earlier_commits_and_bindings() {
        let mut plan = GroupPlan::new();
        assert_eq!(
            plan.decide("a", &batch(1, 1, 1, X), None, FREE),
            DeliveryDecision::Commit { sequence: 1 }
        );
        assert_eq!(
            plan.decide("a", &batch(1, 1, 2, X), None, FREE),
            DeliveryDecision::Commit { sequence: 2 }
        );
        assert_eq!(
            plan.decide("a", &batch(1, 1, 2, Y), None, FREE),
            DeliveryDecision::Conflict {
                committed_through: 2
            }
        );
        // "a" is now bound to Spindle 1 and Spindle 1 to "a", within the group.
        assert_eq!(
            plan.decide("b", &batch(1, 1, 3, X), None, FREE),
            DeliveryDecision::Forbidden
        );
        assert_eq!(
            plan.decide("a", &batch(2, 1, 1, X), None, FREE),
            DeliveryDecision::Forbidden
        );
        // A forbidden Batch stages nothing, so "b" may still bind Spindle 2.
        assert_eq!(
            plan.decide("b", &batch(2, 1, 1, X), None, FREE),
            DeliveryDecision::Commit { sequence: 1 }
        );
    }

    #[test]
    fn identical_inputs_give_identical_decisions() {
        let run = || {
            let mut plan = GroupPlan::new();
            [batch(1, 1, 1, X), batch(1, 1, 1, Y), batch(1, 1, 3, X)]
                .iter()
                .map(|b| plan.decide("a", b, None, FREE))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }
}
