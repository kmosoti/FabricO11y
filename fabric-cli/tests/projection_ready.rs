use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base =
            std::env::var_os("TMPDIR").map_or_else(|| PathBuf::from("/data/tmp"), PathBuf::from);
        let base = fabric_store::validate_non_tmp_path_on_disk(&base).unwrap();
        let path = base.join(format!("fabric-ready-{}-{stamp}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const RECEIPT: &str = r#"{"result":"receipt","receipt":{"id":1,"identity":{"producer":"ready-test","generation":"generation","sequence":1},"duplicate":false}}"#;
const RECORD: &str = r#"{"result":"records","records":[[1,{"version":2,"registry":"fabrico11y/2","identity":{"producer":"ready-test","generation":"generation","sequence":1},"timestamp_ns":1,"resource":"test","data":{"kind":"log","level":"info","message":"projection ready"}}]]}"#;

#[test]
fn verified_custody_is_pending_without_creating_collection_state() {
    let scratch = Scratch::new();
    let data_dir = scratch.0.join("absent-data");
    let socket = scratch.0.join("spindle.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut connection = loop {
            match listener.accept() {
                Ok((connection, _)) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "CLI did not connect");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept: {error}"),
            }
        };
        connection
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut header = [0; 4];
        connection.read_exact(&mut header).unwrap();
        let mut request = vec![0; u32::from_be_bytes(header) as usize];
        connection.read_exact(&mut request).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&request).unwrap(),
            serde_json::json!({"command":"read_for_origin","expected_origin":"origin-test","after":0,"limit":1})
        );
        connection
            .write_all(&(RECORD.len() as u32).to_be_bytes())
            .unwrap();
        connection.write_all(RECORD.as_bytes()).unwrap();
    });
    let receipt = scratch.0.join("receipt.json");
    fs::write(&receipt, RECEIPT).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_fabric"))
        .arg("--data-dir")
        .arg(&data_dir)
        .args(["projection-ready", "--origin", "origin-test", "--receipt"])
        .arg(receipt)
        .arg("--spindle-socket")
        .arg(socket)
        .args(["--wait", "180ms", "--json"])
        .output()
        .unwrap();
    server.join().unwrap();
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["data"]["status"], "pending");
    assert_eq!(result["data"]["record_id"], 1);
    assert_eq!(result["data"]["projection_frontier"]["record_id"], 0);
    assert!(!data_dir.exists());
}

#[test]
fn blocked_standard_input_respects_the_process_deadline() {
    let scratch = Scratch::new();
    let start = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_fabric"))
        .arg("--data-dir")
        .arg(scratch.0.join("absent-data"))
        .args([
            "projection-ready",
            "--origin",
            "origin-test",
            "--receipt",
            "-",
            "--wait",
            "40ms",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let _open_pipe = child.stdin.take().unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["data"]["reason"], "evidence_deadline");
    assert!(!scratch.0.join("absent-data").exists());
}
