//! Subprocess isolation prevents fork from inheriting sibling projection locks.
#![cfg(target_os = "linux")]
use fabric::{canonical::CanonicalRow, query::*};
use fabric_query::Projection;
use serde_json::Value;
use std::sync::atomic::AtomicBool;
const RAW: &str = include_str!("fixtures/mixed.jsonl");
fn private_tempdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
fn request(operation: Operation) -> QueryRequest {
    QueryRequest {
        version: 1,
        query_id: "file-limit".into(),
        authority: None,
        scope: vec![],
        snapshot: Snapshot::new(),
        filter: Filter::default(),
        operation,
        budgets: Budgets::default(),
        cursor: None,
    }
}
fn global() -> RunKey {
    RunKey::Global {
        run_id: "11111111-1111-4111-8111-111111111111".into(),
    }
}

/// Isolate the process-wide limit from the parent and other parallel tests.
#[cfg(target_os = "linux")]
#[test]
fn thousand_segments_query_under_low_file_limit() {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, atomic::AtomicUsize};
    const CHILD: &str = "FABRIC_QUERY_LOW_FD_CHILD";
    if std::env::var_os(CHILD).as_deref() != Some(std::ffi::OsStr::new("1")) {
        let before = getrlimit(Resource::Nofile);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "thousand_segments_query_under_low_file_limit",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "low-file-limit child failed: {}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let after = getrlimit(Resource::Nofile);
        assert_eq!(after.current, before.current);
        assert_eq!(after.maximum, before.maximum);
        return;
    }
    let limits = getrlimit(Resource::Nofile);
    setrlimit(
        Resource::Nofile,
        Rlimit {
            current: Some(64),
            maximum: limits.maximum,
        },
    )
    .unwrap();
    let dir = private_tempdir();
    let mut projection = Projection::open(dir.path(), 64 * 1024 * 1024).unwrap();
    let scope: Vec<String> = (0..32).map(|i| format!("node-{i}")).collect();
    let mut raw: Value = serde_json::from_str(RAW.lines().next().unwrap()).unwrap();
    // Thirty-two legitimate origins per apply create 1,024 tiny segments
    // without quadratic per-row manifest publication in the test setup.
    for round in 0..32u64 {
        raw["identity"]["sequence"] = (round + 1).into();
        let raw = serde_json::to_string(&raw).unwrap();
        let rows: Vec<_> = scope
            .iter()
            .enumerate()
            .map(|(i, origin)| CanonicalRow {
                canonical_id: round * 32 + i as u64 + 1,
                source_id: round + 1,
                origin: origin.clone(),
                raw: raw.clone(),
            })
            .collect();
        projection.apply(&rows).unwrap();
    }
    assert_eq!(
        std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|s| s == "parquet"))
            .count(),
        1024
    );
    let peak = Arc::new(AtomicUsize::new(0));
    let running = Arc::new(AtomicBool::new(true));
    let sampler = {
        let peak = peak.clone();
        let running = running.clone();
        std::thread::spawn(move || {
            while running.load(Ordering::Relaxed) {
                let open = std::fs::read_dir("/proc/self/fd").unwrap().count();
                peak.fetch_max(open, Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        })
    };
    let mut r = request(Operation::RunSummary);
    r.scope = scope;
    r.filter.run = Some(global());
    r.budgets.timeout_ms = 30_000;
    // Repeated real queries also check that runtime teardown releases files.
    for _ in 0..2 {
        let response = projection.query(r.clone()).unwrap();
        let summaries: Vec<RunSummary> = response.decode_rows().unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].statistics.count, 1024);
        assert_eq!(
            summaries[0].statistics.sum_ns,
            Some(u128::from(u64::MAX) * 1024)
        );
        assert!(response.coverage.complete);
    }
    running.store(false, Ordering::Relaxed);
    sampler.join().unwrap();
    let peak = peak.load(Ordering::Relaxed);
    eprintln!("1,024 segments; soft NOFILE=64; sampled peak open descriptors={peak}");
    assert!(peak < 48, "unexpected descriptor accumulation: {peak}");
}
