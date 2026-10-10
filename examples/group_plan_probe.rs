//! Warm decision-only GroupPlan screen. No journal, clock input or transport
//! work is modeled; fixtures and differential checks precede timed stages.
use fabric_core::delivery::{
    BatchDigest, BindingState, CommittedStrand, DeliveryDecision, GroupPlan, IncomingBatch,
    decide_delivery,
};
use fabric_core::strand::{SpindleId, StrandId};
use std::collections::BTreeMap;
use std::hint::black_box;
use std::num::NonZeroU64;
use std::time::Instant;

// Frozen original staged-Vec lookup, retained independently of the candidate.
#[derive(Default)]
struct Legacy<'a> {
    overlay: BTreeMap<StrandId, CommittedStrand>,
    staged: Vec<(&'a str, SpindleId)>,
}
impl<'a> Legacy<'a> {
    fn decide(&mut self, credential: &'a str, incoming: &IncomingBatch) -> DeliveryDecision {
        let spindle = incoming.strand.spindle();
        let binding = BindingState {
            credential_spindle: self
                .staged
                .iter()
                .find(|(c, _)| *c == credential)
                .map(|(_, s)| *s),
            spindle_bound_elsewhere: self
                .staged
                .iter()
                .any(|(c, s)| *c != credential && *s == spindle),
        };
        let decision = decide_delivery(
            self.overlay.get(&incoming.strand).copied(),
            incoming,
            binding,
        );
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

struct Input {
    credential: String,
    batch: IncomingBatch,
}
fn fixture(size: usize, distinct: bool) -> Vec<Input> {
    (0..size)
        .map(|i| {
            let identity = if distinct { i as u64 + 1 } else { 1 };
            let mut node = [0; 16];
            node[..8].copy_from_slice(&identity.to_le_bytes());
            let spindle = SpindleId::new(node);
            let mut digest = [42; 32];
            digest[..8].copy_from_slice(&(i as u64).to_le_bytes());
            Input {
                credential: format!("credential-{identity:04}"),
                batch: IncomingBatch {
                    strand: StrandId::new(spindle, 1).expect("valid fixture Strand"),
                    sequence: NonZeroU64::new(if distinct { 1 } else { i as u64 + 1 }).unwrap(),
                    digest: BatchDigest(digest),
                },
            }
        })
        .collect()
}

fn verify(inputs: &[Input]) {
    let mut legacy = Legacy::default();
    let mut candidate = GroupPlan::new();
    for input in inputs {
        let expected = DeliveryDecision::Commit {
            sequence: input.batch.sequence.get(),
        };
        assert_eq!(legacy.decide(&input.credential, &input.batch), expected);
        assert_eq!(
            candidate.decide(
                &input.credential,
                &input.batch,
                None,
                BindingState::default()
            ),
            expected
        );
    }
}

fn run(inputs: &[Input], candidate: bool, plans: usize) -> (u128, usize, u64) {
    let mut commits = 0;
    let mut digest = 0_u64;
    let start = Instant::now();
    for _ in 0..plans {
        if candidate {
            let mut plan = GroupPlan::new();
            for input in inputs {
                let decision = plan.decide(
                    black_box(input.credential.as_str()),
                    black_box(&input.batch),
                    None,
                    BindingState::default(),
                );
                if let DeliveryDecision::Commit { sequence } = black_box(decision) {
                    commits += 1;
                    digest = digest.wrapping_add(sequence);
                }
            }
            black_box(plan);
        } else {
            let mut plan = Legacy::default();
            for input in inputs {
                let decision = plan.decide(
                    black_box(input.credential.as_str()),
                    black_box(&input.batch),
                );
                if let DeliveryDecision::Commit { sequence } = black_box(decision) {
                    commits += 1;
                    digest = digest.wrapping_add(sequence);
                }
            }
            black_box(plan);
        }
    }
    (
        start.elapsed().as_nanos(),
        black_box(commits),
        black_box(digest),
    )
}

fn main() {
    for size in [1, 8, 32, 256, 2048] {
        let iterations = (131_072 / size).max(64);
        for distinct in [false, true] {
            let inputs = fixture(size, distinct);
            verify(&inputs);
            for pair in 1..=3 {
                let order = if pair % 2 == 1 {
                    [false, true]
                } else {
                    [true, false]
                };
                for (arm, candidate) in order.into_iter().enumerate() {
                    let expected_digest = inputs
                        .iter()
                        .fold(0_u64, |sum, input| {
                            sum.wrapping_add(input.batch.sequence.get())
                        })
                        .wrapping_mul(iterations as u64);
                    let (wall_ns, commits, digest) = run(&inputs, candidate, iterations);
                    assert_eq!(commits, iterations * size);
                    assert_eq!(digest, expected_digest);
                    println!(
                        "{}",
                        serde_json::json!({
                            "seed": 42, "size": size,
                            "shape": if distinct { "distinct" } else { "hot" },
                            "pair": pair, "arm": arm,
                            "variant": if candidate { "indexed" } else { "legacy" },
                            "iterations": iterations, "total_offers": iterations * size,
                            "required_commit_count": iterations * size,
                            "commits": commits, "wall_ns": wall_ns,
                            "output_digest": digest, "expected_output_digest": expected_digest,
                            "digest_scheme": "wrapping_sum_of_committed_sequences",
                            "exact_decisions_checked": size,
                            "scope": "warm_decisions_with_plan_allocation_and_drop"
                        })
                    );
                }
            }
        }
    }
}
