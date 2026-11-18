//! Real per-process exhaustion stays isolated from other tests and installed services.
use fabric::{
    canonical::{self, ForwardRecord, ProjectionReadyRequest, ProjectionStatus, Request, Response},
    observation,
    query::{API_VERSION, Operation, QueryRequest},
};
use fabric_server::{Config, NodeGrant, Server};
use std::{
    fs::{self, File},
    io::Write,
    net::TcpStream,
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

const LEASE_CHILD: &str = "FABRIC_TEST_LEASE_PRESSURE_ROOT";
const MEMORY_BYTES: &str = "FABRIC_TEST_CGROUP_PRESSURE_BYTES";
const LEASE_TEST: &str = "receiver_preserves_owned_ingest_and_replays_after_pressure_restart";
const PRESSURE_ERROR: &str = "analytical work deferred by resource pressure";

fn start_lease_child(root: &Path) -> (ChildGuard, String) {
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", LEASE_TEST, "--nocapture"])
            .env(LEASE_CHILD, root)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let endpoint = loop {
        if let Ok(text) = fs::read_to_string(root.join("lease-ready"))
            && text.ends_with('\n')
        {
            break text.trim().to_owned();
        }
        assert!(child.0.try_wait().unwrap().is_none(), "receiver exited");
        assert!(Instant::now() < deadline, "receiver startup expired");
        thread::sleep(Duration::from_millis(5));
    };
    (child, endpoint)
}

fn lease_child(root: &Path) {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    let old = getrlimit(Resource::Nofile);
    let limit = old.current.unwrap_or(48).min(48);
    setrlimit(
        Resource::Nofile,
        Rlimit {
            current: Some(limit),
            maximum: old.maximum,
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
        max_connections: 32,
    })
    .unwrap();
    let endpoint = server.address();
    let stop = server.stop_handle();
    let worker = thread::spawn(move || server.serve());
    fs::write(root.join("lease-ready"), format!("{endpoint}\n")).unwrap();

    let deadline = Instant::now() + Duration::from_secs(25);
    let mut memory = Vec::<u8>::new();
    let mut descriptors = Vec::new();
    while !root.join("lease-complete").exists() {
        assert!(Instant::now() < deadline, "pressure parent expired");
        if root.join("apply-pressure").exists() {
            if memory.is_empty() && descriptors.is_empty() {
                if let Ok(bytes) = std::env::var(MEMORY_BYTES) {
                    memory.resize(bytes.parse().unwrap(), 0);
                    for page in memory.chunks_mut(4096) {
                        page[0] = 1;
                    }
                    std::hint::black_box(&memory);
                    assert!(fabric::resources::process_memory().pressured);
                } else {
                    // Reach the spendable boundary and retain room for custody I/O.
                    let target = limit * 3 / 4 + 2;
                    while (fs::read_dir("/proc/self/fd").unwrap().count() as u64) < target {
                        descriptors.push(File::open("/dev/null").unwrap());
                    }
                    assert!(!descriptors.is_empty());
                }
            }
        } else if !memory.is_empty() || !descriptors.is_empty() {
            drop(std::mem::take(&mut memory));
            descriptors.clear();
        }
        assert!(!worker.is_finished(), "server stopped during pressure");
        thread::sleep(Duration::from_millis(5));
    }
    stop.store(true, Ordering::Release);
    worker.join().unwrap().unwrap();
}

fn exchange(endpoint: &str, credential: &str, request: &Request) -> Response {
    canonical::exchange(
        endpoint,
        credential,
        request,
        Instant::now() + Duration::from_secs(3),
    )
    .expect("receiver exchange")
}

fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(6);
    while !condition() {
        assert!(Instant::now() < deadline, "receiver condition expired");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn receiver_preserves_owned_ingest_and_replays_after_pressure_restart() {
    if let Some(root) = std::env::var_os(LEASE_CHILD) {
        lease_child(Path::new(&root));
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (mut child, endpoint) = start_lease_child(root);
    let reader = fs::read_to_string(root.join("reader")).unwrap();
    let node = fs::read_to_string(root.join("alpha")).unwrap();
    let plan = Request::Plan {
        request: QueryRequest {
            version: API_VERSION,
            query_id: "lease-plan".into(),
            authority: None,
            scope: vec!["alpha".into()],
            snapshot: Default::default(),
            filter: Default::default(),
            operation: Operation::Evidence,
            budgets: Default::default(),
            cursor: None,
        },
    };
    assert!(matches!(
        exchange(&endpoint, &reader, &plan),
        Response::Plan { .. }
    ));

    let apply_pressure = || {
        fs::write(root.join("apply-pressure"), b"pressure").unwrap();
        wait_for(
            || matches!(exchange(&endpoint, &reader, &plan), Response::Error { error } if error == PRESSURE_ERROR),
        );
    };
    let cgroup = std::env::var_os(MEMORY_BYTES).is_some();
    if cgroup {
        // Touching the memory can use more than a frame deadline on a CPU-limited slice.
        apply_pressure();
    }

    let ingest = Request::Ingest {
        origin: "alpha".into(),
        records: vec![ForwardRecord {
            source_id: 1,
            raw: RAW.into(),
        }],
    };
    let bytes =
        serde_json::to_vec(&serde_json::json!({"credential": node, "request": ingest})).unwrap();
    let child_fds = format!("/proc/{}/fd", child.0.id());
    let sockets = || {
        fs::read_dir(&child_fds)
            .unwrap()
            .flatten()
            .filter(|entry| {
                fs::read_link(entry.path()).is_ok_and(|target| {
                    target
                        .as_os_str()
                        .as_encoded_bytes()
                        .starts_with(b"socket:[")
                })
            })
            .count()
    };
    wait_for(|| sockets() == 1);
    let mut stream = TcpStream::connect(&endpoint).unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&bytes[..1]).unwrap();
    // Confirm the server took ownership of the partial frame.
    wait_for(|| sockets() == 2);
    if !cgroup {
        apply_pressure();
    }
    stream.write_all(&bytes[1..]).unwrap();
    let response = serde_json::from_slice(
        &canonical::read_frame(&mut stream, Instant::now() + Duration::from_secs(3)).unwrap(),
    )
    .unwrap();
    let Response::Committed {
        authority,
        through: 1,
        ..
    } = response
    else {
        panic!("owned authenticated frame did not commit during pressure: {response:?}");
    };
    let read = Request::Read {
        origin: "alpha".into(),
        after: 0,
        through: 1,
        limit: 1,
    };
    assert!(
        matches!(exchange(&endpoint, &reader, &read), Response::Rows { rows } if rows.len() == 1 && rows[0].raw == RAW)
    );
    assert!(
        matches!(exchange(&endpoint, "unknown-test-credential", &read), Response::Error { error } if error == "unauthorized")
    );
    let readiness = Request::ProjectionReady {
        request: ProjectionReadyRequest {
            authority: authority.clone(),
            source_id: 1,
            origin: "alpha".into(),
            identity: observation::decode_versioned(RAW.as_bytes())
                .unwrap()
                .identity()
                .clone(),
            timeout_ms: 200,
            wait: false,
        },
    };
    thread::sleep(Duration::from_millis(1200));
    assert!(
        matches!(exchange(&endpoint, &reader, &readiness), Response::ProjectionReady { readiness }
        if readiness.status == ProjectionStatus::Unavailable
        && readiness.custody_frontier == Some(1) && readiness.projection_frontier == Some(0)),
        "optional projection moved while pressure was active"
    );
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    fs::remove_file(root.join("lease-ready")).unwrap();
    fs::remove_file(root.join("apply-pressure")).unwrap();

    let (mut child, endpoint) = start_lease_child(root);
    assert!(
        matches!(exchange(&endpoint, &node, &ingest), Response::Committed { authority: found, through: 1, .. } if found == authority)
    );
    assert!(
        matches!(exchange(&endpoint, &reader, &read), Response::Rows { rows } if rows.len() == 1 && rows[0].raw == RAW)
    );
    wait_for(|| {
        matches!(exchange(&endpoint, &reader, &readiness), Response::ProjectionReady { readiness }
        if readiness.status == ProjectionStatus::Visible && readiness.projection_frontier == Some(1))
    });
    assert!(matches!(
        exchange(&endpoint, &reader, &plan),
        Response::Plan { .. }
    ));
    fs::write(root.join("lease-complete"), b"done").unwrap();
    wait_for(|| match child.0.try_wait().unwrap() {
        Some(status) => {
            assert!(status.success());
            true
        }
        None => false,
    });
}
