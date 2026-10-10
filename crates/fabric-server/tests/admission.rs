//! Origin: per-request body limits did not bound aggregate pre-admission bodies.
//! Actual HTTP/1 on loopback; no TLS claim. No native payload is submitted.
use fabric_server::{
    control::{Control, DesiredConfig},
    http::{self, AppState},
    query::History,
    store::{CommitMode, Store},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const ADMIN: &str = "admission-admin-0123456789abcdef0123456789abcdef";
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = PathBuf::from(
            std::env::var_os("FABRIC_SCRATCH_ROOT").expect("contained data-drive scratch"),
        );
        let path = root.join(format!(
            "admission-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn persist(&self, value: &Value) {
        let mut file = fs::File::create(self.0.join("trace.json")).unwrap();
        file.write_all(serde_json::to_string_pretty(value).unwrap().as_bytes())
            .unwrap();
        file.sync_all().unwrap();
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn connect(addr: SocketAddr) -> io::Result<TcpStream> {
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(1))?;
    stream.set_read_timeout(Some(Duration::from_millis(600)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    Ok(stream)
}
fn partial(addr: SocketAddr, path: &str, token: &str) -> io::Result<TcpStream> {
    let mut stream = connect(addr)?;
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: 1024\r\nConnection: close\r\n\r\n{{"
    )?;
    Ok(stream)
}
fn admitted_partial(addr: SocketAddr, path: &str, token: &str) -> io::Result<TcpStream> {
    let mut stream = connect(addr)?;
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: 1024\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n"
    )?;
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= 8192 {
            return Err(io::Error::other("interim header bound"));
        }
        let mut byte = [0];
        stream.read_exact(&mut byte)?;
        head.push(byte[0]);
    }
    if head != b"HTTP/1.1 100 Continue\r\n\r\n" {
        return Err(io::Error::other(format!(
            "expected admission/body-poll witness, received {}",
            String::from_utf8_lossy(&head)
        )));
    }
    stream.write_all(b"{")?;
    Ok(stream)
}
fn response(stream: &mut TcpStream) -> io::Result<Value> {
    response_before(stream, Instant::now() + Duration::from_millis(600))
}
fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "response deadline"))
}
fn response_before(stream: &mut TcpStream, deadline: Instant) -> io::Result<Value> {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() >= 8192 {
            return Err(io::Error::other("response header bound"));
        }
        let mut byte = [0];
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        stream.read_exact(&mut byte)?;
        head.push(byte[0]);
    }
    let text = String::from_utf8(head).map_err(io::Error::other)?;
    let status = text
        .lines()
        .next()
        .and_then(|s| s.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| io::Error::other("missing status"))?;
    let length = text
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .ok_or_else(|| io::Error::other("missing content length"))?;
    if length > 8192 {
        return Err(io::Error::other("response body bound"));
    }
    let mut body = vec![0; length];
    for byte in &mut body {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        stream.read_exact(std::slice::from_mut(byte))?;
    }
    let body: Value = serde_json::from_slice(&body).map_err(io::Error::other)?;
    let retry_after = text
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
        .map(|(_, v)| v.trim().to_owned());
    Ok(json!({"status":status,"retry_after":retry_after,"body":body}))
}
fn observed(result: io::Result<Value>) -> Value {
    match result {
        Ok(value) => value,
        Err(error) => json!({"io_error":error.to_string(),"kind":format!("{:?}",error.kind())}),
    }
}
fn full(addr: SocketAddr, path: &str, body: &[u8], token: &str) -> io::Result<Value> {
    full_before(
        addr,
        path,
        body,
        token,
        Instant::now() + Duration::from_secs(1),
    )
}
fn full_before(
    addr: SocketAddr,
    path: &str,
    body: &[u8],
    token: &str,
    deadline: Instant,
) -> io::Result<Value> {
    let mut stream = TcpStream::connect_timeout(&addr, remaining(deadline)?)?;
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    response_before(&mut stream, deadline)
}
fn cancel(streams: Vec<TcpStream>) {
    for stream in streams {
        let _ = stream.shutdown(Shutdown::Both);
    }
}
fn recovered(addr: SocketAddr, path: &str, body: &[u8], token: &str, expected: u16) -> Value {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut attempts = Vec::new();
    loop {
        let value = observed(full_before(addr, path, body, token, deadline));
        let finished = value["status"] == expected;
        attempts.push(value);
        if finished || Instant::now() >= deadline {
            return json!({"status":if finished { Some(expected) } else { None }, "attempts":attempts});
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn schedule(addr: SocketAddr, node_token: &str) -> io::Result<Value> {
    let query = br#"{"kind":"logs","from_ns":0,"to_ns":18446744073709551615,"limit":1}"#;
    let mut trace = json!({"origin":"missing aggregate prebody admission", "partial_body_bytes":1,
        "declared_body_bytes":1024,"batch_slots":16,"query_slots":2});
    let mut batches = Vec::new();
    for _ in 0..16 {
        batches.push(admitted_partial(addr, "/v1/batches", node_token)?);
    }
    trace["batch_interim_continue_witnesses"] = json!(16);
    let mut excess = partial(addr, "/v1/batches", node_token)?;
    trace["batch_excess_before_body_complete"] = observed(response(&mut excess));
    let _ = excess.shutdown(Shutdown::Both);
    trace["query_while_batch_pool_full"] = observed(full(addr, "/v1/admin/query", query, ADMIN));
    trace["invalid_credential_while_batch_pool_full"] =
        observed(full(addr, "/v1/batches", b"{}", ADMIN));
    cancel(batches);
    trace["batch_after_partial_cancellation"] =
        recovered(addr, "/v1/batches", b"{}", node_token, 400);
    let mut queries = Vec::new();
    for _ in 0..2 {
        queries.push(admitted_partial(addr, "/v1/admin/query", ADMIN)?);
    }
    trace["query_interim_continue_witnesses"] = json!(2);
    let mut excess = partial(addr, "/v1/admin/query", ADMIN)?;
    trace["query_excess_before_body_complete"] = observed(response(&mut excess));
    let _ = excess.shutdown(Shutdown::Both);
    trace["batch_while_query_pool_full"] = observed(full(addr, "/v1/batches", b"{}", node_token));
    cancel(queries);
    trace["query_after_partial_cancellation"] =
        recovered(addr, "/v1/admin/query", query, ADMIN, 200);
    Ok(trace)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_bodies_are_rejected_before_buffering_and_cancellation_recovers_independent_pools()
{
    let scratch = Scratch::new();
    let state_dir = scratch.0.join("state");
    let store = Store::open_with(&state_dir, 64 * 1024, 16 * 1024, CommitMode::GROUPED).unwrap();
    let (intake, commit) = store.spawn_joinable().unwrap();
    let mut control = Control::open(&state_dir).unwrap();
    let (_, node_token) = control
        .enroll(
            "admission-node",
            DesiredConfig {
                metric_interval_s: 1,
                ..Default::default()
            },
        )
        .unwrap();
    let app = http::router(AppState {
        intake,
        control: Arc::new(Mutex::new(control)),
        admin_token_sha256: Sha256::digest(ADMIN.as_bytes()).into(),
        history: Arc::new(History::new(&state_dir)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown, stopping) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = stopping.await;
            })
            .await
    });
    let result = tokio::task::spawn_blocking(move || schedule(addr, &node_token))
        .await
        .unwrap();
    let _ = shutdown.send(());
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .expect("HTTP cleanup deadline")
        .unwrap()
        .unwrap();
    tokio::task::spawn_blocking(move || commit.join().unwrap())
        .await
        .unwrap();
    let trace = result.unwrap_or_else(|error| json!({"schedule_error":error.to_string()}));
    scratch.persist(&trace);
    println!("{}", json!({"admission_trace":trace,"scratch":scratch.0}));
    // Authenticated malformed input reaches body decoding only after admission.
    // An unknown credential must be rejected before using the saturated pool.
    assert_eq!(
        trace["invalid_credential_while_batch_pool_full"]["status"], 401,
        "{trace}"
    );
    for name in [
        "batch_after_partial_cancellation",
        "batch_while_query_pool_full",
    ] {
        assert_eq!(trace[name]["status"], 400, "{name}: {trace}");
    }
    for name in [
        "query_while_batch_pool_full",
        "query_after_partial_cancellation",
    ] {
        assert_eq!(trace[name]["status"], 200, "{name}: {trace}");
    }
    for name in [
        "batch_excess_before_body_complete",
        "query_excess_before_body_complete",
    ] {
        assert_eq!(
            trace[name]["status"], 503,
            "uncompleted excess body must receive overload response: {trace}"
        );
        assert_eq!(trace[name]["retry_after"], "1", "{name}: {trace}");
    }
    drop(Store::open_with(&state_dir, 64 * 1024, 16 * 1024, CommitMode::GROUPED).unwrap());
}
