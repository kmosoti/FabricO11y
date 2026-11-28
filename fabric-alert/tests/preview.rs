use fabric_alert::{
    Direction, Error, Evaluation, Lifecycle, MAX_PREVIEW_OBSERVATIONS, Outcome, PreviewError,
    Reducer, Rule, Sample, Validity, preview,
};

fn rule() -> Rule {
    Rule {
        rule_id: [1; 16],
        instance_id: [2; 16],
        revision: 7,
        profile: [3; 32],
        source: [4; 32],
        direction: Direction::Above,
        trigger: 10,
        recovery: 5,
        firing_hold: 5,
        recovery_hold: 4,
    }
}
fn covered(frontier: u64, time: u64, value: i64) -> Evaluation {
    Evaluation {
        profile: [3; 32],
        source: [4; 32],
        frontier,
        time,
        sample: Sample::Covered {
            value,
            evidence: [5; 32],
            coverage_start: 0,
            coverage_end: time,
        },
    }
}
#[test]
fn frozen_preview_matches_transitions_and_preserves_missing_and_duplicates() {
    let first = covered(1, 10, 12);
    let missing = Evaluation {
        frontier: 2,
        time: 12,
        sample: Sample::Missing(Validity::Stale),
        ..first
    };
    let inputs = [
        first,
        first,
        missing,
        covered(3, 13, 12),
        covered(4, 18, 12),
    ];
    let candidate = rule();
    let result = preview(candidate, &inputs).unwrap();
    assert_eq!(result, preview(candidate, &inputs).unwrap());
    assert_eq!(result.rule, candidate);
    assert!(
        matches!(result.outcomes[0], Outcome::Applied(decision) if decision.lifecycle == Lifecycle::Pending)
    );
    assert!(matches!(result.outcomes[1], Outcome::Duplicate(_)));
    assert!(
        matches!(result.outcomes[2], Outcome::Applied(decision) if decision.validity == Validity::Stale)
    );
    assert!(
        matches!(result.outcomes[4], Outcome::Applied(decision) if decision.lifecycle == Lifecycle::Firing)
    );
    let mut control = Reducer::new(candidate).unwrap();
    let independently_replayed: Vec<_> = inputs
        .iter()
        .map(|input| control.evaluate(*input).unwrap())
        .collect();
    assert_eq!(result.outcomes, independently_replayed);
    assert_eq!(result.final_checkpoint, control.checkpoint());
}
#[test]
fn refusal_reports_first_error_and_leaves_active_reducer_unchanged() {
    let candidate = rule();
    let mut active = Reducer::new(candidate).unwrap();
    active.evaluate(covered(1, 10, 12)).unwrap();
    let before = active.checkpoint();
    let bad = Evaluation {
        source: [9; 32],
        ..covered(2, 11, 12)
    };
    assert_eq!(
        preview(candidate, &[covered(1, 10, 12), bad, covered(0, 0, 0)]),
        Err(PreviewError::InvalidInput {
            index: 1,
            error: Error::WrongSource
        })
    );
    assert_eq!(
        preview(candidate, &[covered(2, 10, 12), covered(1, 11, 12)]),
        Err(PreviewError::InvalidInput {
            index: 1,
            error: Error::OlderFrontier
        })
    );
    assert_eq!(
        preview(
            candidate,
            &vec![covered(1, 10, 12); MAX_PREVIEW_OBSERVATIONS]
        )
        .unwrap()
        .outcomes
        .len(),
        MAX_PREVIEW_OBSERVATIONS
    );
    let mut invalid = candidate;
    invalid.revision = 0;
    assert_eq!(
        preview(invalid, &[]),
        Err(PreviewError::InvalidRule(Error::InvalidRule))
    );
    assert_eq!(
        preview(
            candidate,
            &vec![covered(1, 10, 12); MAX_PREVIEW_OBSERVATIONS + 1]
        ),
        Err(PreviewError::TooManyObservations)
    );
    assert_eq!(active.checkpoint(), before);
}
