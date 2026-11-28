use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    thread,
};
static SEQ: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let p = PathBuf::from("/data/tmp").join(format!(
            "cli-cursor-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn cli(root: &Scratch) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_fabric"));
    c.env("FABRIC_RUNTIME_DIR", &root.0)
        .env("TMPDIR", "/data/tmp")
        .arg("--data-dir")
        .arg(root.0.join("data"))
        .args(["--tmp-dir", "/data/tmp", "--json", "live-metrics"]);
    c
}
#[test]
fn emits_exact_typed_page_and_rejects_skipped_ids() {
    let root = Scratch::new();
    let listener = UnixListener::bind(root.0.join("daemon.sock")).unwrap();
    let server = thread::spawn(move || {
        for skipped in [false, true] {
            let (stream, _) = listener.accept().unwrap();
            let mut stream = BufReader::new(stream);
            let mut request = String::new();
            stream.read_line(&mut request).unwrap();
            let request: serde_json::Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["op"], "live_metrics_v1");
            let page = serde_json::json!({"kind":"live_metrics_v1","version":1,"origin":"origin","epoch":"a".repeat(64),"observed_through":u64::MAX,"epoch_start_after":0,"oldest_retained":1,"highest_lost":1,"highest_lost_reason":"evicted","gap":true,"gap_reason":"evicted","next_after":u64::MAX,"entries":[{"record_id":if skipped { 2 } else { u64::MAX },"point":{"timestamp_ms":0,"name":"m","value":1.0,"host":"h","slice":"s","unit":"u","pid":null,"labels_json":"{}"}}]});
            stream
                .get_mut()
                .write_all(format!("{page}\n").as_bytes())
                .unwrap();
        }
    });
    let output = cli(&root).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["command"], "live-metrics");
    assert_eq!(body["data"]["entries"][0]["record_id"], u64::MAX);
    assert_eq!(body["data"]["gap_reason"], "evicted");
    let bad = cli(&root).output().unwrap();
    assert!(!bad.status.success());
    assert!(bad.stdout.is_empty());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("invalid daemon live cursor response"));
    server.join().unwrap();
}
#[test]
fn socket_failure_has_no_fallback() {
    let root = Scratch::new();
    let output = cli(&root).output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("daemon socket connect"));
}

#[test]
fn explicit_socket_overrides_runtime_directory() {
    let root = Scratch::new();
    let actual = root.0.join("actual");
    std::fs::create_dir(&actual).unwrap();
    let path = actual.join("daemon.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        stream.read_line(&mut line).unwrap();
        let page = serde_json::json!({"kind":"live_metrics_v1","version":1,"origin":"origin","epoch":"a".repeat(64),"observed_through":0,"epoch_start_after":0,"oldest_retained":null,"highest_lost":0,"highest_lost_reason":null,"gap":false,"gap_reason":null,"next_after":0,"entries":[]});
        stream
            .get_mut()
            .write_all(format!("{page}\n").as_bytes())
            .unwrap();
    });
    let result = cli(&root).arg("--socket").arg(&path).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(body["data"]["epoch"], "a".repeat(64));
    server.join().unwrap();
}
