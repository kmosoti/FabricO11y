//! Counterexamples from the isolated October 11 cargo-mutants audit.
//! Literal expected outcomes supplement the legacy differential references.

use fabric_core::delivery::{
    BatchDigest, BindingState, DeliveryDecision, GroupPlan, IncomingBatch,
};
use fabric_core::query::{CounterNumber, CounterSample, CounterStep, counter_step_numbers};
use fabric_core::strand::{SpindleId, StrandId};

#[test]
fn exact_i64_min_mixed_counter_is_not_a_reset() {
    let sample = |time_ns, value| CounterSample {
        start_ns: 1,
        time_ns,
        value,
    };
    assert_eq!(
        counter_step_numbers(
            sample(1_000_000_000, CounterNumber::Int(i64::MIN)),
            sample(
                2_000_000_000,
                CounterNumber::Double(-9_223_372_036_854_775_808.0)
            ),
        ),
        CounterStep::Rate(0.0),
    );
}

#[test]
fn bindings_added_after_index_promotion_still_forbid_rebinding() {
    let credentials: Vec<_> = (1..=34).map(|n| format!("credential-{n}")).collect();
    let incoming = |n: u8, generation, sequence| IncomingBatch {
        strand: StrandId::new(SpindleId::new([n; 16]), generation).unwrap(),
        sequence: core::num::NonZeroU64::new(sequence).unwrap(),
        digest: BatchDigest([42; 32]),
    };
    let mut plan = GroupPlan::new();
    for (index, credential) in credentials.iter().enumerate() {
        assert_eq!(
            plan.decide(
                credential,
                &incoming((index + 1) as u8, 1, 1),
                None,
                BindingState::default()
            ),
            DeliveryDecision::Commit { sequence: 1 },
        );
    }
    // New generations expose binding decisions without a sequence conflict.
    for (index, credential) in credentials.iter().enumerate() {
        let n = (index + 1) as u8;
        assert_eq!(
            plan.decide(
                credential,
                &incoming(100, 1, 1),
                None,
                BindingState::default()
            ),
            DeliveryDecision::Forbidden
        );
        assert_eq!(
            plan.decide(
                "intruder",
                &incoming(n, 2, 1),
                None,
                BindingState::default()
            ),
            DeliveryDecision::Forbidden
        );
        assert_eq!(
            plan.decide(
                credential,
                &incoming(n, 2, 1),
                None,
                BindingState::default()
            ),
            DeliveryDecision::Commit { sequence: 1 }
        );
    }
}

#[test]
fn any_nonfinite_counter_pair_is_a_reset() {
    let values = [
        f64::NEG_INFINITY,
        -f64::MAX,
        -1.0,
        0.0,
        1.0,
        f64::MAX,
        f64::INFINITY,
        f64::NAN,
    ];
    for a in values {
        for b in values {
            if a.is_finite() && b.is_finite() {
                continue;
            }
            let sample = |time_ns, value| CounterSample {
                start_ns: 1,
                time_ns,
                value: CounterNumber::Double(value),
            };
            assert_eq!(
                counter_step_numbers(sample(1_000_000_000, a), sample(2_000_000_000, b)),
                CounterStep::Reset
            );
        }
    }
}
