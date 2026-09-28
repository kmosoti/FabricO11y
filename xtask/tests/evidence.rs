//! Negative controls for the counterexample registry check.

use std::path::Path;
use xtask::evidence::{MISSING_REPRODUCER, UNKNOWN_FIX, check_counterexamples};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

fn registry(test: &str, commit: &str) -> std::path::PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("cx-{test}-{commit}.json"));
    let json = format!(
        r#"{{"counterexamples": [{{"id": "CX-T", "origin": "fixture", "defect": "d", "seed_or_trace": "s", "contract": "DEL-4",
        "reproducer": {{"file": "crates/fabric-core/src/delivery.rs", "test": "{test}"}}, "fix_commit": "{commit}"}}]}}"#
    );
    std::fs::write(&path, json).unwrap();
    path
}

#[test]
fn a_real_reproducer_and_commit_pass() {
    let v = check_counterexamples(
        root(),
        &registry("an_exhausted_strand_accepts_no_successor", "c355a2c"),
    )
    .unwrap();
    assert_eq!(v, vec![]);
}

#[test]
fn a_missing_reproducer_test_fails() {
    let v = check_counterexamples(root(), &registry("no_such_regression_test", "c355a2c")).unwrap();
    assert_eq!(
        v.iter().map(|v| v.code).collect::<Vec<_>>(),
        vec![MISSING_REPRODUCER]
    );
}

#[test]
fn an_unknown_fix_commit_fails() {
    let v = check_counterexamples(
        root(),
        &registry(
            "an_exhausted_strand_accepts_no_successor",
            "0000000deadbeef",
        ),
    )
    .unwrap();
    assert_eq!(
        v.iter().map(|v| v.code).collect::<Vec<_>>(),
        vec![UNKNOWN_FIX]
    );
}
