use std::process::Command;
#[test]
#[ignore = "bounded subprocess and durable I/O campaigns run in acceptance"]
fn collection_programs() {
    for binary in [
        env!("CARGO_BIN_EXE_collection-resources"),
        env!("CARGO_BIN_EXE_collection-faults"),
    ] {
        let output = Command::new("timeout")
            .args(["60", binary])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{binary}: {}\n{stdout}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(report["passed"], true);
        println!("{binary}: {stdout}");
    }
}
