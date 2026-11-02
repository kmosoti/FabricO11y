use std::process::Command;
#[test]
#[ignore = "process fault campaign and durable samples run in acceptance"]
fn distributed_programs() {
    let out = Command::new("timeout")
        .args(["120", env!("CARGO_BIN_EXE_distributed-faults")])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["passed"],
        true
    );
    for codec in ["json", "rows", "columns"] {
        for mode in ["compatible", "native"] {
            let out = Command::new("timeout")
                .args([
                    "30",
                    env!("CARGO_BIN_EXE_representation-evaluator"),
                    "sample",
                    "17",
                    "mixed",
                    "8",
                    codec,
                    mode,
                ])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let row: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            for role in ["spindle", "server"] {
                assert_eq!(row["stages"][role]["recovered_records"], 8);
            }
            assert!(
                row["fabric_seal_ns"].as_u64().unwrap() >= row["spindle_seal_ns"].as_u64().unwrap()
            );
        }
    }
}
