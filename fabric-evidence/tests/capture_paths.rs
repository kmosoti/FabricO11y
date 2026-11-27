use fabric_evidence::capture_policy::{
    CaptureAction, CaptureError, CapturePolicy, FieldRule, MAX_CAPTURE_ARRAY_ITEMS,
    MAX_CAPTURE_PATH_DEPTH, MAX_CAPTURE_PATH_WORK, Omission, PathRule, ScalarType,
};

fn rule(path: &[&str], action: CaptureAction) -> PathRule {
    PathRule {
        path: path.iter().map(|s| (*s).to_owned()).collect(),
        action,
    }
}
fn policy() -> CapturePolicy {
    CapturePolicy::new_paths(
        "nested".into(),
        4096,
        MAX_CAPTURE_PATH_WORK,
        vec![
            rule(
                &["env", "REGION"],
                CaptureAction::Scalar {
                    scalar_type: ScalarType::Text,
                },
            ),
            rule(&["env", "TOKEN"], CaptureAction::Redact),
            rule(
                &["config", "KEY.WITH.DOTS"],
                CaptureAction::Scalar {
                    scalar_type: ScalarType::Integer,
                },
            ),
            rule(
                &["config", "slash/key"],
                CaptureAction::Scalar {
                    scalar_type: ScalarType::Boolean,
                },
            ),
            rule(
                &["samples"],
                CaptureAction::ScalarArray {
                    scalar_type: ScalarType::Integer,
                    max_items: 3,
                },
            ),
        ],
    )
    .unwrap()
}
#[test]
fn nested_paths_redact_without_suppressed_name_value_or_digest_leaks() {
    let first = br#"{"env":{"REGION":"west","TOKEN":"SECRET-ONE","SECRET-NAME":"HIDDEN-ONE"},"config":{"KEY.WITH.DOTS":7,"slash/key":true,"omitted-secret":"PRIVATE"},"samples":[1,-2,3],"discarded":{"secret":"HIDDEN"}}"#;
    let second = br#"{"env":{"REGION":"west","TOKEN":{"different":"SECRET-TWO"},"other-name":"HIDDEN-TWO"},"config":{"KEY.WITH.DOTS":7,"slash/key":true,"another":"PRIVATE-TWO"},"samples":[1,-2,3],"something":"ELSE"}"#;
    let a = policy().capture(first).unwrap();
    let b = policy().capture(second).unwrap();
    assert_eq!(a.bytes, br#"{"config":{"KEY.WITH.DOTS":7,"slash/key":true},"env":{"REGION":"west","TOKEN":"[REDACTED]"},"samples":[1,-2,3]}"#);
    assert_eq!(a.bytes, b.bytes);
    assert_eq!(a.digest, b.digest);
    assert_eq!(a.policy_digest, b.policy_digest);
    assert_eq!(
        a.omissions,
        vec![Omission::PolicyExcluded, Omission::Redacted]
    );
    assert_eq!(a.omissions, b.omissions);
    for forbidden in [
        "SECRET-ONE",
        "SECRET-NAME",
        "HIDDEN-ONE",
        "omitted-secret",
        "PRIVATE",
        "discarded",
        "HIDDEN",
    ] {
        assert!(!serde_json::to_string(&a).unwrap().contains(forbidden));
    }
    assert_eq!(
        CapturePolicy::decode(&policy().canonical_bytes()).unwrap(),
        policy()
    );
}
#[test]
fn arrays_are_explicit_typed_bounded_and_never_partially_emitted() {
    let p = policy();
    let wrong_type = p.capture(br#"{"samples":[1,"HIDDEN",3]}"#).unwrap();
    assert_eq!(wrong_type.bytes, b"{}");
    assert_eq!(wrong_type.omissions, vec![Omission::TypeNotPermitted]);
    let oversized = p.capture(br#"{"samples":[1,2,3,4]}"#).unwrap();
    assert_eq!(oversized.bytes, b"{}");
    assert_eq!(oversized.omissions, vec![Omission::ValueLimit]);
    let nested = p.capture(br#"{"env":"HIDDEN"}"#).unwrap();
    assert_eq!(nested.bytes, b"{}");
    assert_eq!(nested.omissions, vec![Omission::TypeNotPermitted]);
    let deny = CapturePolicy::new_paths("deny".into(), 4096, 65_536, vec![]).unwrap();
    assert_eq!(deny.capture(br#"{"samples":[1,2]}"#).unwrap().bytes, b"{}");
}
#[test]
fn duplicate_prefix_wildcard_depth_work_and_wire_conflicts_fail_closed() {
    let leaf = CaptureAction::Scalar {
        scalar_type: ScalarType::Text,
    };
    for rules in [
        vec![
            rule(&["env"], leaf.clone()),
            rule(&["env", "name"], leaf.clone()),
        ],
        vec![
            rule(&["env", "name"], leaf.clone()),
            rule(&["env", "name"], CaptureAction::Redact),
        ],
        vec![rule(&["env", "*"], leaf.clone())],
    ] {
        assert_eq!(
            CapturePolicy::new_paths("p".into(), 4096, MAX_CAPTURE_PATH_WORK, rules),
            Err(CaptureError::InvalidPolicy)
        );
    }
    // Reject an oversized path before walking even empty segments.
    assert_eq!(
        CapturePolicy::new_paths(
            "p".into(),
            4096,
            MAX_CAPTURE_PATH_WORK,
            vec![PathRule {
                path: vec![String::new(); 1024],
                action: leaf.clone()
            }],
        ),
        Err(CaptureError::InvalidPolicy)
    );
    // Every individual segment fits, but their aggregate exceeds the policy cap.
    let many = (0..64)
        .map(|i| PathRule {
            path: vec![format!("{i:02}{}", "x".repeat(126)), "x".repeat(128)],
            action: leaf.clone(),
        })
        .collect();
    assert_eq!(
        CapturePolicy::new_paths("p".into(), 4096, MAX_CAPTURE_PATH_WORK, many),
        Err(CaptureError::InvalidPolicy)
    );
    let deep = vec!["x"; MAX_CAPTURE_PATH_DEPTH + 1];
    assert_eq!(
        CapturePolicy::new_paths(
            "p".into(),
            4096,
            MAX_CAPTURE_PATH_WORK,
            vec![rule(&deep, leaf)]
        ),
        Err(CaptureError::InvalidPolicy)
    );
    assert_eq!(
        CapturePolicy::new_paths(
            "p".into(),
            4096,
            MAX_CAPTURE_PATH_WORK,
            vec![rule(
                &["sample"],
                CaptureAction::ScalarArray {
                    scalar_type: ScalarType::Text,
                    max_items: (MAX_CAPTURE_ARRAY_ITEMS + 1) as u8
                }
            )]
        ),
        Err(CaptureError::InvalidPolicy)
    );
    let tiny = CapturePolicy::new_paths(
        "p".into(),
        4096,
        32,
        vec![rule(&["a", "b"], CaptureAction::Redact)],
    )
    .unwrap();
    assert_eq!(
        tiny.capture(br#"{"a":{"b":"private"}}"#),
        Err(CaptureError::WorkLimit)
    );
    let legacy = CapturePolicy::new(
        "policy-a".into(),
        4096,
        65536,
        vec![FieldRule {
            name: "allowed".into(),
            scalar_type: ScalarType::Integer,
        }],
    )
    .unwrap();
    assert_eq!(
        legacy.digest().0.hex,
        "96ccd3d813c73f9410069c1ad791aa80fa2cd5675cbf5b4028eadfce567759ea"
    );
    let mut wire: serde_json::Value = serde_json::from_slice(&legacy.canonical_bytes()).unwrap();
    wire["paths"] = serde_json::Value::Null;
    assert_eq!(
        CapturePolicy::decode(&serde_json::to_vec(&wire).unwrap()),
        Err(CaptureError::InvalidPolicy)
    );
}

#[test]
fn escaped_names_are_literal_and_duplicate_keys_never_reach_capture() {
    let p = CapturePolicy::new_paths(
        "keys".into(),
        4096,
        MAX_CAPTURE_PATH_WORK,
        vec![
            rule(
                &["env", "café"],
                CaptureAction::Scalar {
                    scalar_type: ScalarType::Text,
                },
            ),
            rule(
                &["env", "a.b"],
                CaptureAction::Scalar {
                    scalar_type: ScalarType::Text,
                },
            ),
        ],
    )
    .unwrap();
    let out = p
        .capture(br#"{"env":{"caf\u00e9":"ok","a.b":"literal","a":{"b":"HIDDEN"}}}"#)
        .unwrap();
    assert_eq!(
        out.bytes,
        "{\"env\":{\"a.b\":\"literal\",\"café\":\"ok\"}}".as_bytes()
    );
    assert_eq!(out.omissions, vec![Omission::PolicyExcluded]);
    assert_eq!(
        p.capture(r#"{"env":{"café":"first","caf\u00e9":"SECOND-SECRET"}}"#.as_bytes()),
        Err(CaptureError::InvalidInput)
    );
}
