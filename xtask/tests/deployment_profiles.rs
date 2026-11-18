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
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
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
        budgets: Budgets::default(),
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
