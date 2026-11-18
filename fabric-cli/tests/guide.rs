use std::process::Command;

#[test]
fn guide_does_not_open_storage_or_require_a_daemon() {
    let result = Command::new(env!("CARGO_BIN_EXE_fabric"))
        .args([
            "--data-dir",
            "/tmp/fabric-guide-unusable-storage",
            "--tmp-dir",
            "/tmp",
            "guide",
            "fleet",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["command"], "guide");
    assert_eq!(report["data"]["topics"][0]["id"], "fleet");
    assert_eq!(
        report["data"]["topics"][0]["proofs"][0]["kind"],
        "independent_model"
    );
    assert!(report["data"]["evidence"].is_null());
}

#[test]
fn guide_rejects_special_evidence_files() {
    let result = Command::new(env!("CARGO_BIN_EXE_fabric"))
        .args(["guide", "--evidence", "/dev/null", "--json"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("evidence must be a regular file"));
}
