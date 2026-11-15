use std::{fs, path::Path};

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

#[test]
fn main_human_gate_and_ci_workflows_enforce_promotion_invariants() {
    let gate_path = repo_root().join(".github/workflows/main-human-gate.yml");
    let gate = fs::read_to_string(&gate_path).expect("read main-human-gate.yml");

    // Human approval deployment gate runs only on develop pushes (protected by environment branch policy).
    assert!(gate.contains("push:"));
    assert!(gate.contains("- develop"));
    assert!(!gate.contains("pull_request:"));
    assert!(!gate.contains("actions/checkout"));
    assert!(gate.contains("permissions:\n  contents: read"));
    assert!(gate.contains("main-human-approval:"));
    assert!(gate.contains("name: main-human-approval"));
    assert!(gate.contains("environment: main-release"));
    assert!(gate.contains("RELEASE_REF\" != \"refs/heads/develop\""));

    // Promotion source check validates main-targeted PRs originate from same-repo develop.
    let src_path = repo_root().join(".github/workflows/main-promotion-source.yml");
    let src = fs::read_to_string(&src_path).expect("read main-promotion-source.yml");
    assert!(src.contains("pull_request_target:"));
    assert!(src.contains("pull_request:"));
    assert!(src.contains("- main"));
    assert!(!src.contains("actions/checkout"));
    assert!(src.contains("permissions:\n  contents: read"));
    assert!(src.contains("verify-promotion-source:"));
    assert!(src.contains("name: verify-promotion-source"));
    assert!(src.contains("github.base_ref"));
    assert!(src.contains("github.head_ref"));
    assert!(src.contains("github.event.pull_request.head.repo.full_name"));
    assert!(src.contains("BASE_REF\" != \"main\""));
    assert!(src.contains("RELEASE_HEAD_REF\" != \"develop\""));
    assert!(src.contains("HEAD_REPO\" != \"$BASE_REPO\""));

    // Development checks must remain bound to both develop and main with verify & native-acceptance.
    let dev_path = repo_root().join(".github/workflows/development-checks.yml");
    let dev = fs::read_to_string(&dev_path).expect("read development-checks.yml");
    assert!(dev.contains("branches: [main, develop]"));
    assert!(dev.contains("verify:"));
    assert!(dev.contains("native-acceptance:"));
}
