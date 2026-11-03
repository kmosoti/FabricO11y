use fabric::{
    observation,
    spindle::{self, Reply, Request, transport},
};
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Service(Child);
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn request(socket: &Path, r: &Request) -> Result<Reply, String> {
    transport::request(socket, r, Instant::now() + Duration::from_secs(2))
}
fn start(config: &Path, socket: &Path) -> Service {
    let child = Service(
        Command::new(env!("CARGO_BIN_EXE_spindle"))
            .arg("serve")
            .arg(config)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        if matches!(request(socket, &Request::Status), Ok(Reply::Status { .. })) {
            return child;
        }
        assert!(Instant::now() < end, "readiness deadline");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn envelope(fields: &str, sequence: u64) -> Vec<u8> {
    format!(r#"{{"version":2,"registry":"fabrico11y/2","identity":{{"producer":"raw","generation":"boundary","sequence":{sequence}}},"timestamp_ns":1,"resource":"test","data":{{"kind":"json_log","fields":{fields}}}}}"#).into_bytes()
}
fn lifecycle(raw: Vec<u8>) {
    let expected = observation::decode(&raw).unwrap();
    let temp = tempfile::tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let socket = temp.path().join("spindle.sock");
    let path = temp.path().join("config.json");
    let cfg = spindle::Config {
        socket: socket.clone(),
        storage: temp.path().join("custody"),
        quota_bytes: 2 * 1024 * 1024,
        max_connections: 4,
        client_timeout_ms: 1000,
        files: vec![],
        journal: None,
        host: None,
    };
    fs::write(&path, serde_json::to_vec(&cfg).unwrap()).unwrap();
    // Prepopulate through custody to also exercise records acknowledged by older servers.
    {
        let mut store = spindle::custody::Store::open(&cfg.storage, cfg.quota_bytes).unwrap();
        store.submit(&raw, None).unwrap();
    }
    let service = start(&path, &socket);
    let receipt =
        transport::submit_bytes(&socket, &raw, Instant::now() + Duration::from_secs(2)).unwrap();
    assert!(receipt.duplicate);
    let following = envelope(r#"{"next":true}"#, 2);
    transport::submit_bytes(&socket, &following, Instant::now() + Duration::from_secs(2)).unwrap();
    {
        match request(&socket, &Request::Read { after: 0, limit: 1 }).unwrap() {
            Reply::Records { records } => {
                assert_eq!(records.len(), 1);
                assert_eq!(records[0].1, expected)
            }
            r => panic!("{r:?}"),
        }
        let output = Command::new("timeout")
            .args(["5", env!("CARGO_BIN_EXE_spindle"), "export"])
            .arg(&socket)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let records: Vec<observation::Observation> = output
            .stdout
            .split(|b| *b == b'\n')
            .filter(|b| !b.is_empty())
            .map(|b| serde_json::from_slice(b).unwrap())
            .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0], expected);
        assert_eq!(records[1], observation::decode(&following).unwrap());
    }
    drop(service);
    let _restarted = start(&path, &socket);
    match request(&socket, &Request::Read { after: 0, limit: 1 }).unwrap() {
        Reply::Records { records } => assert_eq!(records[0].1, expected),
        r => panic!("{r:?}"),
    }
    let output = Command::new("timeout")
        .args(["5", env!("CARGO_BIN_EXE_spindle"), "export"])
        .arg(&socket)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        output
            .stdout
            .split(|b| *b == b'\n')
            .filter(|b| !b.is_empty())
            .count(),
        2
    );
}
#[test]
fn accepted_depth_boundary_survives_lifecycle() {
    let nested = format!("{}true{}", "[".repeat(13), "]".repeat(13));
    lifecycle(envelope(&format!(r#"{{"nested":{nested}}}"#), 1));
}
#[test]
fn compact_numbers_survive_lifecycle() {
    let array = format!("[{}]", vec!["1e9"; 64].join(","));
    let mut fields = (0..55)
        .map(|i| format!("\"n{i}\":{array}"))
        .collect::<Vec<_>>();
    for key in ["a", "b"] {
        fields.push(format!("\"{key}\":\"{}\"", "x".repeat(20000)));
    }
    let raw = envelope(&format!("{{{}}}", fields.join(",")), 1);
    assert!(raw.len() < observation::MAX_FRAME);
    assert!(
        serde_json::to_vec(&observation::decode(&raw).unwrap())
            .unwrap()
            .len()
            > spindle::MAX_REPLY
    );
    lifecycle(raw);
}
#[test]
fn combined_boundaries_survive_lifecycle() {
    for depth in [0, 12, 13] {
        let nested = format!("{}1e9{}", "[".repeat(depth), "]".repeat(depth));
        lifecycle(envelope(
            &format!(
                r#"{{"nested":{nested},"escaped":"{}"}}"#,
                "\\n".repeat(28000)
            ),
            1,
        ));
    }
}
#[test]
fn cosine_positive_rescaling_extremes() {
    for scale_a in [1e-300, 1.0, 1e300] {
        for scale_b in [1e-300, 1.0, 1e300] {
            let a = [3.0 * scale_a, 4.0 * scale_a];
            let b = [4.0 * scale_b, -3.0 * scale_b];
            assert!(fabric::analysis::cosine(&a, &b).unwrap().abs() < 1e-14);
            assert!((fabric::analysis::cosine(&a, &a).unwrap() - 1.0).abs() < 1e-14);
            assert_eq!(
                fabric::analysis::cosine(&a, &b),
                fabric::analysis::cosine(&b, &a)
            );
        }
    }
}
