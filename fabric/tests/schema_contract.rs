use fabric::observation;

#[test]
fn independent_wire_contract_fixtures() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("../../registry/fixtures/observations.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let bytes = serde_json::to_vec(&case["wire"]).unwrap();
        assert_eq!(
            observation::decode(&bytes).is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
    }
}

#[test]
fn escaped_delimiters_are_not_nesting_and_extreme_numbers_are_rejected() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../registry/fixtures/observations.json")).unwrap();
    let mut log = fixture[3]["wire"].clone();
    log["data"]["message"] = "[[[{{{\\\"".repeat(100).into();
    assert!(observation::decode(&serde_json::to_vec(&log).unwrap()).is_ok());
    let metric = serde_json::to_string(&fixture[6]["wire"]).unwrap();
    let overflow = metric.replace("\"value\":1", "\"value\":1e9999");
    assert_ne!(metric, overflow);
    assert!(observation::decode(overflow.as_bytes()).is_err());
    let deep = "[".repeat(observation::MAX_DEPTH + 1) + &"]".repeat(observation::MAX_DEPTH + 1);
    assert!(observation::decode(deep.as_bytes()).is_err());
}
