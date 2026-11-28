//! Local transform-only comparison; no storage, sockets or custody work.
use fabric_evidence::capture_policy::{
    CaptureAction, CaptureError, CapturePolicy, FieldRule, Omission, PathRule, ScalarType,
};
use sha2::{Digest, Sha256};
use std::{hint::black_box, time::Instant};

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn baseline(input: &[u8], policy_bytes: &[u8]) -> (Vec<u8>, String, String) {
    let value: serde_json::Value = serde_json::from_slice(input).unwrap();
    let bytes = serde_json::to_vec(&value).unwrap();
    let artifact = hex(&bytes);
    let policy = hex(policy_bytes);
    (bytes, artifact, policy)
}

#[test]
#[ignore = "release-profile transform timing, isolated from custody"]
fn matched_capture_transform_timing() {
    let policy = CapturePolicy::new(
        "bench".into(),
        4096,
        65536,
        vec![
            FieldRule {
                name: "count".into(),
                scalar_type: ScalarType::Integer,
            },
            FieldRule {
                name: "label".into(),
                scalar_type: ScalarType::Text,
            },
        ],
    )
    .unwrap();
    let allowed = br#"{"label":"public","count":42}"#;
    let mixed = br#"{"label":"public","count":42,"PRIVATE-NAME":"PRIVATE-VALUE"}"#;
    let policy_bytes = policy.canonical_bytes();
    let expected = baseline(allowed, &policy_bytes);
    let captured = policy.capture(allowed).unwrap();
    assert_eq!(captured.bytes, expected.0);
    assert_eq!(captured.digest.0.hex, expected.1);
    assert_eq!(captured.policy_digest.0.hex, expected.2);
    assert!(captured.omissions.is_empty());
    assert_eq!(policy.capture(mixed).unwrap().bytes, expected.0);
    for _ in 0..1000 {
        black_box(policy.capture(black_box(allowed)).unwrap());
        black_box(baseline(black_box(allowed), black_box(&policy_bytes)));
    }
    for repeat in 0..7 {
        for mode in if repeat % 2 == 0 {
            ["baseline", "capture"]
        } else {
            ["capture", "baseline"]
        } {
            let start = Instant::now();
            for _ in 0..5000 {
                if mode == "baseline" {
                    black_box(baseline(black_box(allowed), black_box(&policy_bytes)));
                } else {
                    black_box(policy.capture(black_box(allowed)).unwrap());
                }
            }
            println!(
                "FABRIC239_TIMING {{\"repeat\":{repeat},\"mode\":\"{mode}\",\"iterations\":5000,\"elapsed_ns\":{}}}",
                start.elapsed().as_nanos()
            );
        }
        let start = Instant::now();
        for _ in 0..5000 {
            black_box(policy.capture(black_box(mixed)).unwrap());
        }
        println!(
            "FABRIC239_TIMING {{\"repeat\":{repeat},\"mode\":\"mixed_capture\",\"iterations\":5000,\"elapsed_ns\":{}}}",
            start.elapsed().as_nanos()
        );
    }
}

#[test]
#[ignore = "release-profile nested capture timing, isolated from custody"]
fn nested_capture_input_budget_timing() {
    const LIMIT: usize = 4096;
    const ITERATIONS: usize = 5000;
    let rules = vec![
        PathRule {
            path: vec!["env".into(), "REGION".into()],
            action: CaptureAction::Scalar {
                scalar_type: ScalarType::Text,
            },
        },
        PathRule {
            path: vec!["env".into(), "TOKEN".into()],
            action: CaptureAction::Redact,
        },
    ];
    let policy =
        CapturePolicy::new_paths("nested-timing".into(), LIMIT, 65_536, rules.clone()).unwrap();
    let input = |size| {
        format!(
            r#"{{"env":{{"REGION":"west","TOKEN":"input-token","PAD":"{}"}}}}"#,
            "x".repeat(size)
        )
    };
    let small = input(1);
    let boundary = input(LIMIT - input(0).len());
    assert_eq!(boundary.len(), LIMIT);
    let expected = br#"{"env":{"REGION":"west","TOKEN":"[REDACTED]"}}"#;
    let policy_digest = hex(&policy.canonical_bytes());
    for fixture in [&small, &boundary] {
        let captured = policy.capture(fixture.as_bytes()).unwrap();
        assert_eq!(captured.bytes, expected);
        assert_eq!(captured.digest.0.hex, hex(expected));
        assert_eq!(captured.policy_digest.0.hex, policy_digest);
        assert_eq!(
            captured.omissions,
            [Omission::PolicyExcluded, Omission::Redacted]
        );
    }
    assert_eq!(
        policy.capture(format!("{boundary} ").as_bytes()),
        Err(CaptureError::InputSize)
    );
    let limited = CapturePolicy::new_paths("nested-timing".into(), LIMIT, 1, rules).unwrap();
    assert_eq!(
        limited.capture(small.as_bytes()),
        Err(CaptureError::WorkLimit)
    );

    let workload = [
        ("small", small.as_bytes()),
        ("input_budget", boundary.as_bytes()),
    ];
    for (_, fixture) in workload {
        for _ in 0..1000 {
            black_box(policy.capture(black_box(fixture)).unwrap());
        }
    }
    for repeat in 0..7 {
        for index in [repeat % 2, (repeat + 1) % 2] {
            let (case, fixture) = workload[index];
            let started = Instant::now();
            for _ in 0..ITERATIONS {
                black_box(policy.capture(black_box(fixture)).unwrap());
            }
            println!(
                "FABRIC239_NESTED {{\"repeat\":{repeat},\"case\":\"{case}\",\"input_bytes\":{},\"captured_bytes\":{},\"iterations\":{ITERATIONS},\"elapsed_ns\":{}}}",
                fixture.len(),
                expected.len(),
                started.elapsed().as_nanos()
            );
        }
    }
}
