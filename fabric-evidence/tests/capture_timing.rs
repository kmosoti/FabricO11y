//! Local transform-only comparison; no storage, sockets or custody work.
use fabric_evidence::capture_policy::{
    CaptureAction, CaptureError, CapturePolicy, FieldRule, MAX_CAPTURE_PATH_BYTES, Omission,
    PathRule, ScalarType,
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

const MAX_INPUT_BYTES: usize = 4096;
const MAX_PATH_RULES: usize = 64;
const MAX_PATH_SEGMENT_BYTES: usize = 128;
const PATH_WORK_PER_BYTE: usize = 65;
const PARSE_WORK_PER_INPUT_BYTE: usize = 8;
const WORK_RESERVATION_OVERHEAD: usize = 32;

fn maximum_work_fixture(rule_count: usize, work_slack: usize) -> (CapturePolicy, String, usize) {
    assert!((1..=MAX_PATH_RULES).contains(&rule_count));
    let keys: Vec<String> = (0..MAX_PATH_RULES)
        .map(|index| format!("r{index:03}_{}", "a".repeat(MAX_PATH_SEGMENT_BYTES - 5)))
        .collect();
    assert!(keys.iter().all(|key| key.len() == MAX_PATH_SEGMENT_BYTES));
    assert_eq!(
        keys.iter().map(String::len).sum::<usize>(),
        MAX_CAPTURE_PATH_BYTES
    );

    let paths: Vec<PathRule> = keys[..rule_count]
        .iter()
        .map(|key| PathRule {
            path: vec![key.clone()],
            action: CaptureAction::Scalar {
                scalar_type: ScalarType::Integer,
            },
        })
        .collect();
    let input = |pad_bytes| format!(r#"{{"{}":7,"PAD":"{}"}}"#, keys[0], "x".repeat(pad_bytes));
    let fixture = input(MAX_INPUT_BYTES - input(0).len());
    assert_eq!(fixture.len(), MAX_INPUT_BYTES);

    let reserved_work = fixture
        .len()
        .checked_mul(PARSE_WORK_PER_INPUT_BYTE)
        .and_then(|work| {
            paths.iter().try_fold(work, |total, rule| {
                rule.path.iter().try_fold(total, |total, segment| {
                    total.checked_add(segment.len().checked_mul(PATH_WORK_PER_BYTE)?)
                })
            })
        })
        .and_then(|work| work.checked_add(WORK_RESERVATION_OVERHEAD))
        .expect("bounded fixture reservation");
    let policy = CapturePolicy::new_paths(
        format!("max-work-{rule_count}"),
        MAX_INPUT_BYTES,
        reserved_work
            .checked_sub(work_slack)
            .expect("valid work slack"),
        paths,
    )
    .unwrap();
    (policy, fixture, reserved_work)
}

#[test]
fn maximum_path_work_is_deterministic_and_fails_one_unit_over() {
    let (exact, fixture, max_reservation) = maximum_work_fixture(MAX_PATH_RULES, 0);
    let (below, below_fixture, below_reservation) = maximum_work_fixture(MAX_PATH_RULES, 1);
    assert_eq!(fixture, below_fixture);
    assert_eq!(below_reservation, max_reservation);
    assert_eq!(max_reservation, 565_280);
    assert_eq!(
        below.capture(fixture.as_bytes()),
        Err(CaptureError::WorkLimit)
    );

    let expected = format!(
        r#"{{"{}":7}}"#,
        "r000_".to_owned() + &"a".repeat(MAX_PATH_SEGMENT_BYTES - 5)
    );
    let captured = exact.capture(fixture.as_bytes()).unwrap();
    assert_eq!(captured.bytes, expected.as_bytes());
    assert_eq!(captured.digest.0.hex, hex(&captured.bytes));
    assert_eq!(captured.policy_digest.0.hex, hex(&exact.canonical_bytes()));
    assert_eq!(captured.omissions, [Omission::PolicyExcluded]);
    assert!(
        !serde_json::to_vec(&captured)
            .unwrap()
            .windows(3)
            .any(|window| window == b"PAD")
    );
}

#[test]
#[ignore = "release-profile maximum path-work timing, isolated from custody"]
fn matched_maximum_path_work_timing() {
    const ITERATIONS: usize = 250;
    const REPEATS: usize = 5;

    let (one_rule, one_fixture, one_reservation) = maximum_work_fixture(1, 0);
    let (max_rules, max_fixture, max_reservation) = maximum_work_fixture(MAX_PATH_RULES, 0);
    assert_eq!(one_fixture, max_fixture);
    let expected = one_rule.capture(one_fixture.as_bytes()).unwrap();
    let max_captured = max_rules.capture(max_fixture.as_bytes()).unwrap();
    assert_eq!(expected.bytes, max_captured.bytes);
    assert_eq!(expected.digest, max_captured.digest);
    assert_eq!(expected.omissions, max_captured.omissions);
    assert_eq!(one_fixture.len(), MAX_INPUT_BYTES);
    assert_eq!(max_reservation, 565_280);
    assert_eq!(one_reservation, 41_120);

    let (max_work_below, below_fixture, below_reservation) =
        maximum_work_fixture(MAX_PATH_RULES, 1);
    assert_eq!(max_fixture, below_fixture);
    assert_eq!(max_reservation, below_reservation);
    assert_eq!(
        max_work_below.capture(max_fixture.as_bytes()),
        Err(CaptureError::WorkLimit)
    );

    for policy in [&one_rule, &max_rules] {
        black_box(policy.capture(black_box(one_fixture.as_bytes())).unwrap());
    }
    for repeat in 0..REPEATS {
        let order = if repeat % 2 == 0 {
            [
                ("one_rule", &one_rule, one_reservation),
                ("max_rules", &max_rules, max_reservation),
            ]
        } else {
            [
                ("max_rules", &max_rules, max_reservation),
                ("one_rule", &one_rule, one_reservation),
            ]
        };
        for (case, policy, reserved_work) in order {
            let started = Instant::now();
            let mut digest = String::new();
            let mut policy_digest = String::new();
            for _ in 0..ITERATIONS {
                let captured =
                    black_box(policy.capture(black_box(one_fixture.as_bytes())).unwrap());
                digest = captured.digest.0.hex;
                policy_digest = captured.policy_digest.0.hex;
                black_box(captured.bytes);
            }
            println!(
                "FABRIC239_MAX_WORK {{\"repeat\":{repeat},\"case\":\"{case}\",\"input_bytes\":{},\"reserved_work\":{reserved_work},\"captured_bytes\":{},\"artifact_digest\":\"{digest}\",\"policy_digest\":\"{policy_digest}\",\"iterations\":{ITERATIONS},\"elapsed_ns\":{}}}",
                one_fixture.len(),
                expected.bytes.len(),
                started.elapsed().as_nanos()
            );
        }
    }
}
