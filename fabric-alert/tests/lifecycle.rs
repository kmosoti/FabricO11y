use fabric_alert::{
    Decision, Direction, Error, Evaluation, Lifecycle, Outcome, Reducer, Rule, Sample, Validity,
};
fn rule(direction: Direction) -> Rule {
    Rule {
        rule_id: [1; 16],
        instance_id: [2; 16],
        revision: 1,
        profile: [3; 32],
        source: [4; 32],
        direction,
        trigger: if direction == Direction::Above {
            10
        } else {
            -10
        },
        recovery: if direction == Direction::Above { 5 } else { -5 },
        firing_hold: 5,
        recovery_hold: 3,
    }
}
fn covered(frontier: u64, time: u64, start: u64, value: i64) -> Evaluation {
    Evaluation {
        profile: [3; 32],
        source: [4; 32],
        frontier,
        time,
        sample: Sample::Covered {
            value,
            evidence: [frontier as u8; 32],
            coverage_start: start,
            coverage_end: time,
        },
    }
}
fn missing(frontier: u64, time: u64, validity: Validity) -> Evaluation {
    Evaluation {
        profile: [3; 32],
        source: [4; 32],
        frontier,
        time,
        sample: Sample::Missing(validity),
    }
}
fn step(reducer: &mut Reducer, e: Evaluation) -> Decision {
    match reducer.evaluate(e).unwrap() {
        Outcome::Applied(d) => d,
        Outcome::Duplicate(_) => panic!("unexpected duplicate"),
    }
}
#[test]
fn holds_hysteresis_and_boundaries_above() {
    let mut r = Reducer::new(rule(Direction::Above)).unwrap();
    assert_eq!(
        step(&mut r, covered(1, 10, 10, 10)).lifecycle,
        Lifecycle::Pending
    );
    assert_eq!(
        step(&mut r, covered(2, 14, 10, 11)).lifecycle,
        Lifecycle::Pending
    );
    assert_eq!(
        step(&mut r, covered(3, 15, 14, 10)).lifecycle,
        Lifecycle::Firing
    );
    assert_eq!(
        step(&mut r, covered(4, 16, 15, 6)).lifecycle,
        Lifecycle::Firing
    );
    assert_eq!(
        step(&mut r, covered(5, 17, 16, 5)).lifecycle,
        Lifecycle::Recovering
    );
    assert_eq!(
        step(&mut r, covered(6, 19, 17, 5)).lifecycle,
        Lifecycle::Recovering
    );
    assert_eq!(
        step(&mut r, covered(7, 20, 19, 5)).lifecycle,
        Lifecycle::Resolved
    );
}
#[test]
fn below_direction_and_extreme_values() {
    let mut r = Reducer::new(rule(Direction::Below)).unwrap();
    assert_eq!(
        step(&mut r, covered(1, 1, 1, i64::MIN)).lifecycle,
        Lifecycle::Pending
    );
    assert_eq!(
        step(&mut r, covered(2, 6, 1, -10)).lifecycle,
        Lifecycle::Firing
    );
    assert_eq!(
        step(&mut r, covered(3, 7, 6, -5)).lifecycle,
        Lifecycle::Recovering
    );
    assert_eq!(
        step(&mut r, covered(4, 10, 7, i64::MAX)).lifecycle,
        Lifecycle::Resolved
    );
}
#[test]
fn all_missingness_breaks_holds_and_never_resolves_firing() {
    for validity in [
        Validity::NoData,
        Validity::Stale,
        Validity::Partial,
        Validity::Error,
        Validity::Unavailable,
        Validity::Denied,
    ] {
        let mut r = Reducer::new(rule(Direction::Above)).unwrap();
        step(&mut r, covered(1, 1, 1, 10));
        let d = step(&mut r, missing(1, 9, validity));
        assert_eq!(d.lifecycle, Lifecycle::Inactive);
        assert_eq!(d.validity, validity);
        assert_eq!(
            step(&mut r, covered(2, 10, 1, 10)).lifecycle,
            Lifecycle::Pending
        );
        assert_eq!(
            step(&mut r, covered(3, 15, 10, 10)).lifecycle,
            Lifecycle::Firing
        );
        assert_eq!(
            step(&mut r, covered(4, 16, 15, 5)).lifecycle,
            Lifecycle::Recovering
        );
        let d = step(&mut r, missing(4, 25, validity));
        assert_eq!(d.lifecycle, Lifecycle::Firing);
        assert_eq!(d.validity, validity);
        assert_eq!(
            step(&mut r, covered(5, 26, 16, 5)).lifecycle,
            Lifecycle::Recovering
        );
    }
}
#[test]
fn uncovered_gap_restarts_pending_and_recovery() {
    let mut r = Reducer::new(rule(Direction::Above)).unwrap();
    step(&mut r, covered(1, 10, 10, 10));
    assert_eq!(
        step(&mut r, covered(2, 30, 29, 10)).lifecycle,
        Lifecycle::Pending
    );
    assert_eq!(
        step(&mut r, covered(3, 35, 30, 10)).lifecycle,
        Lifecycle::Firing
    );
    step(&mut r, covered(4, 36, 35, 5));
    assert_eq!(
        step(&mut r, covered(5, 50, 49, 5)).lifecycle,
        Lifecycle::Recovering
    );
    assert_eq!(
        step(&mut r, covered(6, 53, 50, 5)).lifecycle,
        Lifecycle::Resolved
    );
}
#[test]
fn replay_conflicts_and_invalid_inputs_leave_state_unchanged() {
    let mut r = Reducer::new(rule(Direction::Above)).unwrap();
    let first = covered(5, 10, 10, 10);
    let d = step(&mut r, first);
    assert_eq!(r.evaluate(first), Ok(Outcome::Duplicate(d)));
    for (input, error) in [
        (covered(5, 10, 10, 11), Error::ConflictingReplay),
        (covered(6, 9, 9, 11), Error::OlderTime),
        (covered(4, 11, 10, 11), Error::OlderFrontier),
        (covered(5, 11, 10, 10), Error::ReusedFrontier),
    ] {
        let before = r.clone();
        assert_eq!(r.evaluate(input), Err(error));
        assert_eq!(r, before);
    }
    let mut wrong = covered(6, 11, 10, 10);
    wrong.profile = [4; 32];
    assert_eq!(r.evaluate(wrong), Err(Error::WrongProfile));
    let mut source = covered(6, 11, 10, 10);
    source.source = [9; 32];
    assert_eq!(r.evaluate(source), Err(Error::WrongSource));
    let mut bad = covered(6, 11, 10, 10);
    bad.sample = Sample::Covered {
        value: 10,
        evidence: [1; 32],
        coverage_start: 10,
        coverage_end: 12,
    };
    assert_eq!(r.evaluate(bad), Err(Error::InvalidCoverage));
    assert_eq!(
        r.evaluate(missing(6, 11, Validity::Covered)),
        Err(Error::InvalidValidity)
    );
    assert_eq!(r.last(), Some(d));
}
#[test]
fn zero_holds_and_u64_boundary() {
    let mut spec = rule(Direction::Above);
    spec.firing_hold = 0;
    spec.recovery_hold = 0;
    let mut r = Reducer::new(spec).unwrap();
    assert_eq!(
        step(&mut r, covered(1, u64::MAX - 1, u64::MAX - 1, 10)).lifecycle,
        Lifecycle::Firing
    );
    assert_eq!(
        step(&mut r, covered(2, u64::MAX, u64::MAX - 1, 5)).lifecycle,
        Lifecycle::Resolved
    );
}
#[test]
fn invalid_hysteresis_and_revision_are_rejected() {
    let mut spec = rule(Direction::Above);
    spec.recovery = spec.trigger;
    assert_eq!(Reducer::new(spec), Err(Error::InvalidRule));
    spec = rule(Direction::Below);
    spec.recovery = spec.trigger - 1;
    assert_eq!(Reducer::new(spec), Err(Error::InvalidRule));
    spec = rule(Direction::Above);
    spec.revision = 0;
    assert_eq!(Reducer::new(spec), Err(Error::InvalidRule));
}
