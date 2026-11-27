use fabric_core::{CliEnvelope, DoctorReport};
use std::process::Command;

#[test]
fn offline_doctor_emits_typed_versioned_diagnostic() {
    let base = std::env::temp_dir().join(format!("fabric-doctor-schema-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_fabric"))
        .args(["--json", "doctor"])
        .env("FABRIC_DATA_DIR", base.join("data"))
        .env("FABRIC_TMP_DIR", &base)
        .env("XDG_CONFIG_HOME", base.join("config"))
        .env_remove("SPINDLE_SOCKET")
        .env_remove("XDG_RUNTIME_DIR")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: CliEnvelope<DoctorReport> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope.schema_version, "fabric.cli.v1");
    assert_eq!(envelope.command, "doctor");
    assert!(!envelope.data.daemon_running);
    assert!(!envelope.data.checks.is_empty());
    std::fs::remove_dir_all(base).unwrap();
}
