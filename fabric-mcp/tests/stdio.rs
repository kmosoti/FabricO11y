use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};
#[test]
fn official_stdio_lifecycle_lists_only_read_only_typed_tools() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let credential = directory.path().join("reader.key");
    std::fs::write(&credential, "owned-test-credential").unwrap();
    std::fs::set_permissions(&credential, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_fabric-mcp"))
        .env("FABRIC_ENDPOINT", "127.0.0.1:9")
        .env_remove("FABRIC_CREDENTIAL")
        .env("FABRIC_CREDENTIAL_FILE", &credential)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else { break };
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && tx.send(value).is_err()
            {
                break;
            }
        }
    });
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"frozen-protocol-test","version":"1"}}})).unwrap();
    input.flush().unwrap();
    let initialized = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(initialized["id"], 1);
    assert!(initialized["result"]["capabilities"]["tools"].is_object());
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})
    )
    .unwrap();
    input.flush().unwrap();
    let listed = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 6);
    for tool in tools {
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["annotations"]["destructiveHint"], false);
    }
    let query = tools.iter().find(|t| t["name"] == "query").unwrap();
    let schema = &query["inputSchema"];
    assert!(schema["properties"]["scope"].is_object());
    assert!(schema["properties"]["query_id"].is_object());
    assert!(!serde_json::to_string(schema).unwrap().contains("sql"));
    let _ = child.kill();
    let _ = child.wait();
    drop(input);
    reader.join().unwrap();
}

#[test]
fn overlong_unterminated_input_is_rejected_before_json_decode() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fabric-mcp"))
        .env("FABRIC_ENDPOINT", "127.0.0.1:9")
        .env("FABRIC_CREDENTIAL", "owned-test-credential")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        let _ = input.write_all(&vec![b'x'; 2 * 1024 * 1024]);
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut exited = false;
    while std::time::Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            exited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !exited {
        let _ = child.kill();
    }
    let _ = child.wait();
    writer.join().unwrap();
    assert!(
        exited,
        "bounded codec must terminate an overlong unterminated frame"
    );
}

#[test]
fn status_queue_is_bounded_and_query_cancellation_survives_status_pressure() {
    use fabric::canonical::{self, Authenticated, Request, Response};
    use std::{
        net::TcpListener,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Instant,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let release = Arc::new(AtomicBool::new(false));
    let release_status = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let (tx_events, events) = mpsc::channel();
    let release_server = release.clone();
    let release_status_server = release_status.clone();
    let stop_server = stop.clone();
    let server = std::thread::spawn(move || {
        let mut workers = Vec::new();
        while !stop_server.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let release = release_server.clone();
                    let release_status = release_status_server.clone();
                    let tx = tx_events.clone();
                    workers.push(std::thread::spawn(move || {
                        let deadline = Instant::now() + Duration::from_secs(8);
                        let bytes = canonical::read_frame(&mut stream, deadline).unwrap();
                        let auth: Authenticated = serde_json::from_slice(&bytes).unwrap();
                        let response = match auth.request {
                            Request::Query { .. } => {
                                tx.send("query").unwrap();
                                while !release.load(Ordering::SeqCst) && Instant::now() < deadline {
                                    std::thread::sleep(Duration::from_millis(5));
                                }
                                Response::Error {
                                    error: "cancelled after actual cleanup".into(),
                                }
                            }
                            Request::Cancel { query_id } => {
                                tx.send("cancel").unwrap();
                                Response::Cancelled { query_id }
                            }
                            Request::Capabilities => {
                                tx.send("capabilities").unwrap();
                                while !release_status.load(Ordering::SeqCst)
                                    && Instant::now() < deadline
                                {
                                    std::thread::sleep(Duration::from_millis(5));
                                }
                                Response::Capabilities {
                                    authority: "status-fixture".into(),
                                    schemas: vec![2, 3],
                                    origins: vec![],
                                    max_frame: 1024 * 1024,
                                    max_batch_records: 128,
                                }
                            }
                            Request::Progress => Response::Progress {
                                authority: "test".into(),
                                frontiers: Default::default(),
                            },
                            _ => Response::Error {
                                error: "unexpected request".into(),
                            },
                        };
                        let _ = canonical::write_frame(
                            &mut stream,
                            &serde_json::to_vec(&response).unwrap(),
                            deadline,
                        );
                    }));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("{e}"),
            }
        }
        for worker in workers {
            worker.join().unwrap();
        }
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_fabric-mcp"))
        .env("FABRIC_ENDPOINT", address.to_string())
        .env("FABRIC_CREDENTIAL", "owned-test-credential")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else { break };
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && tx.send(value).is_err()
            {
                break;
            }
        }
    });
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"cancellation-test","version":"1"}}})).unwrap();
    input.flush().unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap()["id"], 1);
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    let args = json!({"version":1,"query_id":"cancel-proof","scope":["a"],"snapshot":{},"filter":{},"operation":{"operation":"evidence"},"cursor":null});
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"query","arguments":args}})).unwrap();
    input.flush().unwrap();
    assert_eq!(
        events.recv_timeout(Duration::from_secs(5)).unwrap(),
        "query"
    );
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"capabilities","arguments":{}}})).unwrap();
    input.flush().unwrap();
    assert_eq!(
        events.recv_timeout(Duration::from_secs(2)).unwrap(),
        "capabilities"
    );
    for id in 20..36 {
        writeln!(input,"{}",json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"progress","arguments":{}}})).unwrap();
    }
    input.flush().unwrap();
    let overflow = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let overflow_id = overflow["id"].as_u64().unwrap();
    assert!((20..36).contains(&overflow_id));
    assert_eq!(overflow["error"]["message"], "MCP status queue full");

    writeln!(input,"{}",json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3,"reason":"test cancellation"}})).unwrap();
    input.flush().unwrap();
    assert_eq!(
        events.recv_timeout(Duration::from_secs(5)).unwrap(),
        "cancel"
    );
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"query","arguments":args}})).unwrap();
    input.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let busy = loop {
        let value = rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        if value["id"] == 4 {
            break value;
        }
    };
    assert!(busy.to_string().contains("MCP worker busy"));
    release_status.store(true, Ordering::SeqCst);

    let mut expected = (20..36)
        .filter(|id| *id != overflow_id)
        .collect::<std::collections::BTreeSet<_>>();
    expected.insert(10);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !expected.is_empty() {
        let value = rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        let Some(id) = value["id"].as_u64() else {
            continue;
        };
        if expected.remove(&id) {
            assert!(
                value.get("error").is_none(),
                "status call {id} failed: {value}"
            );
            assert_eq!(value["result"]["isError"].as_bool(), Some(false));
        }
    }
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"progress","arguments":{}}})).unwrap();
    input.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let progress = loop {
        let value = rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        if value["id"] == 5 {
            break value;
        }
    };
    assert!(progress.to_string().contains("test"));
    assert!(!progress.to_string().contains("MCP worker busy"));
    release.store(true, Ordering::SeqCst);
    let _ = child.kill();
    let _ = child.wait();
    drop(input);
    reader.join().unwrap();
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
}
