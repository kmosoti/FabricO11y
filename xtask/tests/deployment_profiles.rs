//! Acceptance against isolated, separately built product executables.
use fabric::{
    canonical::{self, Request, Response},
    observation::{self, Data, Identity, Observation, Outcome},
    query::{Budgets, Filter, Operation, QueryRequest},
    spindle::{self, ForwardConfig, Health, Reply, custody::Receipt},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use xtask::collection_lab::{self, OwnedChild};

const TELEMETRY_SOCKET: &str = "FABRIC_SELF_TELEMETRY_SOCKET";
const TELEMETRY_ORIGINS: &str = "FABRIC_SELF_TELEMETRY_ORIGINS";
const READER: &str = "fixture-reader-credential";
const NODE: [(&str, &str); 3] = [
    ("alpha", "fixture-alpha-credential"),
    ("beta", "fixture-beta-credential"),
    ("telemetry", "fixture-telemetry-credential"),
];

fn until<T>(label: &str, budget: Duration, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + budget;
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "{label} timed out");
        thread::sleep(Duration::from_millis(100));
    }
}

fn start(binary: &Path, file: &Path, config: &impl serde::Serialize) -> Command {
    fs::write(file, serde_json::to_vec(config).unwrap()).unwrap();
    let mut command = Command::new(binary);
    command
        .arg("serve")
        .arg(file)
        .env_remove(TELEMETRY_SOCKET)
        .env_remove(TELEMETRY_ORIGINS)
        .env_remove("NOTIFY_SOCKET")
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    command
}

fn status(config: &spindle::Config) -> Option<Health> {
    match spindle::transport::request(
        &config.socket,
        &spindle::Request::Status,
        Instant::now() + Duration::from_secs(1),
    ) {
        Ok(Reply::Status { status }) => Some(status),
        _ => None,
    }
}

struct Node {
    config: spindle::Config,
    origin: String,
    credential_file: PathBuf,
    child: OwnedChild,
}

impl Node {
    fn open(bins: &Path, root: &Path, name: &str, credential: &str) -> Self {
        let path = root.join(name);
        fs::create_dir(&path).unwrap();
        let mut config = collection_lab::config(&path);
        // The 15% CPU slice is shared by all four service processes.
        config.client_timeout_ms = 2_000;
        let credential_file = path.join("credential");
        private_file(&credential_file, credential);
        let mut node = Self {
            child: spawn_spindle(bins, &config, None),
            config,
            origin: String::new(),
            credential_file,
        };
        node.origin = node.health().origin;
        node
    }

    fn health(&self) -> Health {
        until("Spindle status", Duration::from_secs(10), || {
            status(&self.config)
        })
    }

    fn forward(&mut self, bins: &Path, endpoint: &str, authority: &str, telemetry: Option<&Path>) {
        assert!(matches!(
            spindle::transport::request(
                &self.config.socket,
                &spindle::Request::Stop,
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap(),
            Reply::Stopped
        ));
        until("Spindle stop", Duration::from_secs(5), || {
            self.child.0.try_wait().unwrap()
        });
        self.config.forwarding = Some(ForwardConfig {
            endpoint: endpoint.into(),
            authority: authority.into(),
            credential_file: self.credential_file.clone(),
            reclaim_confirmed: false,
        });
        self.child = spawn_spindle(bins, &self.config, telemetry);
        assert_eq!(self.health().origin, self.origin);
    }

    fn submit(&self, record: &str) -> Receipt {
        spindle::transport::submit_bytes(
            &self.config.socket,
            record.as_bytes(),
            Instant::now() + Duration::from_secs(3),
        )
        .unwrap_or_else(|error| panic!("submit to {}: {error}", self.config.socket.display()))
    }
}

fn spawn_spindle(bins: &Path, config: &spindle::Config, telemetry: Option<&Path>) -> OwnedChild {
    let path = config.socket.with_extension("json");
    let mut command = start(&bins.join("spindle"), &path, config);
    if let Some(socket) = telemetry {
        command.env(TELEMETRY_SOCKET, socket);
    }
    OwnedChild(command.spawn().unwrap())
}

fn private_file(path: &Path, text: &str) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(text.as_bytes()).unwrap();
}

fn observations(resource: &str, sequence: u64) -> String {
    serde_json::to_string(&Observation {
        version: observation::VERSION,
        registry: observation::REGISTRY.into(),
        identity: Identity {
            producer: "deployment-fixture".into(),
            generation: "shared-identity".into(),
            sequence,
        },
        timestamp_ns: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .try_into()
            .unwrap(),
        resource: resource.into(),
        data: Data::ServiceRequest {
            service: resource.into(),
            operation: "deployment-observation".into(),
            duration_ns: 700,
            status: Outcome::Ok,
            trace_id: None,
        },
    })
    .unwrap()
}

fn local(bins: &Path, root: &Path, data: &Path, args: &[&str]) -> Output {
    Command::new("timeout")
        .arg("30")
        .arg(bins.join("fabric"))
        .arg("--tmp-dir")
        .arg(root)
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .output()
        .unwrap()
}

fn parse_output(output: Output, status: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(status),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn project(bins: &Path, root: &Path, data: &Path, socket: &Path) {
    let output = local(
        bins,
        root,
        data,
        &[
            "daemon",
            "--once",
            "--spindle-socket",
            socket.to_str().unwrap(),
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn receipt_state(
    bins: &Path,
    root: &Path,
    data: &Path,
    node: &Node,
    receipt: &Receipt,
    expected: (i32, &str),
) {
    let input = data.with_extension("receipt.json");
    fs::write(&input, serde_json::to_vec(receipt).unwrap()).unwrap();
    let output = local(
        bins,
        root,
        data,
        &[
            "projection-ready",
            "--json",
            "--origin",
            &node.origin,
            "--receipt",
            input.to_str().unwrap(),
            "--spindle-socket",
            node.config.socket.to_str().unwrap(),
        ],
    );
    let report = parse_output(output, expected.0);
    assert_eq!(report["data"]["status"], expected.1, "{report}");
    assert_eq!(report["data"]["record_id"], receipt.id, "{report}");
    assert_eq!(report["data"]["origin"], node.origin, "{report}");
}

fn canonical_receipt_state(
    bins: &Path,
    root: &Path,
    endpoint: &str,
    authority: &str,
    origin: &str,
    receipt: &Receipt,
    expected: (i32, &str),
) {
    let input = root.join("canonical-receipt.json");
    fs::write(&input, serde_json::to_vec(receipt).unwrap()).unwrap();
    let output = local(
        bins,
        root,
        &root.join("unused-local-state"),
        &[
            "projection-ready",
            "--target",
            "canonical",
            "--authority",
            authority,
            "--server-endpoint",
            endpoint,
            "--reader-credential-file",
            root.join("reader").to_str().unwrap(),
            "--origin",
            origin,
            "--receipt",
            input.to_str().unwrap(),
            "--wait",
            "10s",
            "--json",
        ],
    );
    let report = parse_output(output, expected.0);
    assert_eq!(report["data"]["status"], expected.1, "{report}");
    assert_eq!(report["data"]["origin"], origin, "{report}");
    if expected.0 == 0 {
        assert_eq!(report["data"]["authority"], authority);
        assert!(report["data"]["custody_frontier"].as_u64().unwrap() >= receipt.id);
        assert!(
            report["data"]["projection_frontier"]["record_id"]
                .as_u64()
                .unwrap()
                >= receipt.id
        );
    }
}

fn receiver(
    bins: &Path,
    root: &Path,
    endpoint: &str,
    nodes: [&Node; 3],
    reader: &Path,
) -> (OwnedChild, String) {
    let lane = nodes[2];
    let config = json!({
        "listen": endpoint,
        "storage": root.join("canonical"),
        "projection": root.join("projection"),
        "quota_bytes": 32 * 1024 * 1024,
        "projection_quota_bytes": 32 * 1024 * 1024,
        "reader_credential_file": reader,
        "nodes": nodes.map(|node| json!({"origin": node.origin, "credential_file": node.credential_file})),
        "node_quota_bytes": { (&lane.origin): 2 * 1024 * 1024 },
    });
    let child = OwnedChild(
        start(
            &bins.join("fabric-server"),
            &root.join("receiver.json"),
            &config,
        )
        .env(TELEMETRY_SOCKET, &lane.config.socket)
        .env(TELEMETRY_ORIGINS, &lane.origin)
        .spawn()
        .unwrap(),
    );
    let authority =
        until(
            "canonical receiver",
            Duration::from_secs(15),
            || match canonical::exchange(
                endpoint,
                READER,
                &Request::Progress,
                Instant::now() + Duration::from_millis(750),
            ) {
                Ok(Response::Progress { authority, .. }) => Some(authority),
                _ => None,
            },
        );
    (child, authority)
}

fn canonical_rows(endpoint: &str, authority: &str, snapshot: BTreeMap<String, u64>) -> Vec<Value> {
    let request = QueryRequest {
        version: 1,
        query_id: "deployment-acceptance".into(),
        authority: Some(authority.to_owned()),
        scope: snapshot.keys().cloned().collect(),
        snapshot: snapshot.clone(),
        filter: Filter::default(),
        operation: Operation::Evidence,
        budgets: Budgets {
            rows: 256,
            ..Budgets::default()
        },
        cursor: None,
    };
    let response = canonical::exchange(
        endpoint,
        READER,
        &Request::Query { request },
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap();
    let Response::Query { response } = response else {
        panic!("unexpected projection reply: {response:?}");
    };
    assert!(response.coverage.complete, "{:?}", response.coverage);
    assert_eq!(response.coverage.snapshot, snapshot);
    for (origin, through) in &snapshot {
        assert!(response.coverage.projection.frontiers[origin] >= *through);
    }
    response.rows
}

fn read_lane(lane: &Node, through: u64, records: &mut Vec<(u64, Vec<u8>)>) -> Result<(), String> {
    let mut after = records.last().map_or(0, |(id, _)| *id);
    while after < through {
        let mut reply = spindle::transport::read_raw_for_origin(
            &lane.config.socket,
            &lane.origin,
            after,
            Instant::now() + Duration::from_secs(3),
        )?;
        let (id, raw) = reply
            .pop()
            .expect("missing record inside durable lane prefix");
        assert_eq!(id, after + 1);
        after = id;
        records.push((id, raw));
    }
    Ok(())
}

fn signal_coverage(records: &[(u64, Vec<u8>)]) -> BTreeMap<String, BTreeSet<&'static str>> {
    let mut signals = BTreeMap::<String, BTreeSet<_>>::new();
    for (_, raw) in records {
        let observation = observation::decode_versioned(raw).unwrap();
        let kind = match observation.data() {
            Data::Metric { .. } => "metric",
            Data::Event { .. } => "event",
            Data::Log { .. } | Data::JsonLog { .. } => "log",
            Data::ServiceRequest { .. } => "trace",
            other => panic!("unanticipated self-observation {other:?}"),
        };
        signals
            .entry(observation.identity().producer.clone())
            .or_default()
            .insert(kind);
    }
    signals
}

fn open_batch_adapter(
    bins: &Path,
    socket: &Path,
    outbox: &Path,
) -> (OwnedChild, mpsc::Receiver<Value>) {
    let mut child = OwnedChild(
        Command::new(bins.join("fabric-adapter"))
            .arg(socket)
            .args(["batch-fixture", "--batch-outbox"])
            .arg(outbox)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = BufReader::new(child.0.stdout.take().unwrap());
    let (replies, responses) = mpsc::sync_channel(1);
    thread::spawn(move || {
        for line in stdout.lines() {
            let json = serde_json::from_str(&line.unwrap()).unwrap();
            if replies.send(json).is_err() {
                break;
            }
        }
    });
    (child, responses)
}

fn batch_record(sequence: u64) -> Value {
    let run_id = format!("00000000-0000-4000-8000-{sequence:012x}");
    json!({
        "command": "record",
        "report_id": format!("00000000-0000-4001-8000-{sequence:012x}"),
        "context": { "run_id": run_id },
        "data": {
            "kind": "agent_run", "run_id": run_id, "agent": "batch-fixture",
            "duration_ns": sequence, "status": "ok",
        },
        "timestamp_ns": observation::now_ns(),
    })
}

fn batch_send(adapter: &mut OwnedChild, request: &Value) {
    writeln!(adapter.0.stdin.as_mut().unwrap(), "{request}").unwrap();
}

fn batch_reply(responses: &mpsc::Receiver<Value>) -> Value {
    responses
        .recv_timeout(Duration::from_secs(5))
        .expect("batch adapter response")
}

fn read_spindle_frame(socket: &mut UnixStream) -> Vec<u8> {
    let mut length = [0; 4];
    socket.read_exact(&mut length).unwrap();
    let length = u32::from_be_bytes(length) as usize;
    assert!(length <= 256 * 1024);
    let mut data = vec![0; length];
    socket.read_exact(&mut data).unwrap();
    data
}

fn write_spindle_frame(socket: &mut UnixStream, bytes: &[u8]) {
    socket
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    socket.write_all(bytes).unwrap();
}

#[test]
#[ignore = "requires exact release executables and fabrico11y.slice"]
fn application_adapter_replays_after_spindle_restart() {
    assert!(
        fs::read_to_string("/proc/self/cgroup")
            .unwrap()
            .contains("/fabrico11y.slice/")
    );
    let bins =
        PathBuf::from(std::env::var_os("FABRIC_TEST_BIN_DIR").expect("release binary directory"));
    let sandbox = xtask::evidence::non_tmp_tempdir().unwrap();
    let root = sandbox.path();
    let mut source = Node::open(&bins, root, NODE[0].0, NODE[0].1);
    let proxy_path = root.join("application-proxy.sock");
    let proxy = UnixListener::bind(&proxy_path).unwrap();
    proxy.set_nonblocking(true).unwrap();
    let real_socket = source.config.socket.clone();
    let (sent, received) = mpsc::sync_channel(1);
    let fault = thread::spawn(move || {
        loop {
            let mut adapter = until("application proxy", Duration::from_secs(10), || match proxy
                .accept()
            {
                Ok((socket, _)) => Some(socket),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => None,
                Err(e) => panic!("proxy accept: {e}"),
            });
            let mut spindle = UnixStream::connect(&real_socket).unwrap();
            for socket in [&adapter, &spindle] {
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
            }
            let request = read_spindle_frame(&mut adapter);
            write_spindle_frame(&mut spindle, &request);
            let response = read_spindle_frame(&mut spindle);
            if let Reply::Receipt { receipt } = serde_json::from_slice(&response).unwrap() {
                sent.send(receipt).unwrap();
                break; // receipt was committed, but no response reaches the adapter
            }
            write_spindle_frame(&mut adapter, &response);
        }
    });
    let original = "{\"label\":\"caf\\u00e9\"}\n";
    let report = json!({
        "command":"record_application", "report_id":"00000000-0000-4001-8000-000000000001",
        "context":{"run_id":"00000000-0000-4000-8000-000000000001"},
        "scope":{"origin":source.origin, "producer":"batch-fixture"},
        "parent_report_id":null, "schema":"example.result.v1",
        "producer_completion":{"unix_ms":42,"clock":{"clock_id":"producer","source":"fixture","uncertainty_ms":null}},
        "artifact_json":original, "timestamp_ns":123
    });
    let outbox = root.join("application-outbox");
    let (mut adapter, replies) = open_batch_adapter(&bins, &proxy_path, &outbox);
    batch_send(&mut adapter, &report);
    let first = received.recv_timeout(Duration::from_secs(15)).unwrap();
    let unknown = batch_reply(&replies);
    assert_eq!(unknown["delivery_state"], "outcome_unknown", "{unknown}");
    assert_eq!(unknown["durably_staged"], true);
    batch_send(
        &mut adapter,
        &json!({"command":"report_status", "report_id":report["report_id"]}),
    );
    let pending = batch_reply(&replies);
    assert_eq!(
        pending["report_status"]["delivery_state"],
        "outcome_unknown"
    );
    let timing = pending["report_status"]["application_timing"].clone();
    assert_eq!(timing["producer_completion"]["unix_ms"], 42);
    assert_eq!(
        timing["outbox_staged_at"]["clock"]["source"],
        "batch_outbox.stage_transaction"
    );
    assert_eq!(
        timing["outbox_staged_at"]["clock"]["clock_id"],
        "system_time.unix_ms"
    );
    adapter.kill_wait();
    fault.join().unwrap();
    assert_eq!(first.id, 1);
    assert!(matches!(
        spindle::transport::request(
            &source.config.socket,
            &spindle::Request::Stop,
            Instant::now() + Duration::from_secs(3)
        )
        .unwrap(),
        Reply::Stopped
    ));
    until("Spindle stop", Duration::from_secs(5), || {
        source.child.0.try_wait().unwrap()
    });
    source.child = spawn_spindle(&bins, &source.config, None);
    assert_eq!(source.health().origin, source.origin);
    let (mut adapter, replies) = open_batch_adapter(&bins, &source.config.socket, &outbox);
    batch_send(&mut adapter, &json!({"command":"status"}));
    assert_eq!(batch_reply(&replies)["status"]["confirmed"], 1);
    batch_send(
        &mut adapter,
        &json!({"command":"report_status", "report_id":report["report_id"]}),
    );
    let status = batch_reply(&replies);
    assert_eq!(
        status["report_status"]["delivery_state"],
        "custody_confirmed"
    );
    assert_eq!(status["report_status"]["application_timing"], timing);
    batch_send(&mut adapter, &report);
    let confirmed = batch_reply(&replies);
    assert_eq!(
        confirmed["delivery_state"], "custody_confirmed",
        "{confirmed}"
    );
    assert_eq!(confirmed["application_timing"], timing);
    let replay: Receipt = serde_json::from_value(confirmed["receipt"].clone()).unwrap();
    assert_eq!(replay.id, first.id);
    assert_eq!(replay.identity, first.identity);
    assert_eq!(
        confirmed["original_artifact_digest"]["hex"],
        "426d8f9948cd8aa9aede8235c65e8f5fd79eb048055e962afbea5c2b5ca1b64d"
    );
    let mut changed = report.clone();
    changed["artifact_json"] = "{\"label\":\"café\"}".into();
    batch_send(&mut adapter, &changed);
    assert_eq!(batch_reply(&replies)["code"], "conflict");
    let mut rows = Vec::new();
    read_lane(&source, 1, &mut rows).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        !rows[0]
            .1
            .windows(b"caf\\u00e9".len())
            .any(|w| w == b"caf\\u00e9")
    );
    let record = observation::decode_versioned(&rows[0].1).unwrap();
    let Data::JsonLog { fields } = record.data() else {
        panic!("expected application result");
    };
    let encoded = serde_json::to_vec(&fields["fabrico11y.application_result.v1"]).unwrap();
    let result = fabric_core::ApplicationResult::decode_bounded(&encoded).unwrap();
    result
        .verify_original_artifact(original.as_bytes())
        .unwrap();
    assert_eq!(
        result.application_payload_digest().0.hex,
        "2d7fba14f0a7cffed454bc268d791c0f32d11f62946998a323087ccf5df29075"
    );
    receipt_state(
        &bins,
        root,
        &root.join("application-local"),
        &source,
        &replay,
        (3, "pending"),
    );
    adapter.kill_wait();
}

#[test]
#[ignore = "requires exact release executables and fabrico11y.slice"]
fn batch_adapter_replays_a_committed_report_and_projects_130_runs() {
    assert!(
        fs::read_to_string("/proc/self/cgroup")
            .unwrap()
            .contains("/fabrico11y.slice/"),
        "run this test in fabrico11y.slice"
    );
    let bins =
        PathBuf::from(std::env::var_os("FABRIC_TEST_BIN_DIR").expect("release binary directory"));
    assert!(bins.is_absolute());
    let sandbox = xtask::evidence::non_tmp_tempdir().unwrap();
    let root = sandbox.path();
    let mut source = Node::open(&bins, root, NODE[0].0, NODE[0].1);
    let spare = Node::open(&bins, root, NODE[1].0, NODE[1].1);
    let telemetry = Node::open(&bins, root, NODE[2].0, NODE[2].1);
    let endpoint = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let reader = root.join("reader");
    private_file(&reader, READER);
    let (mut server, authority) = receiver(
        &bins,
        root,
        &endpoint,
        [&source, &spare, &telemetry],
        &reader,
    );
    source.forward(&bins, &endpoint, &authority, None);
    server.kill_wait();

    let proxy_path = root.join("receipt-proxy.sock");
    let proxy = UnixListener::bind(&proxy_path).unwrap();
    proxy.set_nonblocking(true).unwrap();
    let real_socket = source.config.socket.clone();
    let (observed, committed) = mpsc::sync_channel(1);
    let fault = thread::spawn(move || {
        loop {
            let mut adapter =
                until(
                    "batch proxy connection",
                    Duration::from_secs(10),
                    || match proxy.accept() {
                        Ok((socket, _)) => Some(socket),
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => None,
                        Err(e) => panic!("proxy accept: {e}"),
                    },
                );
            let mut spindle = UnixStream::connect(&real_socket).unwrap();
            for socket in [&adapter, &spindle] {
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
            }
            let request = read_spindle_frame(&mut adapter);
            write_spindle_frame(&mut spindle, &request);
            let response = read_spindle_frame(&mut spindle);
            if let Reply::Receipt { receipt } = serde_json::from_slice(&response).unwrap() {
                assert!(!receipt.duplicate);
                observed.send(receipt).unwrap();
                break;
            }
            write_spindle_frame(&mut adapter, &response);
        }
    });
    let outbox = root.join("outbox");
    let (mut adapter, lost_stdout) = open_batch_adapter(&bins, &proxy_path, &outbox);
    let first_request = batch_record(1);
    batch_send(&mut adapter, &first_request);
    let first_receipt = committed.recv_timeout(Duration::from_secs(15)).unwrap();
    let unknown = batch_reply(&lost_stdout);
    assert_eq!(unknown["delivery_state"], "outcome_unknown", "{unknown}");
    adapter.kill_wait();
    fault.join().unwrap();
    assert_eq!(first_receipt.id, 1);
    assert_eq!(source.health().canonical_through, 0);

    // The first adapter owns one lease; fill the remaining slots without records.
    for index in 0..127 {
        let reply = spindle::transport::request(
            &source.config.socket,
            &spindle::Request::LeaseForOrigin {
                producer: format!("capacity-{index}"),
                expected_origin: source.origin.clone(),
            },
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        assert!(matches!(reply, Reply::Lease { .. }), "{reply:?}");
    }
    let health = source.health();
    let capacity = health.generation_capacity.unwrap();
    assert_eq!((capacity.used, capacity.limit), (128, 128));
    let wrong_origin = spindle::transport::request(
        &source.config.socket,
        &spindle::Request::LeaseForOrigin {
            producer: "different-tenant".into(),
            expected_origin: "different-origin".into(),
        },
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap();
    assert!(
        matches!(wrong_origin, Reply::Error { ref error } if error == spindle::ORIGIN_MISMATCH)
    );
    assert_eq!(health.highest_record_id, 1);
    let (mut refused_adapter, refused_responses) =
        open_batch_adapter(&bins, &source.config.socket, &root.join("refused-outbox"));
    batch_send(&mut refused_adapter, &batch_record(999));
    let refused = batch_reply(&refused_responses);
    assert_eq!(refused["code"], "generation_capacity", "{refused}");
    assert_eq!(refused["delivery_state"], "refused_before_acceptance");
    batch_send(&mut refused_adapter, &json!({"command":"status"}));
    assert!(batch_reply(&refused_responses)["status"]["pending_report_id"].is_null());
    assert_eq!(source.health().highest_record_id, 1);
    refused_adapter.kill_wait();

    let (mut adapter, responses) = open_batch_adapter(&bins, &source.config.socket, &outbox);
    batch_send(&mut adapter, &json!({ "command": "status" }));
    let status = batch_reply(&responses);
    assert_eq!(status["status"]["confirmed"], 1, "{status}");
    batch_send(&mut adapter, &first_request);
    let repeated = batch_reply(&responses);
    assert_eq!(repeated["delivery_state"], "custody_confirmed");
    assert_eq!(repeated["projection"], "unchecked");
    let replayed: Receipt = serde_json::from_value(repeated["receipt"].clone()).unwrap();
    assert_eq!(replayed.id, first_receipt.id);
    assert_eq!(replayed.identity, first_receipt.identity);
    receipt_state(
        &bins,
        root,
        &root.join("batch-local"),
        &source,
        &replayed,
        (3, "pending"),
    );
    let mut conflict = first_request;
    conflict["data"]["duration_ns"] = json!(1_000);
    batch_send(&mut adapter, &conflict);
    let conflicting = batch_reply(&responses);
    assert!(conflicting.get("error").is_some(), "{conflicting}");

    let mut runs = BTreeSet::from([batch_record(1)["context"]["run_id"]
        .as_str()
        .unwrap()
        .to_owned()]);
    for sequence in 2..=130 {
        let report = batch_record(sequence);
        runs.insert(report["context"]["run_id"].as_str().unwrap().to_owned());
        batch_send(&mut adapter, &report);
        let reply = batch_reply(&responses);
        let receipt: Receipt = serde_json::from_value(reply["receipt"].clone())
            .unwrap_or_else(|error| panic!("report {sequence}: {reply}: {error}"));
        assert_eq!(receipt.id, sequence);
        assert_eq!(
            receipt.identity.generation,
            first_receipt.identity.generation
        );
        assert_eq!(receipt.identity.sequence, sequence);
    }
    adapter.0.stdin.take();
    let drained = batch_reply(&responses);
    assert_eq!(drained["eof_receipt"]["id"], 130, "{drained}");
    let exit = until("batch adapter exit", Duration::from_secs(5), || {
        adapter.0.try_wait().unwrap()
    });
    assert!(exit.success(), "{exit}");
    assert_eq!(source.health().canonical_through, 0);
    let mut stored = Vec::new();
    read_lane(&source, 130, &mut stored).unwrap();

    let (_server, resumed_authority) = receiver(
        &bins,
        root,
        &endpoint,
        [&source, &spare, &telemetry],
        &reader,
    );
    assert_eq!(authority, resumed_authority);
    until(
        "batch canonical acknowledgement",
        Duration::from_secs(45),
        || (source.health().canonical_through == 130).then_some(()),
    );
    let rows = canonical_rows(
        &endpoint,
        &authority,
        BTreeMap::from([(source.origin, 130)]),
    );
    assert_eq!(rows.len(), 130);
    for row in rows {
        let source_id = row["source_id"].as_str().unwrap().parse::<usize>().unwrap();
        let raw = row["raw"].as_str().unwrap().as_bytes();
        assert_eq!(raw, stored[source_id - 1].1);
        let observation = observation::decode_versioned(raw).unwrap();
        let run_id = &observation.context().unwrap().run_id;
        assert!(runs.remove(run_id), "unexpected or repeated run {run_id}");
    }
    assert!(runs.is_empty(), "{runs:?}");
}

#[test]
#[ignore = "requires exact release executables and fabrico11y.slice"]
fn local_and_canonical_deployment_profiles() {
    let started = Instant::now();
    assert!(
        fs::read_to_string("/proc/self/cgroup")
            .unwrap()
            .contains("/fabrico11y.slice/"),
        "run this test in fabrico11y.slice"
    );
    let bins =
        PathBuf::from(std::env::var_os("FABRIC_TEST_BIN_DIR").expect("release binary directory"));
    assert!(
        bins.is_absolute(),
        "use an absolute release binary directory"
    );
    let directory = xtask::evidence::non_tmp_tempdir().unwrap();
    let root = directory.path();
    let mut alpha = Node::open(&bins, root, NODE[0].0, NODE[0].1);
    let mut beta = Node::open(&bins, root, NODE[1].0, NODE[1].1);
    let mut lane = Node::open(&bins, root, NODE[2].0, NODE[2].1);
    assert_ne!(alpha.origin, beta.origin);
    assert_ne!(alpha.origin, lane.origin);
    assert_ne!(beta.origin, lane.origin);

    let alpha_first = observations("alpha-source", 1);
    let beta_first = observations("beta-source", 1);
    let receipt = alpha.submit(&alpha_first);
    assert_eq!(receipt.id, 1);
    assert_eq!(beta.submit(&beta_first).id, 1);
    assert_eq!(alpha.health().canonical_through, 0);
    let local_data = root.join("local");
    receipt_state(&bins, root, &local_data, &alpha, &receipt, (3, "pending"));
    project(&bins, root, &local_data, &alpha.config.socket);
    receipt_state(&bins, root, &local_data, &alpha, &receipt, (0, "visible"));

    // Releases the ephemeral reservation immediately before the owned receiver starts.
    let endpoint = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let reader = root.join("reader");
    private_file(&reader, READER);
    let (mut server, authority) = receiver(&bins, root, &endpoint, [&alpha, &beta, &lane], &reader);
    lane.forward(&bins, &endpoint, &authority, None);
    alpha.forward(&bins, &endpoint, &authority, Some(&lane.config.socket));
    beta.forward(&bins, &endpoint, &authority, None);
    let alpha_second = observations("alpha-source", 2);
    let second = alpha.submit(&alpha_second);
    assert_eq!(second.id, 2);
    until("initial network handoff", Duration::from_secs(15), || {
        (alpha.health().canonical_through == 2 && beta.health().canonical_through == 1)
            .then_some(())
    });
    assert!(matches!(
        canonical::exchange(
            &endpoint,
            NODE[0].1,
            &Request::Read {
                origin: beta.origin.clone(),
                after: 0,
                through: 1,
                limit: 1
            },
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap(),
        Response::Error { .. }
    ));

    let workloads = BTreeMap::from([(alpha.origin.clone(), 2), (beta.origin.clone(), 1)]);
    let initial = canonical_rows(&endpoint, &authority, workloads);
    let expected = [
        (&alpha.origin, "1", &alpha_first),
        (&alpha.origin, "2", &alpha_second),
        (&beta.origin, "1", &beta_first),
    ];
    assert_eq!(initial.len(), expected.len());
    for (origin, id, raw) in expected {
        assert!(
            initial.iter().any(|row| {
                row["origin"] == origin.as_str()
                    && row["source_id"] == id
                    && row["raw"] == raw.as_str()
            }),
            "missing exact workload {origin}/{id}"
        );
    }
    canonical_receipt_state(
        &bins,
        root,
        &endpoint,
        &authority,
        &alpha.origin,
        &second,
        (0, "visible"),
    );
    canonical_receipt_state(
        &bins,
        root,
        &endpoint,
        &authority,
        &beta.origin,
        &second,
        (4, "unavailable"),
    );

    server.kill_wait();
    let alpha_offline = observations("alpha-offline", 3);
    let receipt = alpha.submit(&alpha_offline);
    assert_eq!(receipt.id, 3);
    assert_eq!(alpha.health().canonical_through, 2);
    canonical_receipt_state(
        &bins,
        root,
        &endpoint,
        &authority,
        &alpha.origin,
        &receipt,
        (4, "unavailable"),
    );
    project(&bins, root, &local_data, &alpha.config.socket);
    receipt_state(&bins, root, &local_data, &alpha, &receipt, (0, "visible"));
    let (_restarted, restored) = receiver(&bins, root, &endpoint, [&alpha, &beta, &lane], &reader);
    assert_eq!(authority, restored);
    until("network recovery", Duration::from_secs(20), || {
        (alpha.health().canonical_through == 3).then_some(())
    });
    let duplicate = alpha.submit(&alpha_offline);
    assert_eq!(duplicate.id, receipt.id);
    assert!(duplicate.duplicate);
    assert!(matches!(
        canonical::exchange(
            &endpoint,
            NODE[0].1,
            &Request::Ingest {
                origin: alpha.origin.clone(),
                records: vec![canonical::ForwardRecord {
                    source_id: receipt.id,
                    raw: alpha_offline.clone(),
                }],
            },
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap(),
        Response::Committed { through: 3, .. }
    ));
    let recovered = canonical_rows(
        &endpoint,
        &authority,
        BTreeMap::from([(alpha.origin.clone(), 3)]),
    );
    assert_eq!(recovered.len(), 3);
    assert_eq!(
        recovered
            .iter()
            .filter(|row| row["raw"] == alpha_offline)
            .count(),
        1
    );

    canonical_receipt_state(
        &bins,
        root,
        &endpoint,
        &authority,
        &alpha.origin,
        &receipt,
        (0, "visible"),
    );

    // Exercise one application operation while the restarted receiver measures new work.
    assert_eq!(alpha.submit(&observations("alpha-after-recovery", 4)).id, 4);
    let complete = BTreeSet::from(["metric", "event", "log", "trace"]);
    let mut melt = Vec::new();
    until("both services emit MELT", Duration::from_secs(120), || {
        let through = status(&lane.config)?.highest_record_id.min(100);
        if let Err(error) = read_lane(&lane, through, &mut melt) {
            eprintln!("retrying a read-only MELT lane snapshot: {error}");
            return None;
        }
        let coverage = signal_coverage(&melt);
        (coverage.get("spindle.self") == Some(&complete)
            && coverage.get("fabric-server.self") == Some(&complete))
        .then_some(())
    });
    let through = melt.last().unwrap().0;
    until(
        "MELT canonical acknowledgement",
        Duration::from_secs(20),
        || (lane.health().canonical_through >= through).then_some(()),
    );
    let canonical_melt = canonical_rows(
        &endpoint,
        &authority,
        BTreeMap::from([(lane.origin.clone(), through)]),
    );
    assert_eq!(canonical_melt.len(), melt.len());
    for (id, raw) in melt {
        assert!(
            canonical_melt.iter().any(|row| {
                row["origin"] == lane.origin
                    && row["source_id"]
                        .as_str()
                        .and_then(|source| source.parse().ok())
                        == Some(id)
                    && row["raw"].as_str().unwrap().as_bytes() == raw
            }),
            "missing exact canonical MELT row {id}"
        );
    }
    let local_melt = root.join("local-melt");
    let projection = local(
        &bins,
        root,
        &local_melt,
        &[
            "daemon",
            "--spindle-only",
            "--interval-ms",
            "500",
            "--max-ticks",
            "32",
            "--spindle-socket",
            lane.config.socket.to_str().unwrap(),
        ],
    );
    assert!(
        projection.status.success(),
        "{}",
        String::from_utf8_lossy(&projection.stderr)
    );
    for domain in ["metrics", "logs", "traces"] {
        let query = parse_output(
            local(
                &bins,
                root,
                &local_melt,
                &["query", domain, "--since", "1h", "--json"],
            ),
            0,
        );
        let rows = query["data"]["rows"].as_array().unwrap();
        for service in ["spindle", "fabric-server"] {
            let producer = format!("{service}.self");
            match domain {
                "metrics" => assert!(
                    rows.iter().any(|row| {
                        let labels: Value =
                            serde_json::from_str(row["labels_json"].as_str().unwrap()).unwrap();
                        labels["producer"] == producer
                    }),
                    "{service} metric missing locally"
                ),
                "logs" => {
                    for kind in ["event", "log"] {
                        assert!(
                            rows.iter().any(|row| {
                                let attributes: Value =
                                    serde_json::from_str(row["attributes_json"].as_str().unwrap())
                                        .unwrap();
                                row["source"] == producer && attributes["kind"] == kind
                            }),
                            "{service} {kind} missing locally"
                        );
                    }
                }
                "traces" => assert!(
                    rows.iter().any(|row| row["service"] == service),
                    "{service} trace missing locally"
                ),
                _ => unreachable!(),
            }
        }
    }
    println!(
        "{}",
        json!({
            "passed": true,
            "transport": "same-host loopback",
            "authority": authority,
            "workload_origins": [alpha.origin, beta.origin],
            "telemetry_origin": lane.origin,
            "canonical_telemetry_through": through,
            "telemetry_signals": complete,
            "receiver_restart": true,
            "elapsed_ms": started.elapsed().as_millis(),
        })
    );
}
