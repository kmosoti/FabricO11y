#![cfg(feature = "local-http")]
//! Real Unix HTTP transport with an authenticated canonical-protocol mock.
use fabric::canonical::{self, Request, Response};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

fn http(path: &std::path::Path, request: &[u8]) -> String {
    let mut stream = UnixStream::connect(path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(request).unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).unwrap();
    out
}
#[test]
fn unix_http_authenticates_peer_and_gates_before_backend() {
    const READER: &str = "reader-credential-for-http-test";
    let root = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(std::env::var_os("TMPDIR").unwrap())
        .unwrap();
    let socket = root.path().join("api.sock");
    let credential = root.path().join("reader.key");
    fs::write(&credential, READER).unwrap();
    fs::set_permissions(&credential, fs::Permissions::from_mode(0o600)).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let active = stop.clone();
    let seen = calls.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_mock = cancelled.clone();
    let (event_tx, events) = std::sync::mpsc::channel();
    let mock = thread::spawn(move || {
        let mut workers = Vec::new();
        while !active.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let seen = seen.clone();
                    let cancelled = cancelled_mock.clone();
                    let event_tx = event_tx.clone();
                    workers.push(thread::spawn(move || {
                        let deadline = Instant::now() + Duration::from_secs(10);
                        let (_, request) =
                            canonical::accept_authenticated_sync(&mut stream, [READER], deadline)
                                .unwrap();
                        let response = match request {
                            Request::Query { request } => {
                                seen.fetch_add(1, Ordering::SeqCst);
                                if request.query_id == "http-disconnect" {
                                    event_tx.send("started").unwrap();
                                    while !cancelled.load(Ordering::SeqCst)
                                        && Instant::now() < deadline
                                    {
                                        thread::sleep(Duration::from_millis(5));
                                    }
                                }
                                Response::Error {
                                    error: "mock query complete".into(),
                                }
                            }
                            Request::Cancel { query_id } if query_id == "http-disconnect" => {
                                cancelled.store(true, Ordering::SeqCst);
                                event_tx.send("cancelled").unwrap();
                                Response::Cancelled { query_id }
                            }
                            _ => panic!("unexpected canonical request"),
                        };
                        let _ = canonical::write_frame(
                            &mut stream,
                            &serde_json::to_vec(&response).unwrap(),
                            deadline,
                        );
                    }));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
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
        .env("FABRIC_MCP_MODE", "reader")
        .env("FABRIC_MCP_HTTP_SOCKET", &socket)
        .env("FABRIC_CREDENTIAL_FILE", &credential)
        .env_remove("FABRIC_CREDENTIAL")
        .env_remove("FABRIC_MCP_LIVE_SOCKET")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(Instant::now() < deadline, "socket did not start");
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
        0o600
    );
    for request in [
        b"GET /v1/query HTTP/1.1\r\nHost: local\r\nContent-Length: 0\r\n\r\n".as_slice(),
        b"POST /v1/accept-plan HTTP/1.1\r\nHost: local\r\nContent-Length: 0\r\n\r\n",
        b"POST /v1/query HTTP/1.1\r\nHost: local\r\nOrigin: null\r\nContent-Length: 0\r\n\r\n",
        b"POST /v1/query HTTP/1.1\r\nHost: local\r\nContent-Length: 196609\r\n\r\n",
        b"POST /v1/query HTTP/1.1\r\nHost: local\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        b"POST /v1/query HTTP/1.1\r\nHost: local\r\nContent-Length: 2\r\n\r\n{}",
        b"POST /v1/query HTTP/1.1\r\nHost: local\r\nContent-Length: 4\r\n\r\n{}{}",
    ] {
        assert!(!http(&socket, request).starts_with("HTTP/1.1 200"));
    }
    let body = r#"{"version":1,"request":{"version":1,"query_id":"http-query","scope":["origin"],"snapshot":{},"filter":{},"operation":{"operation":"evidence"},"cursor":null}}"#;
    let request = format!(
        "POST /v1/query HTTP/1.1\r\nHost: local\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let reply = http(&socket, request.as_bytes());
    assert!(reply.starts_with("HTTP/1.1 200"));
    assert!(reply.contains("mock query complete"));
    assert!(!reply.contains(READER));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let body = body.replace("http-query", "http-disconnect");
    let request = format!(
        "POST /v1/query HTTP/1.1\r\nHost: local\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let mut connection = UnixStream::connect(&socket).unwrap();
    connection.write_all(request.as_bytes()).unwrap();
    assert_eq!(
        events.recv_timeout(Duration::from_secs(5)).unwrap(),
        "started"
    );
    drop(connection);
    assert_eq!(
        events.recv_timeout(Duration::from_secs(5)).unwrap(),
        "cancelled"
    );
    assert!(cancelled.load(Ordering::SeqCst));
    assert!(
        Command::new("kill")
            .arg("-TERM")
            .arg(child.id().to_string())
            .status()
            .unwrap()
            .success()
    );
    assert!(child.wait().unwrap().success());
    assert!(!socket.exists());
    stop.store(true, Ordering::SeqCst);
    mock.join().unwrap();
}
