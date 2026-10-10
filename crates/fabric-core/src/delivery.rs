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
    first_binding: Option<(&'a str, SpindleId)>,
    secondary: SecondaryBindings<'a>,
}

// One fixed crossover: at most 32 distinct reverse bindings (including the
// inline first binding) use a compact list. Promotion consumes that list;
// indexed groups retain no third copy of the bindings.
const SMALL_BINDINGS: usize = 32;

#[derive(Debug)]
enum SecondaryBindings<'a> {
    Small(Vec<(&'a str, SpindleId)>),
    Indexed {
        credentials: BTreeMap<&'a str, SpindleId>,
        spindles: BTreeMap<SpindleId, &'a str>,
    },
}
impl Default for SecondaryBindings<'_> {
    fn default() -> Self {
        Self::Small(Vec::new())
    }
}
impl<'a> SecondaryBindings<'a> {
    fn credential(&self, credential: &str) -> Option<SpindleId> {
        match self {
            Self::Small(bindings) => bindings
                .iter()
                .find(|(c, _)| *c == credential)
                .map(|(_, s)| *s),
            Self::Indexed { credentials, .. } => credentials.get(credential).copied(),
        }
    }
    fn spindle(&self, spindle: SpindleId) -> Option<&'a str> {
        match self {
            Self::Small(bindings) => bindings
                .iter()
                .find(|(_, s)| *s == spindle)
                .map(|(c, _)| *c),
            Self::Indexed { spindles, .. } => spindles.get(&spindle).copied(),
        }
    }
    // Called only for a previously unseen Spindle whose binding was accepted.
    fn insert_new(&mut self, credential: &'a str, spindle: SpindleId, first_credential: &str) {
        if let Self::Small(bindings) = self {
            if bindings.len() < SMALL_BINDINGS - 1 {
                bindings.push((credential, spindle));
                return;
            }
            let mut credentials = BTreeMap::new();
            let mut spindles = BTreeMap::new();
            for (c, s) in core::mem::take(bindings) {
                if c != first_credential {
                    credentials.entry(c).or_insert(s);
                }
                spindles.insert(s, c);
            }
            *self = Self::Indexed {
                credentials,
                spindles,
            };
        }
        if let Self::Indexed {
            credentials,
            spindles,
        } = self
        {
            if credential != first_credential {
                credentials.entry(credential).or_insert(spindle);
            }
            spindles.insert(spindle, credential);
        }
    }
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
        // The first binding stays inline: repeated offers from one Spindle
        // do not allocate an index or lengthen a scan. Explicit durable facts
        // may override the first staged credential mapping, so retain reverse
        // bindings for every distinct Spindle, not just every credential.
        let staged_spindle = self
            .first_binding
            .filter(|(_, s)| *s == spindle)
            .map(|(c, _)| c)
            .or_else(|| self.secondary.spindle(spindle));
        let binding = BindingState {
            credential_spindle: durable_binding.credential_spindle.or_else(|| {
                self.first_binding
                    .filter(|(c, _)| *c == credential)
                    .map(|(_, s)| s)
                    .or_else(|| self.secondary.credential(credential))
            }),
            spindle_bound_elsewhere: durable_binding.spindle_bound_elsewhere
                || staged_spindle.is_some_and(|c| c != credential),
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
            match self.first_binding {
                None => self.first_binding = Some((credential, spindle)),
                Some((first_credential, _)) => {
                    // Remember each distinct reverse binding once. Repeated
                    // accepted Batches do not grow the compact list or index.
                    if staged_spindle.is_none() {
                        self.secondary
                            .insert_new(credential, spindle, first_credential);
                    }
                }
            }
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

    // Frozen pre-index implementation: a differential control, not another
    // invocation of the candidate's binding lookup.
    #[derive(Default)]
    struct LegacyPlan<'a> {
        overlay: BTreeMap<StrandId, CommittedStrand>,
        staged: Vec<(&'a str, SpindleId)>,
    }
    impl<'a> LegacyPlan<'a> {
        fn decide(
            &mut self,
            credential: &'a str,
            incoming: &IncomingBatch,
            durable: Option<CommittedStrand>,
            facts: BindingState,
        ) -> DeliveryDecision {
            let spindle = incoming.strand.spindle();
            let binding = BindingState {
                credential_spindle: facts.credential_spindle.or_else(|| {
                    self.staged
                        .iter()
                        .find(|(c, _)| *c == credential)
                        .map(|(_, s)| *s)
                }),
                spindle_bound_elsewhere: facts.spindle_bound_elsewhere
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

    #[test]
    fn indexed_plan_differential_covers_all_delivery_branches() {
        let schedule = [
            (
                "a",
                batch(1, 1, 1, X),
                DeliveryDecision::Commit { sequence: 1 },
            ),
            (
                "a",
                batch(1, 1, 1, X),
                DeliveryDecision::Duplicate {
                    committed_through: 1,
                },
            ),
            (
                "a",
                batch(1, 1, 1, Y),
                DeliveryDecision::Conflict {
                    committed_through: 1,
                },
            ),
            (
                "a",
                batch(1, 1, 3, X),
                DeliveryDecision::Gap {
                    committed_through: 1,
                },
            ),
            (
                "a",
                batch(1, 1, 2, X),
                DeliveryDecision::Commit { sequence: 2 },
            ),
            (
                "a",
                batch(1, 1, 1, X),
                DeliveryDecision::Stale {
                    committed_through: 2,
                },
            ),
            ("b", batch(1, 1, 3, X), DeliveryDecision::Forbidden),
        ];
        let mut candidate = GroupPlan::new();
        let mut legacy = LegacyPlan::default();
        for (credential, incoming, expected) in schedule {
            assert_eq!(legacy.decide(credential, &incoming, None, FREE), expected);
            assert_eq!(
                candidate.decide(credential, &incoming, None, FREE),
                expected
            );
        }
    }

    #[test]
    fn indexed_bindings_preserve_first_mapping_and_every_reverse_binding() {
        // Explicit facts override staged credential lookup. This adversarial
        // schedule commits two Spindles under one credential, then tests both.
        let schedule = [
            ("a", batch(1, 1, 1, X), FREE),
            (
                "a",
                batch(2, 1, 1, X),
                BindingState {
                    credential_spindle: Some(spindle(2)),
                    ..FREE
                },
            ),
            ("a", batch(2, 1, 2, X), FREE),
            ("b", batch(2, 1, 2, X), FREE),
            ("a", batch(1, 1, 2, X), FREE),
            (
                "a",
                batch(2, 1, 2, X),
                BindingState {
                    credential_spindle: Some(spindle(2)),
                    ..FREE
                },
            ),
            ("c", batch(3, 1, 1, X), FREE),
            (
                "c",
                batch(4, 1, 1, X),
                BindingState {
                    credential_spindle: Some(spindle(4)),
                    ..FREE
                },
            ),
            ("c", batch(4, 1, 2, X), FREE),
            ("d", batch(4, 1, 2, X), FREE),
        ];
        let mut candidate = GroupPlan::new();
        let mut legacy = LegacyPlan::default();
        for (credential, incoming, facts) in schedule {
            assert_eq!(
                candidate.decide(credential, &incoming, None, facts),
                legacy.decide(credential, &incoming, None, facts)
            );
        }
        let SecondaryBindings::Small(bindings) = &candidate.secondary else {
            panic!("small fixture promoted");
        };
        assert_eq!(bindings.len(), 3);
    }

    #[test]
    fn indexed_bindings_match_legacy_with_changing_explicit_facts_seed_42() {
        use alloc::collections::BTreeSet;
        const CREDENTIALS: [&str; 8] = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let mut seed = 42_u64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed >> 32
        };
        for _ in 0..64 {
            let mut candidate = GroupPlan::new();
            let mut legacy = LegacyPlan::default();
            let mut committed_bindings = BTreeSet::new();
            for _ in 0..512 {
                let credential = CREDENTIALS[next() as usize % CREDENTIALS.len()];
                let incoming = batch(
                    (next() % 8 + 1) as u8,
                    next() % 3 + 1,
                    next() % 12 + 1,
                    BatchDigest([next() as u8; 32]),
                );
                let durable = (next() % 3 != 0).then(|| CommittedStrand {
                    last: next() % 12 + 1,
                    digest: BatchDigest([next() as u8; 32]),
                });
                let facts = BindingState {
                    credential_spindle: (next() % 3 != 0).then(|| spindle((next() % 8 + 1) as u8)),
                    spindle_bound_elsewhere: next() % 7 == 0,
                };
                let actual = candidate.decide(credential, &incoming, durable, facts);
                assert_eq!(actual, legacy.decide(credential, &incoming, durable, facts));
                if matches!(actual, DeliveryDecision::Commit { .. }) {
                    committed_bindings.insert((credential, incoming.strand.spindle()));
                }
                match &candidate.secondary {
                    SecondaryBindings::Small(bindings) => {
                        assert!(bindings.len() < SMALL_BINDINGS);
                        assert!(bindings.len() <= committed_bindings.len());
                    }
                    SecondaryBindings::Indexed {
                        credentials,
                        spindles,
                    } => {
                        assert!(credentials.len() <= committed_bindings.len());
                        assert!(spindles.len() <= committed_bindings.len());
                    }
                }
            }
        }
    }

    #[test]
    fn hot_single_binding_stays_inline_for_2048_commits() {
        let mut plan = GroupPlan::new();
        for sequence in 1..=2048 {
            assert_eq!(
                plan.decide("a", &batch(1, 1, sequence, X), None, FREE),
                DeliveryDecision::Commit { sequence }
            );
        }
        assert_eq!(plan.first_binding, Some(("a", spindle(1))));
        let SecondaryBindings::Small(bindings) = &plan.secondary else {
            panic!("singleton promoted");
        };
        assert!(bindings.is_empty());
        assert_eq!(plan.overlay.len(), 1);
    }

    #[test]
    fn compact_bindings_promote_once_preserving_overridden_forward_mappings() {
        fn compare(
            candidate: &mut GroupPlan<'_>,
            legacy: &mut LegacyPlan<'_>,
            credential: &'static str,
            incoming: IncomingBatch,
            facts: BindingState,
        ) {
            assert_eq!(
                candidate.decide(credential, &incoming, None, facts),
                legacy.decide(credential, &incoming, None, facts)
            );
        }
        let mut candidate = GroupPlan::new();
        let mut legacy = LegacyPlan::default();
        compare(&mut candidate, &mut legacy, "a", batch(1, 1, 1, X), FREE);
        compare(&mut candidate, &mut legacy, "b", batch(2, 1, 1, X), FREE);
        for (credential, node) in [("a", 3), ("b", 4)] {
            compare(
                &mut candidate,
                &mut legacy,
                credential,
                batch(node, 1, 1, X),
                BindingState {
                    credential_spindle: Some(spindle(node)),
                    ..FREE
                },
            );
        }
        for node in 5..=32 {
            compare(
                &mut candidate,
                &mut legacy,
                "extra",
                batch(node, 1, 1, X),
                BindingState {
                    credential_spindle: Some(spindle(node)),
                    ..FREE
                },
            );
        }
        let SecondaryBindings::Small(bindings) = &candidate.secondary else {
            panic!("premature promotion");
        };
        assert_eq!(bindings.len(), 31);
        compare(
            &mut candidate,
            &mut legacy,
            "extra",
            batch(33, 1, 1, X),
            BindingState {
                credential_spindle: Some(spindle(33)),
                ..FREE
            },
        );
        let SecondaryBindings::Indexed {
            credentials,
            spindles,
        } = &candidate.secondary
        else {
            panic!("missing promotion");
        };
        assert_eq!(credentials.len(), 2);
        assert_eq!(spindles.len(), 32);
        assert_eq!(credentials.get("b"), Some(&spindle(2)));
        assert_eq!(credentials.get("extra"), Some(&spindle(5)));
        for (credential, node) in [
            ("a", 1),
            ("a", 3),
            ("b", 2),
            ("b", 4),
            ("extra", 5),
            ("extra", 33),
        ] {
            compare(
                &mut candidate,
                &mut legacy,
                credential,
                batch(node, 1, 2, X),
                FREE,
            );
            // Both the first and later reverse bindings must forbid intrusion.
            compare(
                &mut candidate,
                &mut legacy,
                "intruder",
                batch(node, 1, 2, X),
                FREE,
            );
            compare(
                &mut candidate,
                &mut legacy,
                credential,
                batch(node, 1, 2, X),
                BindingState {
                    credential_spindle: Some(spindle(node)),
                    ..FREE
                },
            );
        }
        for sequence in 3..=2048 {
            compare(
                &mut candidate,
                &mut legacy,
                "extra",
                batch(33, 1, sequence, X),
                BindingState {
                    credential_spindle: Some(spindle(33)),
                    ..FREE
                },
            );
        }
        let SecondaryBindings::Indexed {
            credentials,
            spindles,
        } = &candidate.secondary
        else {
            panic!("index demoted");
        };
        assert_eq!(credentials.len(), 2);
        assert_eq!(spindles.len(), 32);
    }

    #[test]
    fn compact_secondary_binding_deduplicates_repeated_commits() {
        let mut candidate = GroupPlan::new();
        let mut legacy = LegacyPlan::default();
        for sequence in 1..=2048 {
            for (credential, node) in [("a", 1), ("b", 2)] {
                let incoming = batch(node, 1, sequence, X);
                assert_eq!(
                    candidate.decide(credential, &incoming, None, FREE),
                    legacy.decide(credential, &incoming, None, FREE)
                );
            }
        }
        let SecondaryBindings::Small(bindings) = &candidate.secondary else {
            panic!("duplicates promoted");
        };
        assert_eq!(bindings.as_slice(), &[("b", spindle(2))]);
    }
}
