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

#[test]
fn finite_metric_preserves_exact_float_bits() {
    let bits = 7405554147052665218u64;
    let wire = serde_json::json!({"version":2,"registry":"fabrico11y/2","identity":{"producer":"precision","generation":"g","sequence":1},"timestamp_ns":1,"resource":"fixture","data":{"kind":"metric","name":"value","unit":"1","value":f64::from_bits(bits),"metric_type":"gauge","reset":false}});
    let decoded = fabric::observation::decode(&serde_json::to_vec(&wire).unwrap()).unwrap();
    if let fabric::observation::Data::Metric { value, .. } = decoded.data {
        assert_eq!(value.to_bits(), bits);
    } else {
        panic!("metric expected");
    }
}

#[test]
fn versioned_context_preserves_strict_v2_and_native_envelope_shape() {
    let v2=br#"{"version":2,"registry":"fabrico11y/2","identity":{"producer":"p","generation":"g","sequence":1},"timestamp_ns":18446744073709551615,"resource":"fixture","data":{"kind":"event","name":"event","message":"test"}}"#;
    let mut value: serde_json::Value = serde_json::from_slice(v2).unwrap();
    value["context"] = serde_json::Value::Null;
    assert!(observation::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    assert!(observation::decode_versioned(&serde_json::to_vec(&value).unwrap()).is_err());
    value["version"] = 3.into();
    value["registry"] = "fabrico11y/3".into();
    value["context"] = serde_json::json!({"run_id":"f404b128-2c19-412f-bcd0-823f9c1f1b31"});
    let decoded = observation::decode_versioned(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(decoded.version(), 3);
    assert_eq!(decoded.timestamp_ns(), u64::MAX);
    assert_eq!(serde_json::to_value(&decoded).unwrap(), value);
    for bad in [
        "local-name",
        "F404b128-2c19-412f-bcd0-823f9c1f1b31",
        "00000000-0000-0000-0000-000000000000",
    ] {
        value["context"]["run_id"] = bad.into();
        assert!(observation::decode_versioned(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    assert!(observation::decode(v2).is_ok());
}

#[test]
fn independent_v3_wire_contract_fixtures() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("../../registry/v3/fixtures.json")).unwrap();
    for case in cases.as_array().unwrap() {
        assert_eq!(
            observation::decode_versioned(&serde_json::to_vec(&case["wire"]).unwrap()).is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
    }
}

#[test]
fn v3_agent_and_tool_run_ids_must_equal_global_context() {
    for kind in ["agent_run", "tool_execution"] {
        let mut wire = serde_json::json!({"version":3,"registry":"fabrico11y/3","context":{"run_id":"f404b128-2c19-412f-bcd0-823f9c1f1b31"},"identity":{"producer":"p","generation":"g","sequence":1},"timestamp_ns":1,"resource":"fixture","data":{"kind":kind,"run_id":"local-label","duration_ns":1,"status":"ok"}});
        wire["data"][if kind == "agent_run" { "agent" } else { "tool" }] = "fixture".into();
        assert!(observation::decode_versioned(&serde_json::to_vec(&wire).unwrap()).is_err());
        wire["data"]["run_id"] = wire["context"]["run_id"].clone();
        assert!(observation::decode_versioned(&serde_json::to_vec(&wire).unwrap()).is_ok());
    }
}

#[test]
fn generated_v3_descriptor_extends_the_exact_immutable_v2_contract() {
    use sha2::{Digest, Sha256};
    let v2 = observation::DESCRIPTOR.as_bytes();
    let digest = format!("{:x}", Sha256::digest(v2));
    assert_eq!(
        digest,
        "8676053c09e04c426e130a883a529e4fc42dea96e3f850bc5ee545f374eb5f11"
    );
    let descriptor: serde_json::Value = serde_json::from_str(observation::DESCRIPTOR_V3).unwrap();
    assert_eq!(descriptor["registry"], observation::REGISTRY_V3);
    assert_eq!(descriptor["version"], observation::VERSION_V3);
    assert_eq!(descriptor["extends"], observation::REGISTRY);
    assert_eq!(descriptor["extends_descriptor_sha256"], digest);
    assert_eq!(descriptor["wire"]["max_bytes"], observation::MAX_FRAME);
    assert_eq!(descriptor["wire"]["max_depth"], observation::MAX_DEPTH);
    let groups = descriptor["semantics"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    let context = groups
        .iter()
        .find(|g| g["id"] == "registry.fabric.v3.context")
        .unwrap();
    let attributes = context["attributes"].as_array().unwrap();
    assert_eq!(attributes.len(), 1);
    assert_eq!(attributes[0]["name"], "fabric.context.run_id");
    assert_eq!(attributes[0]["type"], "string");
    assert_eq!(attributes[0]["requirement_level"], "required");
    let lease = groups
        .iter()
        .find(|g| g["id"] == "registry.fabric.v3.lease")
        .unwrap();
    let attributes = lease["attributes"].as_array().unwrap();
    assert!(
        attributes
            .iter()
            .any(|a| a["name"] == "fabric.generation" && a["type"] == "string")
    );
    assert!(
        attributes
            .iter()
            .any(|a| a["name"] == "fabric.lease.expires_unix_s" && a["type"] == "int")
    );
    for attribute in groups
        .iter()
        .flat_map(|g| g["attributes"].as_array().unwrap())
    {
        assert!(
            !attribute["type"].is_object(),
            "v3 must inherit data enums rather than create divergent copies"
        );
    }
}
