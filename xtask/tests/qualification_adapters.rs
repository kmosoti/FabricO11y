use std::path::PathBuf;
use std::process::Command;

#[test]
fn existing_qualification_adapter_modules_run_in_the_workspace_gate() {
    let qualification = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tools/qualification");
    let status = Command::new("python3")
        .args([
            "-B",
            "-m",
            "unittest",
            "-v",
            "test_console_bridge",
            "test_console_bridge_smoke",
            "test_console_firefox",
            "test_production_access",
            "test_release_runtime",
            "test_soak_r3",
            "test_owner_commands",
            "test_owner_phase_resume",
            "test_owner_walkthrough",
            "test_owner_evidence",
            "test_owner_guide",
        ])
        .current_dir(&qualification)
        .status()
        .expect("python3 is required for the existing qualification adapter tests");
    assert!(
        status.success(),
        "the existing Python qualification adapter modules failed (exit {:?})",
        status.code()
    );
}
