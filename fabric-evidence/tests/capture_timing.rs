//! Local transform-only comparison; no storage, sockets or custody work.
use fabric_evidence::capture_policy::{CapturePolicy, FieldRule, ScalarType};
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
