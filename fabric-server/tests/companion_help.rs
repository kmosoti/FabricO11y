use std::process::Command;

#[test]
fn help_and_invalid_invocation() {
    for flag in ["-h", "--help"] {
        let output = Command::new(env!("CARGO_BIN_EXE_fabric-server"))
            .arg(flag)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
    }
    let output = Command::new(env!("CARGO_BIN_EXE_fabric-server"))
        .args(["--help", "unexpected"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}
