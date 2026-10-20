use std::path::PathBuf;
use std::process::Command;

#[test]
fn qualification_and_native_custody_adapters_run_with_the_built_helper() {
    let qualification = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tools/qualification");
    let helper = PathBuf::from(env!("CARGO_BIN_EXE_xtask"));
    let helper_sha = xtask::sha256_hex(&std::fs::read(&helper).expect("read actual xtask executable"));
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
            "test_custody_native",
            "test_custody_capture",
        ])
        .current_dir(&qualification)
        .env("FABRIC_CUSTODY_CUTOFF_HELPER", &helper)
        .env("FABRIC_CUSTODY_CUTOFF_HELPER_SHA256", helper_sha)
        .status()
        .expect("python3 is required for the existing qualification adapter tests");
    assert!(
        status.success(),
        "the existing Python qualification adapter modules failed (exit {:?})",
        status.code()
    );
}
