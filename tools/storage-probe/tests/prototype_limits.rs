//! Regression for a valid oversized FOL2 log accepted by the original CLI.
use fabric_o11y::{
    Payload,
    generator::{EventGenerator, WorkloadConfig},
    log::EventLog,
};
use std::{fs, process::Command};

#[test]
fn log_size_is_checked_before_publish_or_existing_log_ingest() {
    let root = std::env::temp_dir().join(format!("prototype-limits-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let path = root.join("oversized.fol");
    let mut event = EventGenerator::new(WorkloadConfig { seed: 9, events: 1 })
        .next()
        .unwrap();
    event.payload = Payload::Log {
        body: "x".repeat(8 * 1024 * 1024),
    };
    let mut log = EventLog::open(&path).unwrap();
    for _ in 0..9 {
        log.append(&event).unwrap();
    }
    drop(log);
    let before = fs::metadata(&path).unwrap().len();
    assert!(before > 64 * 1024 * 1024);
    let output = Command::new(env!("CARGO_BIN_EXE_fabric-research"))
        .current_dir(&root)
        .args([
            "publish",
            "oversized.fol",
            "snapshot",
            "trusted.json",
            "1",
            "9",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "oversized log accepted: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("64 MiB"),
        "size check must precede log parsing: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!root.join("snapshot").exists());
    assert!(!root.join("trusted.json").exists());
    let input = storage_probe::disk::encode_events(&[event]).unwrap();
    fs::write(root.join("input.json"), input).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_fabric-research"))
        .current_dir(&root)
        .args(["ingest", "input.json", "oversized.fol", "1", "1"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("64 MiB"),
        "size check must precede log parsing: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::metadata(&path).unwrap().len(), before);
    fs::remove_dir_all(&root).unwrap();
}
