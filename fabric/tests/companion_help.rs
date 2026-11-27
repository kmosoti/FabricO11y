use std::process::Command;

#[test]
fn companion_help_and_invalid_invocations() {
    for binary in [
        env!("CARGO_BIN_EXE_spindle"),
        env!("CARGO_BIN_EXE_fabric-adapter"),
    ] {
        for flag in ["-h", "--help"] {
            let output = Command::new(binary).arg(flag).output().unwrap();
            assert!(
                output.status.success(),
                "{binary} {flag}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
        }
        let output = Command::new(binary)
            .args(["--help", "unexpected"])
            .output()
            .unwrap();
        assert!(!output.status.success());
    }
}
