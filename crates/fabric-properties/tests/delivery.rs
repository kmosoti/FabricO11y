//! Delivery properties from ADR-0013 (in order, bounded dedup, bindings).

use fabric_core::delivery::{
    BatchDigest, BindingState, CommittedStrand, DeliveryDecision, GroupPlan, IncomingBatch,
    decide_delivery,
};
use fabric_core::strand::{SpindleId, StrandId};
use proptest::prelude::*;
use std::collections::BTreeMap;
use std::num::NonZeroU64;

/// A small universe so that collisions (same Strand, same credential, same
/// Spindle, equal digests) are frequent.
fn spindle(i: u8) -> SpindleId {
    SpindleId::new([i; 16])
}

fn strand(spindle_index: u8, generation: u64) -> StrandId {
    StrandId::new(spindle(spindle_index), generation).unwrap()
}

#[derive(Debug, Clone)]
struct Offer {
    credential: &'static str,
    spindle: u8,
    generation: u64,
    sequence: u64,
    digest: u8,
}

const CREDENTIALS: [&str; 3] = ["cred-a", "cred-b", "cred-c"];

fn offer() -> impl Strategy<Value = Offer> {
    (0..3_usize, 0..3_u8, 1..3_u64, 1..6_u64, 0..2_u8).prop_map(
        |(credential, spindle, generation, sequence, digest)| Offer {
            credential: CREDENTIALS[credential],
            spindle,
            generation,
            sequence,
            digest,
        },
    )
}

fn incoming(o: &Offer) -> IncomingBatch {
    IncomingBatch {
        strand: strand(o.spindle, o.generation),
        sequence: NonZeroU64::new(o.sequence).unwrap(),
        digest: BatchDigest([o.digest; 32]),
    }
}

/// Durable state as the specification describes it, updated one Batch at a
/// time: the last committed sequence and digest per Strand, and each
/// credential's bound Spindle (bound by its first committed Batch).
#[derive(Debug, Default, Clone)]
struct Durable {
    strands: BTreeMap<StrandId, CommittedStrand>,
    bound: BTreeMap<&'static str, SpindleId>,
}

impl Durable {
    fn binding(&self, credential: &str, spindle: SpindleId) -> BindingState {
        BindingState {
            credential_spindle: self.bound.get(credential).copied(),
            spindle_bound_elsewhere: self
                .bound
                .iter()
                .any(|(c, s)| *c != credential && *s == spindle),
        }
    }

    fn apply(&mut self, o: &Offer, decision: DeliveryDecision) {
        if let DeliveryDecision::Commit { sequence } = decision {
            let batch = incoming(o);
            self.strands.insert(
                batch.strand,
                CommittedStrand {
                    last: sequence,
                    digest: batch.digest,
                },
            );
            self.bound.insert(o.credential, batch.strand.spindle());
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    /// The ADR-0013 table, stated as outcomes: a Batch commits exactly when
    /// it is the next sequence; an equal retry is a duplicate, a different
    /// one a conflict; older is stale; later is a gap; a binding
    /// disagreement forbids before any of these.
    #[test]
    fn every_offer_gets_the_outcome_the_table_names(
        last in proptest::option::of(1..6_u64),
        stored_digest in 0..2_u8,
        o in offer(),
        credential_spindle in proptest::option::of(0..3_u8),
        spindle_bound_elsewhere in any::<bool>(),
    ) {
        let committed = last.map(|last| CommittedStrand { last, digest: BatchDigest([stored_digest; 32]) });
        let binding = BindingState {
            credential_spindle: credential_spindle.map(spindle),
            spindle_bound_elsewhere,
        };
        let decision = decide_delivery(committed, &incoming(&o), binding);
        let permitted = credential_spindle.is_none_or(|s| s == o.spindle) && !spindle_bound_elsewhere;
        let last = last.unwrap_or(0);
        let expected = if !permitted {
            DeliveryDecision::Forbidden
        } else if o.sequence == last + 1 {
            DeliveryDecision::Commit { sequence: o.sequence }
        } else if o.sequence == last && o.digest == stored_digest {
            DeliveryDecision::Duplicate { committed_through: last }
        } else if o.sequence == last {
            DeliveryDecision::Conflict { committed_through: last }
        } else if o.sequence < last {
            DeliveryDecision::Stale { committed_through: last }
        } else {
            DeliveryDecision::Gap { committed_through: last }
        };
        prop_assert_eq!(decision, expected);
    }

    /// Grouping is invisible: deciding a sequence of offers as one commit
    /// group gives the same decisions as committing each offer on its own,
    /// with durable state and bindings updated between them.
    #[test]
    fn a_commit_group_decides_like_one_batch_at_a_time(
        offers in proptest::collection::vec(offer(), 0..24),
    ) {
        let mut sequential = Durable::default();
        let mut expected = Vec::new();
        for o in &offers {
            let batch = incoming(o);
            let decision = decide_delivery(
                sequential.strands.get(&batch.strand).copied(),
                &batch,
                sequential.binding(o.credential, batch.strand.spindle()),
            );
            sequential.apply(o, decision);
            expected.push(decision);
        }

        let durable = Durable::default();
        let mut plan = GroupPlan::new();
        let grouped: Vec<_> = offers
            .iter()
            .map(|o| {
                let batch = incoming(o);
                plan.decide(
                    o.credential,
                    &batch,
                    durable.strands.get(&batch.strand).copied(),
                    durable.binding(o.credential, batch.strand.spindle()),
                )
            })
            .collect();
        prop_assert_eq!(grouped, expected);
    }

    /// Over any history of offers (retries, reorderings, conflicts), each
    /// Strand's committed sequences are exactly 1, 2, ..., n with no
    /// repeats, and a committed Batch's bytes are never replaced.
    #[test]
    fn committed_history_is_contiguous_and_immutable(
        offers in proptest::collection::vec(offer(), 0..40),
    ) {
        let mut durable = Durable::default();
        let mut log: BTreeMap<StrandId, Vec<(u64, BatchDigest)>> = BTreeMap::new();
        for o in &offers {
            let batch = incoming(o);
            let before = durable.strands.get(&batch.strand).copied();
            let decision = decide_delivery(
                before,
                &batch,
                durable.binding(o.credential, batch.strand.spindle()),
            );
            if let DeliveryDecision::Commit { sequence } = decision {
                log.entry(batch.strand).or_default().push((sequence, batch.digest));
            } else {
                // Anything but a commit leaves the Strand as it was.
                durable.apply(o, decision);
                prop_assert_eq!(durable.strands.get(&batch.strand).copied(), before);
                continue;
            }
            durable.apply(o, decision);
        }
        for (strand, entries) in log {
            let sequences: Vec<u64> = entries.iter().map(|(s, _)| *s).collect();
            let expected: Vec<u64> = (1..=sequences.len() as u64).collect();
            prop_assert_eq!(&sequences, &expected, "strand {:?}", strand);
            let last = entries.last().unwrap();
            prop_assert_eq!(durable.strands[&strand], CommittedStrand { last: last.0, digest: last.1 });
        }
    }
}
