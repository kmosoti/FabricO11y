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

fn open_batch_reader(
    bins: &Path,
    socket: &Path,
    outbox: &Path,
    endpoint: &str,
    authority: &str,
    credential: &Path,
) -> (OwnedChild, mpsc::Receiver<Value>) {
    let mut child = OwnedChild(
        Command::new(bins.join("fabric-adapter"))
            .arg(socket)
            .args(["batch-fixture", "--batch-outbox"])
            .arg(outbox)
            .args(["--canonical-reader", endpoint, authority])
            .arg(credential)
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

#[test]
#[ignore = "requires exact release executables and fabrico11y.slice"]
fn application_exact_readback_with_independent_reader() {
    assert!(
        fs::read_to_string("/proc/self/cgroup")
            .unwrap()
            .contains("/fabrico11y.slice/")
    );
    let bins = PathBuf::from(std::env::var_os("FABRIC_TEST_BIN_DIR").unwrap());
    let sandbox = xtask::evidence::non_tmp_tempdir().unwrap();
    let root = sandbox.path();
    let mut source = Node::open(&bins, root, NODE[0].0, NODE[0].1);
    let beta = Node::open(&bins, root, NODE[1].0, NODE[1].1);
    let lane = Node::open(&bins, root, NODE[2].0, NODE[2].1);
    let endpoint = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let credential = root.join("reader");
    private_file(&credential, READER);
    let (_server, authority) =
        receiver(&bins, root, &endpoint, [&source, &beta, &lane], &credential);
    source.forward(&bins, &endpoint, &authority, None);
    let outbox = root.join("batch-outbox");
    let (mut adapter, replies) = open_batch_reader(
        &bins,
        &source.config.socket,
        &outbox,
        &endpoint,
        &authority,
        &credential,
    );
    let parent_id = "00000000-0000-4001-8000-000000000001";
    let child_id = "00000000-0000-4001-8000-000000000002";
    for (id, parent) in [(parent_id, None), (child_id, Some(parent_id))] {
        batch_send(
            &mut adapter,
            &json!({"command":"record_application","report_id":id,"context":{"run_id":"00000000-0000-4000-8000-000000000001"},"scope":{"origin":source.origin,"producer":"batch-fixture"},"parent_report_id":parent,"schema":"example.result.v1","producer_completion":{"unix_ms":42,"clock":{"clock_id":"producer","source":"fixture","uncertainty_ms":null}},"artifact_json":"{\"value\":1}\n","timestamp_ns":123}),
        );
        let response = batch_reply(&replies);
        assert_eq!(
            response["delivery_state"], "custody_confirmed",
            "{response}"
        );
    }
    until(
        "canonical application handoff",
        Duration::from_secs(20),
        || (source.health().canonical_through >= 2).then_some(()),
    );
    batch_send(
        &mut adapter,
        &json!({"command":"verify_application","report_id":child_id}),
    );
    let verified = batch_reply(&replies);
    assert_eq!(
        verified["verified_application"]["result"]["report_id"], child_id,
        "{verified}"
    );
    let first = verified["verified_application"]["first_verified_read"].clone();
    batch_send(
        &mut adapter,
        &json!({"command":"read_application_parent","report_id":child_id}),
    );
    let parent = batch_reply(&replies);
    assert_eq!(
        parent["verified_parent"]["result"]["report_id"], parent_id,
        "{parent}"
    );
    adapter.kill_wait();
    let (mut adapter, replies) = open_batch_reader(
        &bins,
        &source.config.socket,
        &outbox,
        &endpoint,
        &authority,
        &credential,
    );
    batch_send(
        &mut adapter,
        &json!({"command":"report_status","report_id":child_id}),
    );
    assert_eq!(
        batch_reply(&replies)["report_status"]["first_verified_read"],
        first
    );
    adapter.kill_wait();
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

fn independent_sha256(bytes: &[u8]) -> String {
    let mut command = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    command.stdin.take().unwrap().write_all(bytes).unwrap();
    let output = command.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

fn synthetic_rows(script: &str, count: usize, interrupt: bool) -> Vec<(String, bool)> {
    let mut producer = Command::new("sh")
        .args(["-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(producer.stdout.take().unwrap());
    let mut rows = Vec::new();
    for _ in 0..count {
        let mut line = String::new();
        assert!(output.read_line(&mut line).unwrap() > 0);
        let (id, result) = line.trim().split_once(':').unwrap();
        rows.push((
            id.to_owned(),
            match result {
                "exact" => true,
                "different" => false,
                _ => panic!("invalid synthetic row"),
            },
        ));
    }
    if interrupt {
        producer.kill().unwrap();
        assert!(!producer.wait().unwrap().success());
    } else {
        assert!(producer.wait().unwrap().success());
    }
    rows
}

fn application_request(
    id: &str,
    parent: Option<&str>,
    origin: &str,
    artifact: &str,
    completion_ms: i64,
) -> Value {
    json!({"command":"record_application","report_id":id,"context":{"run_id":"00000000-0000-4000-8000-000000000001"},"scope":{"origin":origin,"producer":"batch-fixture"},"parent_report_id":parent,"schema":"example.result.v1","producer_completion":{"unix_ms":completion_ms,"clock":{"clock_id":"fixture.system_time.unix_ms","source":"fixture.producer_completion","uncertainty_ms":null}},"artifact_json":artifact,"timestamp_ns":123})
}

fn wall_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap()
}

fn verify_until(adapter: &mut OwnedChild, replies: &mpsc::Receiver<Value>, id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        batch_send(
            adapter,
            &json!({"command":"verify_application","report_id":id}),
        );
        let reply = batch_reply(replies);
        if reply.get("verified_application").is_some() {
            return reply;
        }
        assert!(
            Instant::now() < deadline,
            "canonical readback timeout: {reply}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "requires exact release executables and fabrico11y.slice"]
fn application_eight_case_restart_and_paired_delivery() {
    assert!(
        fs::read_to_string("/proc/self/cgroup")
            .unwrap()
            .contains("/fabrico11y.slice/")
    );
    let bins = PathBuf::from(std::env::var_os("FABRIC_TEST_BIN_DIR").unwrap());
    let sandbox = xtask::evidence::non_tmp_tempdir().unwrap();
    let root = sandbox.path();
    let roster: BTreeSet<_> = (1..=8).map(|i| format!("case-{i:02}")).collect();
    let primary = synthetic_rows(
        "printf 'case-01:exact\\ncase-02:exact\\ncase-03:exact\\ncase-04:different\\n'",
        4,
        false,
    );
    let supplement = synthetic_rows(
        "printf 'case-05:exact\\ncase-06:exact\\ncase-07:different\\n'; exec sleep 60",
        3,
        true,
    );
    let evaluate = |rows: &[(String, bool)]| {
        let ids: BTreeSet<_> = rows.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(ids.len(), rows.len());
        assert!(ids.is_subset(&roster));
        (
            rows.len(),
            rows.iter().filter(|(_, exact)| *exact).count(),
            roster.len() - rows.len(),
        )
    };
    assert_eq!(evaluate(&primary), (4, 3, 4));
    let mut combined = primary.clone();
    combined.extend(supplement);
    assert_eq!(evaluate(&combined), (7, 5, 1));
    let mut source = Node::open(&bins, root, NODE[0].0, NODE[0].1);
    let beta = Node::open(&bins, root, NODE[1].0, NODE[1].1);
    let lane = Node::open(&bins, root, NODE[2].0, NODE[2].1);
    let endpoint = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let credential = root.join("reader");
    private_file(&credential, READER);
    let (mut server, authority) =
        receiver(&bins, root, &endpoint, [&source, &beta, &lane], &credential);
    source.forward(&bins, &endpoint, &authority, None);
    let outbox = root.join("batch-outbox");
    let primary_id = "00000000-0000-4001-8000-000000000001";
    let supplement_id = "00000000-0000-4001-8000-000000000002";
    let payload = |record_id: &str, parent: Option<&str>, rows: &[(String, bool)]| {
        let (valid, exact, unavailable) = evaluate(rows);
        let states: BTreeMap<_, _> = rows
            .iter()
            .map(|(id, exact)| (id.as_str(), *exact))
            .collect();
        let cases: Vec<_> = roster.iter().map(|id| json!({"id":id,"state":match states.get(id.as_str()) { Some(true) => "exact", Some(false) => "known", None => "unavailable" }})).collect();
        let mut payload = json!({"schema":"example.result.v1","record_id":record_id,"enrolled":roster.len(),"valid":valid,"exact":exact,"unavailable":unavailable,"cases":cases});
        if let Some(parent) = parent {
            payload["parent_report_id"] = parent.into();
            payload["relation"] = "supplements".into();
        }
        payload
    };
    let primary_payload = payload("fixture.primary", None, &primary);
    let supplement_payload = payload("fixture.supplementary", Some(primary_id), &combined);
    let primary_json = format!("{}\n", serde_json::to_string(&primary_payload).unwrap());
    let supplement_json = format!("{}\n", serde_json::to_string(&supplement_payload).unwrap());
    let first_request = application_request(primary_id, None, &source.origin, &primary_json, 42);
    let second_request = application_request(
        supplement_id,
        Some(primary_id),
        &source.origin,
        &supplement_json,
        43,
    );
    let (mut adapter, replies) = open_batch_reader(
        &bins,
        &source.config.socket,
        &outbox,
        &endpoint,
        &authority,
        &credential,
    );
    batch_send(&mut adapter, &first_request);
    let first = batch_reply(&replies);
    assert_eq!(first["delivery_state"], "custody_confirmed", "{first}");
    until("primary canonical handoff", Duration::from_secs(20), || {
        (source.health().canonical_through >= 1).then_some(())
    });
    let first_verified = verify_until(&mut adapter, &replies, primary_id)["verified_application"]["first_verified_read"].clone();
    adapter.kill_wait();
    server.kill_wait();
    let proxy_path = root.join("replay-proxy.sock");
    let proxy = UnixListener::bind(&proxy_path).unwrap();
    proxy.set_nonblocking(true).unwrap();
    let real_socket = source.config.socket.clone();
    let (sent, received) = mpsc::sync_channel(1);
    let fault = thread::spawn(move || {
        loop {
            let mut adapter = until("replay proxy", Duration::from_secs(10), || {
                match proxy.accept() {
                    Ok((socket, _)) => Some(socket),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => None,
                    Err(e) => panic!("proxy: {e}"),
                }
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
                break;
            }
            write_spindle_frame(&mut adapter, &response);
        }
    });
    let (mut adapter, replies) = open_batch_reader(
        &bins,
        &proxy_path,
        &outbox,
        &endpoint,
        &authority,
        &credential,
    );
    batch_send(&mut adapter, &second_request);
    let original_receipt = received.recv_timeout(Duration::from_secs(15)).unwrap();
    let unknown = batch_reply(&replies);
    assert_eq!(unknown["delivery_state"], "outcome_unknown", "{unknown}");
    adapter.kill_wait();
    fault.join().unwrap();
    let (mut adapter, replies) = open_batch_reader(
        &bins,
        &source.config.socket,
        &outbox,
        &endpoint,
        &authority,
        &credential,
    );
    batch_send(&mut adapter, &second_request);
    let replay = batch_reply(&replies);
    assert_eq!(replay["delivery_state"], "custody_confirmed", "{replay}");
    let replay_receipt: Receipt = serde_json::from_value(replay["receipt"].clone()).unwrap();
    assert_eq!(replay_receipt.id, original_receipt.id);
    assert_eq!(replay_receipt.identity, original_receipt.identity);
    batch_send(
        &mut adapter,
        &json!({"command":"verify_application","report_id":supplement_id}),
    );
    let pending = batch_reply(&replies);
    assert!(pending.get("verified_application").is_none(), "{pending}");
    batch_send(
        &mut adapter,
        &json!({"command":"report_status","report_id":supplement_id}),
    );
    assert!(batch_reply(&replies)["report_status"]["first_verified_read"].is_null());
    let (server, restarted_authority) =
        receiver(&bins, root, &endpoint, [&source, &beta, &lane], &credential);
    assert_eq!(restarted_authority, authority);
    let exact_request = canonical::ExactReceiptRequest {
        authority: authority.clone(),
        origin: source.origin.clone(),
        source_id: replay_receipt.id,
        identity: replay_receipt.identity.clone(),
    };
    let denied = canonical::exchange(
        &endpoint,
        NODE[0].1,
        &Request::ExactReceiptLookup {
            request: exact_request.clone(),
        },
        Instant::now() + Duration::from_secs(3),
    )
    .unwrap();
    assert!(matches!(denied, Response::Error { .. }));
    let mut wrong_authority = exact_request;
    wrong_authority.authority = "wrong-authority".into();
    let denied = canonical::exchange(
        &endpoint,
        READER,
        &Request::ExactReceiptLookup {
            request: wrong_authority,
        },
        Instant::now() + Duration::from_secs(3),
    )
    .unwrap();
    assert!(matches!(denied, Response::Error { .. }));
    until(
        "supplement canonical handoff",
        Duration::from_secs(20),
        || (source.health().canonical_through >= 2).then_some(()),
    );
    let second = verify_until(&mut adapter, &replies, supplement_id);
    batch_send(
        &mut adapter,
        &json!({"command":"read_application_parent","report_id":supplement_id}),
    );
    let parent = batch_reply(&replies);
    assert_eq!(
        parent["verified_parent"]["result"]["report_id"], primary_id,
        "{parent}"
    );
    assert_eq!(
        parent["verified_parent"]["first_verified_read"],
        first_verified
    );
    assert_eq!(
        parent["verified_parent"]["result"]["payload"],
        primary_payload
    );
    assert_eq!(
        second["verified_application"]["result"]["payload"],
        supplement_payload
    );
    for (raw, result) in [
        (primary_json.as_str(), &parent["verified_parent"]["result"]),
        (
            supplement_json.as_str(),
            &second["verified_application"]["result"],
        ),
    ] {
        let value: Value = serde_json::from_str(raw).unwrap();
        let canonical = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            result["original_artifact_digest"]["hex"],
            independent_sha256(raw.as_bytes())
        );
        assert_eq!(
            result["application_payload_digest"]["hex"],
            independent_sha256(&canonical)
        );
    }
    let mut changed = second_request.clone();
    changed["artifact_json"] = format!("{supplement_json}\n").into();
    batch_send(&mut adapter, &changed);
    assert_eq!(batch_reply(&replies)["code"], "conflict");
    let mut rows = Vec::new();
    read_lane(&source, 2, &mut rows).unwrap();
    assert_eq!(rows.len(), 2);
    // Each arm receives the same three independent completion events at 0/80/160ms.
    // Immediate consumes on arrival; periodic sweeps all queued completions at 250ms ticks.
    for pair in 0..3 {
        for periodic in if pair % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let (sent, received) = mpsc::channel();
            let base = Instant::now() + Duration::from_millis(100);
            let producer = thread::spawn(move || {
                for (job, offset_ms) in [0_u64, 80, 160].into_iter().enumerate() {
                    let scheduled = base + Duration::from_millis(offset_ms);
                    if let Some(wait) = scheduled.checked_duration_since(Instant::now()) {
                        thread::sleep(wait);
                    }
                    sent.send((job, offset_ms, Instant::now(), wall_millis()))
                        .unwrap();
                }
            });
            let mut completed = 0;
            let mut tick = 1_u32;
            while completed < 3 {
                let mut batch = Vec::new();
                let sweep_tick = if periodic {
                    let scheduled_tick = base + Duration::from_millis(u64::from(tick) * 250);
                    if let Some(wait) = scheduled_tick.checked_duration_since(Instant::now()) {
                        thread::sleep(wait);
                    }
                    while let Ok(event) = received.try_recv() {
                        batch.push(event);
                    }
                    let current = tick;
                    tick += 1;
                    Some(current)
                } else {
                    batch.push(received.recv_timeout(Duration::from_secs(5)).unwrap());
                    None
                };
                for (job, offset_ms, finished, finish_wall_ms) in batch {
                    let id = format!(
                        "00000000-0000-4001-8000-{:012x}",
                        10 + pair * 6 + usize::from(periodic) * 3 + job
                    );
                    let request = application_request(
                        &id,
                        None,
                        &source.origin,
                        &primary_json,
                        finish_wall_ms,
                    );
                    let dispatch_ns = finished.elapsed().as_nanos();
                    batch_send(&mut adapter, &request);
                    let ack = batch_reply(&replies);
                    let ack_ns = finished.elapsed().as_nanos();
                    assert_eq!(ack["delivery_state"], "custody_confirmed", "{ack}");
                    let staged_ms = ack["application_timing"]["outbox_staged_at"]["unix_ms"]
                        .as_i64()
                        .unwrap();
                    assert!(staged_ms >= finish_wall_ms, "wall clock regressed");
                    let verified = verify_until(&mut adapter, &replies, &id);
                    let verified_ns = finished.elapsed().as_nanos();
                    assert!(
                        verified["verified_application"]["first_verified_read"]["unix_ms"]
                            .as_i64()
                            .unwrap()
                            >= staged_ms,
                        "wall clock regressed"
                    );
                    println!(
                        "FABRIC196_SAMPLE {}",
                        json!({"pair":pair,"job":job,"mode":if periodic {"periodic"} else {"immediate"},"period_ms":250,"scheduled_completion_offset_ms":offset_ms,"actual_completion_since_start_ns":finished.duration_since(base).as_nanos(),"sweep_tick":sweep_tick,"finish_wall_ms":finish_wall_ms,"stage_wall_ms":staged_ms,"finish_to_stage_ms":staged_ms-finish_wall_ms,"finish_to_dispatch_ns":dispatch_ns,"finish_to_ack_ns":ack_ns,"finish_to_verified_response_ns":verified_ns,"first_verified_read":verified["verified_application"]["first_verified_read"],"payload_sha256":independent_sha256(primary_json.as_bytes())})
                    );
                    completed += 1;
                }
            }
            producer.join().unwrap();
        }
    }
    adapter.kill_wait();
    let stored = fs::read(outbox.join("outbox.sqlite")).unwrap();
    assert!(
        !stored
            .windows(primary_json.len())
            .any(|bytes| bytes == primary_json.as_bytes())
    );
    assert!(
        !stored
            .windows(supplement_json.len())
            .any(|bytes| bytes == supplement_json.as_bytes())
    );
    drop(server);
}

#[test]
#[ignore = "requires exact release executables and fabrico11y.slice"]
fn snapshot_readback_survives_packaged_restart() {
    assert!(
        fs::read_to_string("/proc/self/cgroup")
            .unwrap()
            .contains("/fabrico11y.slice/")
    );
    let bins = PathBuf::from(std::env::var_os("FABRIC_TEST_BIN_DIR").unwrap());
    let sandbox = xtask::evidence::non_tmp_tempdir().unwrap();
    let root = sandbox.path();
    let mut source = Node::open(&bins, root, NODE[0].0, NODE[0].1);
    let beta = Node::open(&bins, root, NODE[1].0, NODE[1].1);
    let lane = Node::open(&bins, root, NODE[2].0, NODE[2].1);
    let endpoint = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let credential = root.join("reader");
    private_file(&credential, READER);
    let (mut server, authority) =
        receiver(&bins, root, &endpoint, [&source, &beta, &lane], &credential);
    source.forward(&bins, &endpoint, &authority, None);
    let outbox = root.join("snapshot-outbox");
    let (mut adapter, replies) = open_batch_reader(
        &bins,
        &source.config.socket,
        &outbox,
        &endpoint,
        &authority,
        &credential,
    );
    let report_id = |n: u64| format!("00000000-0000-4001-8000-{n:012x}");
    let clock = json!({"unix_ms":42,"clock":{"clock_id":"producer","source":"fixture","uncertainty_ms":null}});
    let make_ref = |id: &str, hex: &str| json!({"id":id,"digest":{"algorithm":"sha256","representation":"exact_bytes","hex":hex.repeat(64)}});
    let source_ref = make_ref("cutoff-source", "a");
    let definition = json!({"evaluator":make_ref("evaluator","b"),"configuration":make_ref("configuration","c"),"unit":"cases","aggregation":"count"});
    let mut case_refs: Vec<Value> = Vec::new();
    let record = |adapter: &mut OwnedChild,
                  id: &str,
                  parent: Option<&str>,
                  schema: &str,
                  artifact: &Value|
     -> Value {
        batch_send(
            adapter,
            &json!({"command":"record_application","report_id":id,"context":{"run_id":"00000000-0000-4000-8000-000000000001"},"scope":{"origin":source.origin,"producer":"batch-fixture"},"parent_report_id":parent,"schema":schema,"producer_completion":clock,"artifact_json":artifact.to_string(),"timestamp_ns":123}),
        );
        let response = batch_reply(&replies);
        assert_eq!(
            response["delivery_state"], "custody_confirmed",
            "{response}"
        );
        response
    };
    let case = |id: usize, refs: &[Value]| -> Value {
        if id <= refs.len() {
            json!({"id":format!("case-{id:02}"),"evidence":{"state":"known","reference":refs[id-1]}})
        } else {
            json!({"id":format!("case-{id:02}"),"evidence":{"state":"unknown","reason":"unavailable"}})
        }
    };
    let payload = |ordinal: u64, refs: &[Value], parent_digest: Option<Value>| -> Value {
        json!({"schema":"fabric.snapshot.v1","cutoff":{"source":source_ref,"ordinal":ordinal,"time":clock},"definition":definition,"cases":(1..=8).map(|i| case(i, refs)).collect::<Vec<_>>(),"parent_payload_digest":parent_digest})
    };
    for i in 1..=4 {
        let id = report_id(i);
        let response = record(
            &mut adapter,
            &id,
            None,
            "example.case.v1",
            &json!({"schema":"example.case.v1","case":format!("case-{i:02}"),"exact":i <= 3}),
        );
        case_refs.push(json!({"report_id":id,"payload_digest":response["application_payload_digest"],"expected_source_id":response["receipt"]["id"]}));
    }
    let first_id = report_id(100);
    let first = record(
        &mut adapter,
        &first_id,
        None,
        "fabric.snapshot.v1",
        &payload(1, &case_refs, None),
    );
    for i in 5..=7 {
        let id = report_id(i);
        let response = record(
            &mut adapter,
            &id,
            None,
            "example.case.v1",
            &json!({"schema":"example.case.v1","case":format!("case-{i:02}"),"exact":i <= 6}),
        );
        case_refs.push(json!({"report_id":id,"payload_digest":response["application_payload_digest"],"expected_source_id":response["receipt"]["id"]}));
    }
    let second_id = report_id(101);
    record(
        &mut adapter,
        &second_id,
        Some(&first_id),
        "fabric.snapshot.v1",
        &payload(
            2,
            &case_refs,
            Some(first["application_payload_digest"].clone()),
        ),
    );
    let mut bad_refs = case_refs.clone();
    bad_refs[0]["payload_digest"]["hex"] = "f".repeat(64).into();
    let bad_digest_id = report_id(102);
    record(
        &mut adapter,
        &bad_digest_id,
        None,
        "fabric.snapshot.v1",
        &payload(1, &bad_refs, None),
    );
    let mut missing_refs = case_refs.clone();
    missing_refs[0]["report_id"] = report_id(999).into();
    missing_refs[0]["expected_source_id"] = Value::Null;
    let missing_id = report_id(103);
    record(
        &mut adapter,
        &missing_id,
        None,
        "fabric.snapshot.v1",
        &payload(1, &missing_refs, None),
    );
    // Explicit, unrelated roots compare retained typed measurements after restart.
    let measurement_schema = "fabric.snapshot.case-measurement.v1";
    let measurement = |value: i64, exact: bool, definition: &Value| {
        json!({
            "schema":measurement_schema,"case_id":"case-01","definition":definition,"value":value,"exact":exact
        })
    };
    let reference = |id: &str, response: &Value| {
        json!({
            "report_id":id,"payload_digest":response["application_payload_digest"],"expected_source_id":response["receipt"]["id"]
        })
    };
    let measure_a_id = report_id(110);
    let measure_a = record(
        &mut adapter,
        &measure_a_id,
        None,
        measurement_schema,
        &measurement(i64::MIN, true, &definition),
    );
    let measure_b_id = report_id(111);
    let measure_b = record(
        &mut adapter,
        &measure_b_id,
        None,
        measurement_schema,
        &measurement(i64::MAX, false, &definition),
    );
    let compare_a_id = report_id(112);
    let compare_a = record(
        &mut adapter,
        &compare_a_id,
        None,
        "fabric.snapshot.v1",
        &payload(
            3,
            &[reference(&measure_a_id, &measure_a), case_refs[1].clone()],
            None,
        ),
    );
    let compare_b_id = report_id(113);
    let compare_b = record(
        &mut adapter,
        &compare_b_id,
        None,
        "fabric.snapshot.v1",
        &payload(
            4,
            &[reference(&measure_b_id, &measure_b), case_refs[1].clone()],
            None,
        ),
    );
    let mut wrong_definition = definition.clone();
    wrong_definition["unit"] = "seconds".into();
    let bad_measure_id = report_id(114);
    let bad_measure = record(
        &mut adapter,
        &bad_measure_id,
        None,
        measurement_schema,
        &measurement(7, true, &wrong_definition),
    );
    let bad_compare_id = report_id(115);
    let bad_compare = record(
        &mut adapter,
        &bad_compare_id,
        None,
        "fabric.snapshot.v1",
        &payload(
            5,
            &[
                reference(&bad_measure_id, &bad_measure),
                case_refs[1].clone(),
            ],
            None,
        ),
    );
    let baseline_ref = reference(&compare_a_id, &compare_a);
    let candidate_ref = reference(&compare_b_id, &compare_b);
    let bad_comparison_ref = reference(&bad_compare_id, &bad_compare);
    let attempt_schema = "fabric.prior-failure.attempt-observation.v1";
    let attempt_identity = json!({"attempt":"attempt-1","candidate":"candidate-1","intended_strategy":"strategy","actual_artifact":"artifact","diagnosis":"timeout"});
    let attempt_body = json!({"schema":attempt_schema,"attempt":"attempt-1","candidate":"candidate-1","intended_strategy":"strategy","actual_artifact":"artifact","diagnosis":"timeout","state":"observed_failure"});
    let attempt_id = report_id(116);
    let attempt = record(
        &mut adapter,
        &attempt_id,
        None,
        attempt_schema,
        &attempt_body,
    );
    let wrong_schema_id = report_id(117);
    let mut wrong_schema_body = attempt_body.clone();
    wrong_schema_body["schema"] = "example.case.v1".into();
    let wrong_schema = record(
        &mut adapter,
        &wrong_schema_id,
        None,
        "example.case.v1",
        &wrong_schema_body,
    );
    let attempt_request = json!({"command":"verify_attempt_observation", "reference":reference(&attempt_id, &attempt),
        "expected_scope":{"origin":source.origin,"producer":"batch-fixture"},
        "expected_original_artifact_digest":attempt["original_artifact_digest"],
        "expected_identity":attempt_identity,"timeout_ms":30000});
    let mut wrong_schema_request = attempt_request.clone();
    wrong_schema_request["reference"] = reference(&wrong_schema_id, &wrong_schema);
    wrong_schema_request["expected_original_artifact_digest"] =
        wrong_schema["original_artifact_digest"].clone();
    until(
        "snapshot canonical handoff",
        Duration::from_secs(30),
        || (source.health().canonical_through >= 19).then_some(()),
    );
    adapter.kill_wait();
    server.kill_wait();
    let (mut server, same_authority) =
        receiver(&bins, root, &endpoint, [&source, &beta, &lane], &credential);
    assert_eq!(same_authority, authority);
    let (mut adapter, replies) = open_batch_reader(
        &bins,
        &source.config.socket,
        &outbox,
        &endpoint,
        &authority,
        &credential,
    );
    for (id, known, exact) in [(&first_id, 4, 3), (&second_id, 7, 5)] {
        batch_send(
            &mut adapter,
            &json!({"command":"verify_snapshot","report_id":id,"timeout_ms":30000}),
        );
        let verified = replies
            .recv_timeout(Duration::from_secs(35))
            .expect("snapshot verification reply");
        let boundary = &verified["verified_snapshot"]["root"];
        assert_eq!(boundary["report_id"], *id, "{verified}");
        let rows = boundary["payload"]["cases"].as_array().unwrap();
        assert_eq!(rows.len(), 8);
        assert_eq!(
            rows.iter()
                .filter(|c| c["evidence"]["state"] == "known")
                .count(),
            known
        );
        let mut exact_count = 0;
        for (index, row) in rows.iter().enumerate() {
            assert_eq!(row["id"], format!("case-{:02}", index + 1));
            if row["evidence"]["state"] == "known" {
                batch_send(
                    &mut adapter,
                    &json!({"command":"verify_application","report_id":row["evidence"]["reference"]["report_id"]}),
                );
                let item = batch_reply(&replies);
                assert_eq!(
                    item["verified_application"]["result"]["payload"]["case"], row["id"],
                    "{item}"
                );
                exact_count +=
                    usize::from(item["verified_application"]["result"]["payload"]["exact"] == true);
            }
        }
        assert_eq!(exact_count, exact);
        if id == &second_id {
            assert_eq!(
                verified["verified_snapshot"]["parent"]["report_id"],
                first_id
            );
        }
    }
    batch_send(
        &mut adapter,
        &json!({"command":"compare_snapshots","baseline":baseline_ref,"candidate":candidate_ref,"timeout_ms":30000}),
    );
    let comparison = replies.recv_timeout(Duration::from_secs(35)).unwrap();
    let compared = &comparison["snapshot_comparison"];
    assert_eq!(compared["baseline_counts"]["enrolled"], 8, "{comparison}");
    assert_eq!(compared["baseline_counts"]["known"], 1);
    assert_eq!(compared["candidate_counts"]["known"], 1);
    assert_eq!(
        compared["cases"]["case-01"]["difference"],
        "18446744073709551615"
    );
    assert_eq!(compared["cases"]["case-01"]["baseline_exact"], true);
    assert_eq!(compared["cases"]["case-01"]["candidate_exact"], false);
    assert_eq!(compared["cases"]["case-02"]["state"], "unavailable");
    assert_eq!(compared["cases"]["case-03"]["state"], "unavailable");
    batch_send(
        &mut adapter,
        &json!({"command":"compare_snapshots","baseline":baseline_ref,"candidate":bad_comparison_ref,"timeout_ms":30000}),
    );
    let bad = replies.recv_timeout(Duration::from_secs(35)).unwrap();
    assert!(
        bad.get("error").is_some() && bad.get("snapshot_comparison").is_none(),
        "{bad}"
    );
    let mut wrong_digest = candidate_ref.clone();
    wrong_digest["payload_digest"]["hex"] = "f".repeat(64).into();
    batch_send(
        &mut adapter,
        &json!({"command":"compare_snapshots","baseline":baseline_ref,"candidate":wrong_digest,"timeout_ms":30000}),
    );
    let bad = replies.recv_timeout(Duration::from_secs(35)).unwrap();
    assert!(
        bad.get("error").is_some() && bad.get("snapshot_comparison").is_none(),
        "{bad}"
    );
    batch_send(&mut adapter, &attempt_request);
    let verified = replies.recv_timeout(Duration::from_secs(35)).unwrap();
    let observation = &verified["verified_attempt_observation"];
    assert_eq!(observation["report_id"], attempt_id, "{verified}");
    assert_eq!(observation["receipt"]["id"], attempt["receipt"]["id"]);
    assert_eq!(observation["observation"], attempt_body);
    for field in [
        "scope",
        "diagnosis",
        "payload_digest",
        "artifact_digest",
        "source_id",
        "schema",
    ] {
        let mut request = attempt_request.clone();
        match field {
            "scope" => request["expected_scope"]["producer"] = "other".into(),
            "diagnosis" => request["expected_identity"]["diagnosis"] = "other".into(),
            "payload_digest" => {
                request["reference"]["payload_digest"]["hex"] = "f".repeat(64).into()
            }
            "artifact_digest" => {
                request["expected_original_artifact_digest"]["hex"] = "f".repeat(64).into()
            }
            "source_id" => request["reference"]["expected_source_id"] = 999.into(),
            "schema" => request = wrong_schema_request.clone(),
            _ => unreachable!(),
        }
        batch_send(&mut adapter, &request);
        let rejected = replies.recv_timeout(Duration::from_secs(35)).unwrap();
        assert!(
            rejected.get("error").is_some()
                && rejected.get("verified_attempt_observation").is_none(),
            "{field}: {rejected}"
        );
    }
    for id in [&bad_digest_id, &missing_id] {
        batch_send(
            &mut adapter,
            &json!({"command":"verify_snapshot","report_id":id,"timeout_ms":30000}),
        );
        let reply = replies.recv_timeout(Duration::from_secs(35)).unwrap();
        assert!(
            reply.get("error").is_some() && reply.get("verified_snapshot").is_none(),
            "{reply}"
        );
    }
    batch_send(
        &mut adapter,
        &json!({"command":"verify_snapshot","report_id":first_id,"timeout_ms":0}),
    );
    assert_eq!(batch_reply(&replies)["code"], "invalid");
    adapter.kill_wait();
    let (mut wrong, replies) = open_batch_reader(
        &bins,
        &source.config.socket,
        &outbox,
        &endpoint,
        "wrong-authority",
        &credential,
    );
    batch_send(
        &mut wrong,
        &json!({"command":"verify_snapshot","report_id":second_id,"timeout_ms":30000}),
    );
    let reply = replies.recv_timeout(Duration::from_secs(35)).unwrap();
    assert!(
        reply.get("error").is_some() && reply.get("verified_snapshot").is_none(),
        "{reply}"
    );
    batch_send(&mut wrong, &attempt_request);
    let rejected = replies.recv_timeout(Duration::from_secs(35)).unwrap();
    assert!(
        rejected.get("error").is_some() && rejected.get("verified_attempt_observation").is_none()
    );
    wrong.kill_wait();
    server.kill_wait();
}

fn independent_sha256(bytes: &[u8]) -> String {
    let mut child = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

fn open_protected_reader(
    bins: &Path,
    socket: &Path,
    outbox: &Path,
    policy: &Path,
    origin: &str,
    reader: (&str, &str, &Path),
) -> (OwnedChild, mpsc::Receiver<Value>) {
    let (endpoint, authority, credential) = reader;
    let mut child = OwnedChild(
        Command::new(bins.join("fabric-adapter"))
            .arg(socket)
            .args(["batch-fixture", "--batch-outbox"])
            .arg(outbox)
            .arg("--protected-policy")
            .arg(policy)
            .arg(origin)
            .args(["--canonical-reader", endpoint, authority])
            .arg(credential)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = BufReader::new(child.0.stdout.take().unwrap());
    let (sent, received) = mpsc::sync_channel(1);
    thread::spawn(move || {
        for line in stdout.lines() {
            let value = serde_json::from_str(&line.unwrap()).unwrap();
            if sent.send(value).is_err() {
                break;
            }
        }
    });
    (child, received)
}

#[test]
#[ignore = "requires exact release executables and fabrico11y.slice"]
fn protected_application_real_custody_and_policy_hold() {
    assert!(
        fs::read_to_string("/proc/self/cgroup")
            .unwrap()
            .contains("/fabrico11y.slice/")
    );
    let bins = PathBuf::from(std::env::var_os("FABRIC_TEST_BIN_DIR").unwrap());
    let sandbox = xtask::evidence::non_tmp_tempdir().unwrap();
    let root = sandbox.path();
    let mut source = Node::open(&bins, root, NODE[0].0, NODE[0].1);
    let beta = Node::open(&bins, root, NODE[1].0, NODE[1].1);
    let lane = Node::open(&bins, root, NODE[2].0, NODE[2].1);
    let endpoint = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let reader = root.join("reader");
    private_file(&reader, READER);
    let (_server, authority) = receiver(&bins, root, &endpoint, [&source, &beta, &lane], &reader);
    source.forward(&bins, &endpoint, &authority, None);
    let policy_text = r#"{"version":1,"policy_id":"fixture","max_input_bytes":4096,"max_work":65536,"rules":[{"name":"allowed","scalar_type":"text"}]}"#;
    let policy = root.join("policy");
    private_file(&policy, policy_text);
    let wrong = root.join("wrong-policy");
    private_file(
        &wrong,
        r#"{"version":1,"policy_id":"other","max_input_bytes":4096,"max_work":65536,"rules":[]}"#,
    );
    let proxy_path = root.join("protected-proxy.sock");
    let proxy = UnixListener::bind(&proxy_path).unwrap();
    proxy.set_nonblocking(true).unwrap();
    let real = source.config.socket.clone();
    let (sent, received) = mpsc::sync_channel(1);
    let fault = thread::spawn(move || {
        loop {
            let mut adapter = until("protected proxy", Duration::from_secs(10), || {
                match proxy.accept() {
                    Ok((socket, _)) => Some(socket),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => None,
                    Err(e) => panic!("proxy: {e}"),
                }
            });
            let mut spindle = UnixStream::connect(&real).unwrap();
            for socket in [&adapter, &spindle] {
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
            }
            let req = read_spindle_frame(&mut adapter);
            write_spindle_frame(&mut spindle, &req);
            let response = read_spindle_frame(&mut spindle);
            if let Reply::Receipt { receipt } = serde_json::from_slice(&response).unwrap() {
                sent.send(receipt).unwrap();
                break;
            }
            write_spindle_frame(&mut adapter, &response);
        }
    });
    let outbox = root.join("protected-outbox");
    let (mut adapter, replies) = open_protected_reader(
        &bins,
        &proxy_path,
        &outbox,
        &policy,
        &source.origin,
        (&endpoint, &authority, &reader),
    );
    let raw = r#"{"allowed":"visible","FORBIDDEN-NAME":"FORBIDDEN-VALUE"}"#;
    let report = json!({"command":"record_application","report_id":"00000000-0000-4001-8000-000000000001","context":{"run_id":"00000000-0000-4000-8000-000000000001"},"scope":{"origin":source.origin,"producer":"batch-fixture"},"parent_report_id":null,"schema":"example.result.v1","producer_completion":{"unix_ms":42,"clock":{"clock_id":"producer","source":"fixture","uncertainty_ms":null}},"artifact_json":raw,"timestamp_ns":123});
    batch_send(&mut adapter, &report);
    let first = received.recv_timeout(Duration::from_secs(15)).unwrap();
    let unknown = batch_reply(&replies);
    assert_eq!(unknown["delivery_state"], "outcome_unknown", "{unknown}");
    let forbidden_hash = independent_sha256(raw.as_bytes());
    let clean = |bytes: &[u8]| {
        for sentinel in ["FORBIDDEN-NAME", "FORBIDDEN-VALUE", forbidden_hash.as_str()] {
            assert!(
                !bytes
                    .windows(sentinel.len())
                    .any(|w| w == sentinel.as_bytes())
            );
        }
    };
    clean(unknown.to_string().as_bytes());
    batch_send(&mut adapter, &json!({"command":"FORBIDDEN-NAME"}));
    let invalid = batch_reply(&replies);
    clean(invalid.to_string().as_bytes());
    for name in ["outbox.sqlite", "outbox.sqlite-journal"] {
        let p = outbox.join(name);
        if p.exists() {
            clean(&fs::read(p).unwrap());
        }
    }
    adapter.kill_wait();
    fault.join().unwrap();
    let (mut held, replies) = open_protected_reader(
        &bins,
        &source.config.socket,
        &outbox,
        &wrong,
        &source.origin,
        (&endpoint, &authority, &reader),
    );
    batch_send(&mut held, &json!({"command":"status"}));
    assert_eq!(batch_reply(&replies)["status"]["policy_state"], "held");
    batch_send(&mut held, &json!({"command":"flush","timeout_ms":100}));
    assert_eq!(batch_reply(&replies)["code"], "policy_held");
    held.kill_wait();
    let ordinary = Command::new(bins.join("fabric-adapter"))
        .arg(&source.config.socket)
        .args(["batch-fixture", "--batch-outbox"])
        .arg(&outbox)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!ordinary.status.success());
    clean(&ordinary.stderr);
    let other_owner = Command::new(bins.join("fabric-adapter"))
        .arg(&source.config.socket)
        .args(["different-producer", "--batch-outbox"])
        .arg(&outbox)
        .arg("--protected-policy")
        .arg(&policy)
        .arg(&source.origin)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!other_owner.status.success());
    clean(&other_owner.stderr);
    let (mut adapter, replies) = open_protected_reader(
        &bins,
        &source.config.socket,
        &outbox,
        &policy,
        &source.origin,
        (&endpoint, &authority, &reader),
    );
    batch_send(&mut adapter, &report);
    let confirmed = batch_reply(&replies);
    assert_eq!(
        confirmed["delivery_state"], "custody_confirmed",
        "{confirmed}"
    );
    clean(confirmed.to_string().as_bytes());
    let receipt: Receipt = serde_json::from_value(confirmed["receipt"].clone()).unwrap();
    assert_eq!(receipt.id, first.id);
    assert_eq!(receipt.identity, first.identity);
    let expected_capture = independent_sha256(br#"{"allowed":"visible"}"#);
    let expected_policy = independent_sha256(policy_text.as_bytes());
    assert_eq!(
        confirmed["capture"]["captured_digest"]["hex"],
        expected_capture
    );
    assert_eq!(
        confirmed["capture"]["policy_digest"]["hex"],
        expected_policy
    );
    assert_eq!(confirmed["capture"]["policy_source"], "owner_policy_file");
    assert_eq!(
        confirmed["capture"]["omissions"],
        json!(["policy_excluded"])
    );
    until(
        "protected canonical handoff",
        Duration::from_secs(20),
        || (source.health().canonical_through >= 1).then_some(()),
    );
    batch_send(
        &mut adapter,
        &json!({"command":"verify_application","report_id":report["report_id"]}),
    );
    let verified = batch_reply(&replies);
    assert_eq!(
        verified["verified_application"]["capture"], confirmed["capture"],
        "{verified}"
    );
    clean(verified.to_string().as_bytes());
    let mut rows = Vec::new();
    read_lane(&source, 1, &mut rows).unwrap();
    assert_eq!(rows.len(), 1);
    clean(&rows[0].1);
    let response = canonical::exchange(
        &endpoint,
        READER,
        &Request::Lookup {
            origin: source.origin.clone(),
            identity: receipt.identity,
        },
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap();
    let Response::Found { row: Some(row) } = response else {
        panic!("canonical row missing")
    };
    clean(row.raw.as_bytes());
    assert_eq!(row.raw.as_bytes(), rows[0].1);
    adapter.kill_wait();
}
