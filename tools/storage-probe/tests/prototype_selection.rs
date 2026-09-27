//! Integrator probes after candidate inspection; independent oracle remains unchanged.
use fabric_o11y::log::EventLog;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "cli-select-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn command(args: Vec<String>) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_fabric-research"))
        .args(args)
        .output()
        .unwrap()
}
fn ok(args: Vec<String>) -> serde_json::Value {
    let r = command(args);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    serde_json::from_slice(&r.stdout).unwrap()
}
#[test]
fn tail_is_fully_drained_for_every_capacity_batch_relation() {
    let s = Scratch::new();
    let input = s.0.join("input.json");
    ok(vec![
        "generate".into(),
        "19".into(),
        "7".into(),
        input.display().to_string(),
    ]);
    for capacity in [1, 2, 5, 10] {
        for batch in [1, 2, 4, 11] {
            let log = s.0.join(format!("c{capacity}b{batch}.fol2"));
            let args = vec![
                "ingest".into(),
                input.display().to_string(),
                log.display().to_string(),
                capacity.to_string(),
                batch.to_string(),
            ];
            let report = ok(args.clone());
            assert_eq!(report["input_events"], 7);
            assert_eq!(report["appended"], 7);
            assert!(report["peak_buffer_events"].as_u64().unwrap() <= capacity);
            let before = fs::read(&log).unwrap();
            let mut reader = EventLog::open(&log).unwrap();
            let mut ids = vec![];
            reader
                .replay(|e| {
                    ids.push(e.id.0);
                    Ok(())
                })
                .unwrap();
            drop(reader);
            assert_eq!(ids, (1..=7).collect::<Vec<_>>());
            let report = ok(args);
            assert_eq!(report["appended"], 0);
            assert_eq!(report["already_committed"], 7);
            assert_eq!(fs::read(&log).unwrap(), before);
        }
    }
}
#[test]
fn duplicate_query_fields_are_rejected_before_checkpoint_creation() {
    let s = Scratch::new();
    let input = s.0.join("input.json");
    let log = s.0.join("log.fol2");
    let snap = s.0.join("snapshot");
    let trust = s.0.join("trusted.json");
    let query = s.0.join("query.json");
    let checkpoint = s.0.join("checkpoint.json");
    ok(vec![
        "generate".into(),
        "19".into(),
        "2".into(),
        input.display().to_string(),
    ]);
    ok(vec![
        "ingest".into(),
        input.display().to_string(),
        log.display().to_string(),
        "1".into(),
        "1".into(),
    ]);
    ok(vec![
        "publish".into(),
        log.display().to_string(),
        snap.display().to_string(),
        trust.display().to_string(),
        "1".into(),
        "22".into(),
    ]);
    fs::write(&query,br#"{"start_ns":0,"start_ns":-9223372036854775808,"end_ns":9223372036854775807,"tenant":null,"token":null}"#).unwrap();
    let r = command(vec![
        "query".into(),
        snap.display().to_string(),
        trust.display().to_string(),
        query.display().to_string(),
        "all".into(),
        checkpoint.display().to_string(),
    ]);
    assert_eq!(r.status.code(), Some(1));
    assert!(!checkpoint.exists());
}
