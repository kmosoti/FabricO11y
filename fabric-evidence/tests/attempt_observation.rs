use fabric_evidence::{
    application_result::{ApplicationResult, ReportScope},
    attempt_observation::{AttemptIdentity, AttemptObservation, SCHEMA, TerminalState},
    evidence_manifest::{ClockProvenance, ProvenancedTime},
};
use serde_json::{Value, json};
fn identity() -> AttemptIdentity {
    AttemptIdentity {
        attempt: "attempt-1".into(),
        candidate: "candidate-1".into(),
        intended_strategy: "strategy".into(),
        actual_artifact: "artifact".into(),
        diagnosis: "timeout".into(),
    }
}
fn body() -> Value {
    json!({"schema":SCHEMA,"attempt":"attempt-1","candidate":"candidate-1", "intended_strategy":"strategy","actual_artifact":"artifact","diagnosis":"timeout","state":"observed_failure"})
}
fn result(schema: &str, value: &Value) -> ApplicationResult {
    ApplicationResult::new(
        ReportScope::new("origin".into(), "producer".into()).unwrap(),
        "00000000-0000-4000-8000-000000000001".into(),
        None,
        schema.into(),
        ProvenancedTime {
            unix_ms: 1,
            clock: ClockProvenance {
                clock_id: "clock".into(),
                source: "fixture".into(),
                uncertainty_ms: None,
            },
        },
        &serde_json::to_vec(value).unwrap(),
    )
    .unwrap()
}
#[test]
fn attempt_observation_is_strictly_bound_reported_data() {
    let expected = identity();
    let valid = result(SCHEMA, &body());
    let decoded = AttemptObservation::from_result(&valid, &expected).unwrap();
    assert_eq!(decoded.identity(), expected);
    assert_eq!(decoded.state, TerminalState::ObservedFailure);
    let mut wrong = identity();
    wrong.diagnosis = "different".into();
    assert!(AttemptObservation::from_result(&valid, &wrong).is_err());
    let mut other_body = body();
    other_body["schema"] = json!("other.v1");
    assert!(
        AttemptObservation::from_result(&result("other.v1", &other_body), &identity()).is_err()
    );
    for (field, value) in [
        ("state", json!("success")),
        ("attempt", json!("")),
        ("candidate", json!("x".repeat(257))),
        ("diagnosis", json!("bad\ntext")),
        ("extra", json!(1)),
    ] {
        let mut changed = body();
        changed[field] = value;
        assert!(AttemptObservation::from_result(&result(SCHEMA, &changed), &identity()).is_err());
    }
}
