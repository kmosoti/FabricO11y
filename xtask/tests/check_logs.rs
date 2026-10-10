//! Receipts must retain the output they hash and must not hide failure.
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use xtask::checks::{self, Status};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let root = PathBuf::from(
            std::env::var_os("FABRIC_SCRATCH_ROOT").expect("resource launcher required"),
        )
        .join(format!(
            "ci-check-logs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn registry_retains_success_and_failure_output_with_matching_hashes() {
    let scratch = Scratch::new();
    let registry = scratch.0.join("checks.json");
    let receipts = scratch.0.join("receipts");
    let checks: Vec<_> = [("pass", 0), ("fail", 7)]
        .into_iter()
        .map(|(id, exit)| {
            json!({
            "id": id, "profile": "fixture", "scope": "synthetic output",
            "expected": "exit 0", "unchecked": "application behavior",
            "requires": ["python3"],
            "command": ["python3", "-c", format!(
                "import sys; print('stdout-sentinel'); print('stderr-sentinel', file=sys.stderr); sys.exit({exit})"
            )],
            })
        })
        .collect();
    std::fs::write(&registry, json!({"checks": checks}).to_string()).unwrap();
    assert_eq!(
        checks::run(&scratch.0, &registry, "fixture", None, &receipts).unwrap(),
        Status::Failed
    );
    assert!(!receipts.join("current-check.json").exists());
    for (id, status, exit) in [("pass", "passed", 0), ("fail", "failed", 7)] {
        let log = std::fs::read(receipts.join(format!("{id}.log"))).unwrap();
        assert_eq!(log, b"stdout-sentinel\nstderr-sentinel\n");
        let receipt: Value =
            serde_json::from_slice(&std::fs::read(receipts.join(format!("{id}.json"))).unwrap())
                .unwrap();
        assert_eq!(
            receipt["output_sha256"],
            "9f4476279ed17e444616ad982db5eb6cd4a34a8a3db47ea94415038e60e56734"
        );
        assert_eq!(receipt["output_log"], format!("{id}.log"));
        assert_eq!(receipt["result"], status);
        assert_eq!(receipt["exit_code"], exit);
    }
}
