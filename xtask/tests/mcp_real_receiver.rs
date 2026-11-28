//! End-to-end qualification with an externally selected, hashed MCP executable.
use fabric::{
    canonical::{
        self, AcceptPlan, ForwardRecord, PlanProposal, PlanScope, Request, Response, SegmentTicket,
    },
    observation::{self, Data, Identity, ObservationV3},
    query::{API_VERSION, Budgets, Filter, Operation, QueryRequest},
};
use fabric_server::{Config, DecisionWriterGrant, NodeGrant, Server};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{atomic::Ordering, mpsc},
    thread,
    time::{Duration, Instant},
};

const READER: &str = "reader-credential-for-mcp-test";
const NODE: &str = "node-credential-for-mcp-test";
const WRITER: &str = "writer-credential-for-mcp-test";
fn credential(root: &Path, name: &str, value: &str) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, value).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}
fn direct(address: &str, secret: &str, request: &Request) -> Response {
    canonical::exchange(
        address,
        secret,
        request,
        Instant::now() + Duration::from_secs(15),
    )
    .unwrap()
}
struct Mcp {
    child: Child,
    input: ChildStdin,
    output: mpsc::Receiver<Value>,
    reader: Option<thread::JoinHandle<()>>,
}
impl Mcp {
    fn start(binary: &Path, address: &str, mode: &str, path: &Path) -> Self {
        let mut cmd = Command::new(binary);
        cmd.env("FABRIC_ENDPOINT", address)
            .env("FABRIC_MCP_MODE", mode)
            .env_remove("FABRIC_CREDENTIAL")
            .env_remove("FABRIC_CREDENTIAL_FILE")
            .env_remove("FABRIC_WRITER_CREDENTIAL_FILE")
            .env_remove("FABRIC_MCP_HTTP_SOCKET")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if mode == "writer" {
            cmd.env("FABRIC_WRITER_CREDENTIAL_FILE", path);
        } else {
            cmd.env("FABRIC_CREDENTIAL_FILE", path);
        }
        let mut child = cmd.spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, output) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut stream = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                let Ok(count) = stream.by_ref().take(1024 * 1024 + 1).read_line(&mut line) else {
                    break;
                };
                if count == 0 || count > 1024 * 1024 || !line.ends_with('\n') {
                    break;
                }
                if let Ok(value) = serde_json::from_str::<Value>(&line)
                    && tx.send(value).is_err()
                {
                    break;
                }
            }
        });
        let mut m = Self {
            child,
            input,
            output,
            reader: Some(reader),
        };
        m.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"real-receiver-test","version":"1"}}}));
        assert_eq!(m.receive()["id"], 1);
        m.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        m
    }
    fn send(&mut self, value: Value) {
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
    }
    fn receive(&self) -> Value {
        self.output.recv_timeout(Duration::from_secs(20)).unwrap()
    }
    fn call(&mut self, id: u32, name: &str, arguments: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}));
        let result = self.receive();
        assert_eq!(result["id"], id);
        assert_ne!(result["result"]["isError"], true, "MCP tool failed");
        let frame: Value =
            serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(frame["untrusted_telemetry"], true);
        frame["telemetry_payload"].clone()
    }
}
impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
struct LocalHttp {
    child: Child,
    socket: PathBuf,
}
impl LocalHttp {
    fn start(binary: &Path, address: &str, mode: &str, credential: &Path, socket: PathBuf) -> Self {
        let mut command = Command::new(binary);
        command
            .env("FABRIC_ENDPOINT", address)
            .env("FABRIC_MCP_MODE", mode)
            .env("FABRIC_MCP_HTTP_SOCKET", &socket)
            .env_remove("FABRIC_CREDENTIAL")
            .env_remove("FABRIC_CREDENTIAL_FILE")
            .env_remove("FABRIC_WRITER_CREDENTIAL_FILE")
            .env_remove("FABRIC_MCP_LIVE_SOCKET")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if mode == "writer" {
            command.env("FABRIC_WRITER_CREDENTIAL_FILE", credential);
        } else {
            command.env("FABRIC_CREDENTIAL_FILE", credential);
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !socket.exists() {
            assert!(
                child.try_wait().unwrap().is_none() && Instant::now() < deadline,
                "HTTP process did not start"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );
        Self { child, socket }
    }
    fn request(&self, path: &str, body: &Value, origin: bool) -> (String, Vec<u8>) {
        let body = serde_json::to_vec(body).unwrap();
        let mut stream = UnixStream::connect(&self.socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let extra = if origin { "Origin: null\r\n" } else { "" };
        write!(
            stream,
            "POST {path} HTTP/1.1\r\nHost: local\r\n{extra}Content-Length: {}\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(&body).unwrap();
        let mut bytes = Vec::new();
        stream
            .take(1024 * 1024 + 4097)
            .read_to_end(&mut bytes)
            .unwrap();
        assert!(bytes.len() <= 1024 * 1024 + 4096);
        let split = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let status = String::from_utf8(bytes[..split].to_vec()).unwrap();
        (status, bytes[split + 4..].to_vec())
    }
    fn call(&self, path: &str, request: Value) -> Value {
        let (status, bytes) = self.request(path, &json!({"version":1,"request":request}), false);
        assert!(status.starts_with("HTTP/1.1 200"), "HTTP request rejected");
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        assert_ne!(result["isError"], true, "HTTP tool failed");
        let frame: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(frame["untrusted_telemetry"], true);
        frame["telemetry_payload"].clone()
    }
}
impl Drop for LocalHttp {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(self.child.id().to_string())
            .status();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                _ if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
    }
}

fn hash(path: &Path) -> String {
    let output = Command::new("sha256sum").arg(path).output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

fn bounded_plan_output(
    stream: impl Read + Send + 'static,
) -> mpsc::Receiver<std::io::Result<Vec<u8>>> {
    let (send, receive) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stream
            .take(canonical::MAX_FRAME as u64 + 1)
            .read_to_end(&mut bytes)
            .and_then(|_| {
                if bytes.len() > canonical::MAX_FRAME {
                    Err(std::io::Error::other("plan CLI output exceeds MAX_FRAME"))
                } else {
                    Ok(bytes)
                }
            });
        let _ = send.send(result);
    });
    receive
}

fn plan_cli(binary: &Path, address: &str, credential: &Path, verb: &str, body: &Value) -> Response {
    let input = serde_json::to_vec(body).unwrap();
    // The controlled fixture writes at most Linux PIPE_BUF to an empty pipe.
    assert!(input.len() <= 4096);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut child = Command::new(binary)
        .arg(verb)
        .env("FABRIC_ENDPOINT", address)
        .env("FABRIC_CREDENTIAL_FILE", credential)
        .env_remove("FABRIC_CREDENTIAL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = bounded_plan_output(child.stdout.take().unwrap());
    let stderr = bounded_plan_output(child.stderr.take().unwrap());
    if let Err(e) = child.stdin.take().unwrap().write_all(&input) {
        let _ = child.kill();
        child.wait().unwrap();
        panic!("{verb} stdin: {e}");
    }
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            other => {
                let _ = child.kill();
                child.wait().unwrap();
                panic!("{verb} failed to exit within the deadline: {other:?}");
            }
        }
    };
    let receive = |output: mpsc::Receiver<std::io::Result<Vec<u8>>>| {
        output
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|e| panic!("{verb} output deadline: {e}"))
            .unwrap_or_else(|e| panic!("{verb} output bound: {e}"))
    };
    let stdout = receive(stdout);
    let stderr = receive(stderr);
    assert!(
        status.success(),
        "{verb}: {}",
        String::from_utf8_lossy(&stderr)
    );
    serde_json::from_slice(&stdout).unwrap()
}

#[test]
#[ignore = "run in the confined slice with a selected hashed fabric-mcp binary"]
fn stdio_reader_writer_match_live_receiver_and_verified_segment() {
    let binary =
        PathBuf::from(std::env::var_os("FABRIC_MCP_TEST_BIN").expect("FABRIC_MCP_TEST_BIN"));
    let expected_hash = std::env::var("FABRIC_MCP_TEST_SHA256").expect("FABRIC_MCP_TEST_SHA256");
    assert!(binary.is_absolute());
    assert_eq!(hash(&binary), expected_hash);
    let root = xtask::evidence::non_tmp_tempdir().unwrap();
    if std::env::var_os("FABRIC_MCP_HTTP_TEST").is_some() {
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    let reader_path = credential(root.path(), "reader", READER);
    let node_path = credential(root.path(), "node", NODE);
    let writer_path = credential(root.path(), "writer", WRITER);
    let config = Config {
        listen: "127.0.0.1:0".into(),
        storage: root.path().join("ledger"),
        projection: root.path().join("projection"),
        quota_bytes: 16 << 20,
        projection_quota_bytes: 4 << 20,
        reader_credential_file: reader_path.clone(),
        nodes: vec![NodeGrant {
            origin: "origin".into(),
            credential_file: node_path,
        }],
        decision_writers: vec![DecisionWriterGrant {
            writer: "owner".into(),
            scope: "plan".into(),
            origin: "origin".into(),
            producer: "worker".into(),
            credential_file: writer_path.clone(),
        }],
        node_quota_bytes: BTreeMap::new(),
        engine_endpoint: None,
        engine_auth_token_file: None,
        allow_unauthenticated_loopback_engine: false,
        max_connections: 8,
    };
    let server = Server::open(config.clone()).unwrap();
    let address = server.address();
    let stop = server.stop_handle();
    let task = thread::spawn(move || server.serve().unwrap());
    let authority = match direct(&address, READER, &Request::Progress) {
        Response::Progress { authority, .. } => authority,
        _ => panic!("progress failed"),
    };
    let carrier = PlanProposal {
        version: 1,
        scope: PlanScope("plan".into()),
        proposal_id: "a".repeat(64),
    };
    let mut fields = serde_json::Map::new();
    fields.insert(
        canonical::PROPOSAL_MARKER.into(),
        serde_json::to_string(&carrier).unwrap().into(),
    );
    let raw = serde_json::to_string(&ObservationV3 {
        version: observation::VERSION_V3,
        registry: observation::REGISTRY_V3.into(),
        context: fabric::run::RunContext {
            run_id: "aaaa0000-0000-0000-0000-000000000001".into(),
        },
        identity: Identity {
            producer: "worker".into(),
            generation: "generation".into(),
            sequence: 1,
        },
        timestamp_ns: 7,
        resource: "local".into(),
        data: Data::JsonLog { fields },
    })
    .unwrap();
    assert!(matches!(
        direct(
            &address,
            NODE,
            &Request::Ingest {
                origin: "origin".into(),
                records: vec![ForwardRecord {
                    source_id: 1,
                    raw: raw.clone()
                }]
            }
        ),
        Response::Committed { through: 1, .. }
    ));
    let query = QueryRequest {
        version: API_VERSION,
        query_id: "mcp-live-query".into(),
        authority: Some(authority.clone()),
        scope: vec!["origin".into()],
        snapshot: BTreeMap::new(),
        filter: Filter::default(),
        operation: Operation::Evidence,
        budgets: Budgets::default(),
        cursor: None,
    };
    let oracle = match direct(
        &address,
        READER,
        &Request::Query {
            request: query.clone(),
        },
    ) {
        Response::Query { response } => response,
        _ => panic!("direct query failed"),
    };
    assert!(!oracle.rows.is_empty());
    let mut reader = Mcp::start(&binary, &address, "reader", &reader_path);
    let actual = reader.call(2, "query", serde_json::to_value(&query).unwrap());
    assert_eq!(
        actual["response"]["rows"],
        serde_json::to_value(&oracle.rows).unwrap()
    );
    assert_eq!(
        actual["response"]["coverage"]["snapshot"],
        serde_json::to_value(&oracle.coverage).unwrap()["snapshot"]
    );
    assert_eq!(
        actual["response"]["coverage"]["canonical_frontier"],
        serde_json::to_value(&oracle.coverage).unwrap()["canonical_frontier"]
    );
    let manifest: Value =
        serde_json::from_slice(&fs::read(config.projection.join("manifest.json")).unwrap())
            .unwrap();
    let segment = &manifest["manifest"]["segments"][0];
    let digest = segment["digest"].as_str().unwrap();
    let expected = fs::read(config.projection.join(segment["file"].as_str().unwrap())).unwrap();
    let len = expected.len().min(128);
    let payload = reader.call(3, "inspect_segment", json!({"authority":authority,"origin":"origin","digest":digest,"start":"0","end":len.to_string()}));
    let ticket: SegmentTicket = serde_json::from_value(payload["ticket"].clone()).unwrap();
    let result = direct(
        &address,
        READER,
        &Request::ReadSegmentChunk {
            ticket,
            offset: 0,
            length: len as u32,
        },
    );
    assert!(
        matches!(result, Response::SegmentChunk { offset: 0, bytes } if bytes == expected[..len])
    );
    let http_enabled = std::env::var_os("FABRIC_MCP_HTTP_TEST").is_some();
    if http_enabled {
        let http_reader = LocalHttp::start(
            &binary,
            &address,
            "reader",
            &reader_path,
            root.path().join("http-reader.sock"),
        );
        let observed = http_reader.call("/v1/query", serde_json::to_value(&query).unwrap());
        assert_eq!(
            observed["response"]["rows"],
            serde_json::to_value(&oracle.rows).unwrap()
        );
        assert_eq!(
            observed["response"]["coverage"]["snapshot"],
            serde_json::to_value(&oracle.coverage).unwrap()["snapshot"]
        );
        assert_eq!(
            observed["response"]["coverage"]["canonical_frontier"],
            serde_json::to_value(&oracle.coverage).unwrap()["canonical_frontier"]
        );
        let access = http_reader.call("/v1/segment-ticket", json!({"authority":authority,"origin":"origin","digest":digest,"start":"0","end":len.to_string()}));
        let ticket: SegmentTicket = serde_json::from_value(access["ticket"].clone()).unwrap();
        assert_eq!(ticket.digest, digest);
        assert!(
            matches!(direct(&address, READER, &Request::ReadSegmentChunk { ticket, offset: 0, length: len as u32 }), Response::SegmentChunk { offset: 0, bytes } if bytes == expected[..len])
        );
        let (status, _) =
            http_reader.request("/v1/accept-plan", &json!({"version":1,"request":{}}), false);
        assert!(status.starts_with("HTTP/1.1 403"));
        let (status, _) =
            http_reader.request("/v1/query", &json!({"version":1,"request":query}), true);
        assert!(status.starts_with("HTTP/1.1 403"));
    }
    let query_probe = std::env::var_os("FABRIC_PLAN_QUERY_TEST_BIN").map(|query_bin| {
        let query_bin = PathBuf::from(query_bin);
        assert!(query_bin.is_absolute());
        let digest = std::env::var("FABRIC_PLAN_QUERY_TEST_SHA256").unwrap();
        assert_eq!(hash(&query_bin), digest);
        let Response::PlanSubscribed {
            subscription,
            frontier,
        } = plan_cli(
            &query_bin,
            &address,
            &writer_path,
            "subscribe-plan",
            &json!({"scope":"plan"}),
        )
        else {
            panic!("typed CLI subscription failed");
        };
        assert_eq!(frontier, 0);
        (
            query_bin,
            digest,
            json!({"scope":"plan","subscription":subscription}),
        )
    });
    let mut writer = Mcp::start(&binary, &address, "writer", &writer_path);
    let arguments = json!({"authority":authority,"command_id":"stable-id","scope":"plan","expected_head":null,"origin":"origin","source_id":"1","proposal_raw":raw});
    let result = if http_enabled {
        let http_writer = LocalHttp::start(
            &binary,
            &address,
            "writer",
            &writer_path,
            root.path().join("http-writer.sock"),
        );
        let (status, _) =
            http_writer.request("/v1/query", &json!({"version":1,"request":{}}), false);
        assert!(status.starts_with("HTTP/1.1 403"));
        http_writer.call("/v1/accept-plan", arguments.clone())
    } else {
        writer.call(2, "accept_plan", arguments.clone())
    };
    let receipt = result["decision"].as_str().unwrap();
    let body = serde_json::to_string(&AcceptPlan {
        version: 1,
        authority: authority.clone(),
        command_id: "stable-id".into(),
        scope: PlanScope("plan".into()),
        expected_head: None,
        origin: "origin".into(),
        source_id: 1,
        proposal_raw: raw.clone(),
    })
    .unwrap();
    assert!(
        matches!(direct(&address, WRITER, &Request::AcceptPlan { body }), Response::PlanAccepted { decision } if decision == receipt)
    );
    if http_enabled {
        let repeated = writer.call(2, "accept_plan", arguments);
        assert_eq!(repeated["decision"].as_str().unwrap(), receipt);
    }
    drop(writer);
    drop(reader);
    let page_input = json!({"scope":"plan","cursor":null,"limit":1,"ttl_secs":30});
    let after_commit = query_probe.map(|(query_bin, digest, poll)| {
        for expected in [true, false] {
            assert!(matches!(
                plan_cli(&query_bin, &address, &writer_path, "poll-plan-hint", &poll),
                Response::PlanHint { pending } if pending == expected
            ));
        }
        let page = plan_cli(&query_bin, &address, &writer_path, "plan-page", &page_input);
        let Response::PlanPage { page: ref read } = page else {
            panic!("typed CLI page failed");
        };
        assert_eq!(read.scope, PlanScope("plan".into()));
        assert_eq!(read.frontier, 1);
        assert!(read.next_cursor.is_none());
        assert_eq!(read.decisions.len(), 1);
        assert_eq!(read.decisions[0].command_id, "stable-id");
        (query_bin, digest, poll, serde_json::to_value(page).unwrap())
    });
    stop.store(true, Ordering::SeqCst);
    task.join().unwrap();
    if let Some((query_bin, digest, poll, expected)) = after_commit {
        let server = Server::open(config.clone()).unwrap();
        let address = server.address();
        let stop = server.stop_handle();
        let task = thread::spawn(move || server.serve().unwrap());
        assert!(matches!(
            plan_cli(&query_bin, &address, &writer_path, "poll-plan-hint", &poll),
            Response::PlanCursorFailure {
                reason: canonical::PlanCursorFailure::Revoked
            }
        ));
        let got = plan_cli(&query_bin, &address, &writer_path, "plan-page", &page_input);
        assert_eq!(serde_json::to_value(got).unwrap(), expected);
        stop.store(true, Ordering::SeqCst);
        task.join().unwrap();
        assert_eq!(hash(&query_bin), digest);
        println!("verified scoped CLI plan page and advisory hint restart readback");
    }
    // Exercise a separate MCP subscription after the CLI parity checks, so no
    // client consumes another client's advisory notification.
    let server = Server::open(config.clone()).unwrap();
    let address = server.address();
    let stop = server.stop_handle();
    let task = thread::spawn(move || server.serve().unwrap());
    let mut writer = Mcp::start(&binary, &address, "writer", &writer_path);
    let mut reader = Mcp::start(&binary, &address, "reader", &reader_path);
    reader.send(json!({"jsonrpc":"2.0","id":90,"method":"tools/call","params":{"name":"plan_page","arguments":{"authority":authority,"scope":"plan","cursor":null,"limit":1,"ttl_secs":30}}}));
    let denied = reader.receive();
    assert!(denied.get("error").is_some() || denied["result"]["isError"] == true);
    drop(reader);
    assert!(matches!(
        direct(
            &address,
            READER,
            &Request::PlanPage {
                request: canonical::PlanPageRequest {
                    scope: PlanScope("plan".into()),
                    cursor: None,
                    limit: 1,
                    ttl_secs: 30,
                }
            }
        ),
        Response::Error { .. }
    ));
    writer.send(json!({"jsonrpc":"2.0","id":91,"method":"tools/call","params":{"name":"subscribe_plan","arguments":{"scope":"other"}}}));
    let denied = writer.receive();
    assert!(denied.get("error").is_some() || denied["result"]["isError"] == true);
    let subscribed = writer.call(92, "subscribe_plan", json!({"scope":"plan"}));
    let subscription = subscribed["subscription"].as_str().unwrap().to_owned();
    assert_eq!(subscribed["frontier"], 1);
    let next = |command: &str, head: i64| json!({"authority":authority,"command_id":command,"scope":"plan","expected_head":head,"origin":"origin","source_id":"1","proposal_raw":raw});
    let second = writer.call(93, "accept_plan", next("cursor-second", 1));
    let second_receipt: canonical::PlanDecision =
        serde_json::from_str(second["decision"].as_str().unwrap()).unwrap();
    assert_eq!(second_receipt.head, 2);
    let hint = writer.call(
        94,
        "poll_plan_hint",
        json!({"scope":"plan","subscription":subscription}),
    );
    assert_eq!(hint["pending"], true);
    let hint = writer.call(
        95,
        "poll_plan_hint",
        json!({"scope":"plan","subscription":subscription}),
    );
    assert_eq!(hint["pending"], false);
    let args = json!({"authority":authority,"scope":"plan","cursor":null,"limit":1,"ttl_secs":900});
    let invalid = writer.call(
        101,
        "plan_page",
        json!({"authority":authority,"scope":"plan","cursor":"invalid","limit":1,"ttl_secs":900}),
    );
    assert_eq!(invalid["result"], "plan_cursor_failure");
    assert_eq!(invalid["reason"], "invalid");
    let first = writer.call(96, "plan_page", args.clone());
    assert_eq!(first["page"]["frontier"], 2);
    assert_eq!(first["page"]["decisions"][0]["command_id"], "stable-id");
    let direct_page = direct(
        &address,
        WRITER,
        &Request::PlanPage {
            request: canonical::PlanPageRequest {
                scope: PlanScope("plan".into()),
                cursor: None,
                limit: 1,
                ttl_secs: 900,
            },
        },
    );
    let Response::PlanPage { page: direct_page } = direct_page else {
        panic!("direct page failed")
    };
    assert_eq!(
        serde_json::to_value(&direct_page.decisions).unwrap(),
        first["page"]["decisions"]
    );
    assert_eq!(direct_page.authority, authority);
    assert_eq!(direct_page.frontier, 2);
    let cursor = first["page"]["next_cursor"].as_str().unwrap().to_owned();
    let third = writer.call(97, "accept_plan", next("cursor-third", 2));
    let third_receipt: canonical::PlanDecision =
        serde_json::from_str(third["decision"].as_str().unwrap()).unwrap();
    assert_eq!(third_receipt.head, 3);
    drop(writer);
    stop.store(true, Ordering::SeqCst);
    task.join().unwrap();
    let server = Server::open(config).unwrap();
    let address = server.address();
    let stop = server.stop_handle();
    let task = thread::spawn(move || server.serve().unwrap());
    let mut writer = Mcp::start(&binary, &address, "writer", &writer_path);
    let revoked = writer.call(
        98,
        "poll_plan_hint",
        json!({"scope":"plan","subscription":subscription}),
    );
    assert_eq!(revoked["result"], "plan_cursor_failure");
    assert_eq!(revoked["reason"], "revoked");
    let continued = writer.call(
        99,
        "plan_page",
        json!({"authority":authority,"scope":"plan","cursor":cursor,"limit":1,"ttl_secs":900}),
    );
    assert_eq!(continued["page"]["frontier"], 2);
    assert_eq!(
        continued["page"]["decisions"][0]["command_id"],
        "cursor-second"
    );
    assert!(continued["page"]["next_cursor"].is_null());
    let fresh = writer.call(
        100,
        "plan_page",
        json!({"authority":authority,"scope":"plan","cursor":null,"limit":64,"ttl_secs":900}),
    );
    assert_eq!(fresh["page"]["frontier"], 3);
    assert_eq!(fresh["page"]["decisions"].as_array().unwrap().len(), 3);
    drop(writer);
    stop.store(true, Ordering::SeqCst);
    task.join().unwrap();
    assert_eq!(hash(&binary), expected_hash);
}
