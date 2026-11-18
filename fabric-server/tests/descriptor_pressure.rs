//! Real per-process exhaustion stays isolated from other tests and installed services.
use fabric::canonical::{self, ForwardRecord, Request, Response};
use fabric_server::{Config, NodeGrant, Server};
use std::{
    fs::{self, File},
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::Ordering,
    thread,
    time::{Duration, Instant},
};
const CHILD: &str = "FABRIC_TEST_DESCRIPTOR_PRESSURE_ROOT";
const TEST: &str = "receiver_recovers_after_process_descriptor_exhaustion";
const RAW: &str = r#"{"version":2,"registry":"fabrico11y/2","identity":{"producer":"fixture","generation":"generation","sequence":1},"timestamp_ns":1,"resource":"svc","data":{"kind":"log","level":"info","message":"retained"}}"#;
fn key(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, format!("test-only-private-{name}-credential")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}
fn pressure_child(root: &Path) {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    let limit = getrlimit(Resource::Nofile);
    setrlimit(
        Resource::Nofile,
        Rlimit {
            current: Some(limit.current.unwrap_or(96).min(96)),
            maximum: limit.maximum,
        },
    )
    .unwrap();
    let server = Server::open(Config {
        listen: "127.0.0.1:0".into(),
        storage: root.join("canonical"),
        projection: root.join("projection"),
        quota_bytes: 8 << 20,
        projection_quota_bytes: 8 << 20,
        reader_credential_file: key(root, "reader"),
        nodes: vec![NodeGrant {
            origin: "alpha".into(),
            credential_file: key(root, "alpha"),
        }],
        node_quota_bytes: Default::default(),
        engine_endpoint: None,
        max_connections: 4,
    })
    .unwrap();
    let endpoint = server.address();
    let stop = server.stop_handle();
    let worker = thread::spawn(move || server.serve());
    let mut ready = File::create(root.join("ready")).unwrap();
    let mut held = Vec::new();
    loop {
        match File::open("/dev/null") {
            Ok(file) => held.push(file),
            Err(error) => {
                assert_eq!(
                    error.raw_os_error(),
                    Some(rustix::io::Errno::MFILE.raw_os_error())
                );
                break;
            }
        }
    }
    writeln!(ready, "{endpoint}").unwrap();
    ready.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !root.join("request-started").exists() {
        assert!(Instant::now() < deadline, "parent did not queue a request");
        thread::sleep(Duration::from_millis(2));
    }
    // Real TCP work queues while accept cannot allocate another descriptor.
    thread::sleep(Duration::from_millis(250));
    drop(held);
    fs::write(root.join("released"), b"descriptors available").unwrap();
    while !root.join("complete").exists() {
        assert!(Instant::now() < deadline, "parent did not finish recovery");
        thread::sleep(Duration::from_millis(2));
    }
    stop.store(true, Ordering::SeqCst);
    worker.join().unwrap().unwrap();
}
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn receiver_recovers_after_process_descriptor_exhaustion() {
    if let Some(root) = std::env::var_os(CHILD) {
        pressure_child(Path::new(&root));
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(CHILD, dir.path())
            // Only the parent reports a harness result; child panic diagnostics
            // still reach stderr and its exit status is checked below.
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(12);
    let endpoint = loop {
        if let Ok(text) = fs::read_to_string(dir.path().join("ready"))
            && text.ends_with('\n')
        {
            break text.trim().to_owned();
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "pressure child exited"
        );
        assert!(
            Instant::now() < deadline,
            "pressure child never became ready"
        );
        thread::sleep(Duration::from_millis(2));
    };
    let reader = fs::read_to_string(dir.path().join("reader")).unwrap();
    let node = fs::read_to_string(dir.path().join("alpha")).unwrap();
    let call = |credential: &str, request| {
        canonical::exchange(
            &endpoint,
            credential,
            &request,
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap()
    };
    fs::write(dir.path().join("request-started"), b"queued").unwrap();
    assert!(matches!(
        call(&reader, Request::Progress),
        Response::Progress { .. }
    ));
    // A descriptor closed by another thread can let Progress finish before
    // the pressure thread releases its files. Ingest needs recovered capacity.
    while !dir.path().join("released").exists() {
        assert!(Instant::now() < deadline, "descriptors were not released");
        thread::sleep(Duration::from_millis(2));
    }
    assert!(matches!(
        call(
            &node,
            Request::Ingest {
                origin: "alpha".into(),
                records: vec![ForwardRecord {
                    source_id: 1,
                    raw: RAW.into()
                }]
            }
        ),
        Response::Committed { through: 1, .. }
    ));
    let Response::Rows { rows } = call(
        &reader,
        Request::Read {
            origin: "alpha".into(),
            after: 0,
            through: 1,
            limit: 1,
        },
    ) else {
        panic!("canonical replay did not recover")
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].raw, RAW);
    fs::write(dir.path().join("complete"), b"recovered").unwrap();
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success(), "pressure child failed: {status}");
            break;
        }
        assert!(Instant::now() < deadline, "pressure child did not stop");
        thread::sleep(Duration::from_millis(2));
    }
}
