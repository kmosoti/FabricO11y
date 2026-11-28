#![forbid(unsafe_code)]
//! Same-user Unix socket behind an independently authenticated owner proxy.
use fabric_client::FabricQueryClient;
use fabric_ui_gateway_core::{Error, MAX_INPUT_BYTES, QueryPolicy};
use http_body_util::{BodyExt, Full};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header,
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::{UnixListener, UnixStream},
    sync::Semaphore,
    task::JoinSet,
    time::{Instant, timeout_at},
};

const DEADLINE: Duration = Duration::from_secs(15);
const LIFETIME: Duration = Duration::from_secs(900);
struct Config {
    policy: QueryPolicy,
    client: FabricQueryClient,
    origin: String,
    host: String,
    expires: Instant,
}
fn response(status: StatusCode, body: impl Into<Bytes>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::CONNECTION, "close")
        .body(Full::new(body.into()))
        .expect("static headers")
}
fn rejected(status: StatusCode) -> Response<Full<Bytes>> {
    response(status, "{\"error\":\"request rejected\"}")
}
fn exactly(headers: &hyper::HeaderMap, key: header::HeaderName, value: &str) -> bool {
    let mut values = headers.get_all(key).iter();
    values
        .next()
        .is_some_and(|v| v.as_bytes() == value.as_bytes())
        && values.next().is_none()
}
fn length(headers: &hyper::HeaderMap) -> Result<usize, StatusCode> {
    if headers.contains_key(header::TRANSFER_ENCODING) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut values = headers.get_all(header::CONTENT_LENGTH).iter();
    let raw = values
        .next()
        .ok_or(StatusCode::LENGTH_REQUIRED)?
        .to_str()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if values.next().is_some() || raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let n: usize = raw.parse().map_err(|_| StatusCode::BAD_REQUEST)?;
    if n > MAX_INPUT_BYTES {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    Ok(n)
}
async fn handle(
    req: Request<Incoming>,
    config: Arc<Config>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let deny = if Instant::now() >= config.expires {
        Some(StatusCode::SERVICE_UNAVAILABLE)
    } else if req.method() != Method::POST {
        Some(StatusCode::METHOD_NOT_ALLOWED)
    } else if req.uri().path() != "/api/v1/query" || req.uri().query().is_some() {
        Some(StatusCode::NOT_FOUND)
    } else if !exactly(req.headers(), header::HOST, &config.host)
        || !exactly(req.headers(), header::ORIGIN, &config.origin)
    {
        Some(StatusCode::FORBIDDEN)
    } else if !exactly(req.headers(), header::CONTENT_TYPE, "application/json") {
        Some(StatusCode::UNSUPPORTED_MEDIA_TYPE)
    } else {
        None
    };
    if let Some(status) = deny {
        return Ok(rejected(status));
    }
    let n = match length(req.headers()) {
        Ok(n) => n,
        Err(status) => return Ok(rejected(status)),
    };
    let mut body = req.into_body();
    let mut bytes = Vec::with_capacity(n);
    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else {
            return Ok(rejected(StatusCode::BAD_REQUEST));
        };
        let Ok(chunk) = frame.into_data() else {
            return Ok(rejected(StatusCode::BAD_REQUEST));
        };
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|len| len > n)
        {
            return Ok(rejected(StatusCode::BAD_REQUEST));
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() != n {
        return Ok(rejected(StatusCode::BAD_REQUEST));
    }
    let result = config
        .policy
        .authorize_and_query(&config.client, &bytes)
        .await;
    Ok(match result {
        Ok(bytes) => response(StatusCode::OK, bytes),
        Err(Error::InvalidRequest) => rejected(StatusCode::BAD_REQUEST),
        Err(Error::Forbidden) => rejected(StatusCode::FORBIDDEN),
        Err(_) => rejected(StatusCode::BAD_GATEWAY),
    })
}
fn validate_socket(path: &Path, uid: u32) -> Result<(), Box<dyn std::error::Error>> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path.starts_with("/tmp")
        || path.starts_with("/dev/shm")
    {
        return Err("invalid socket path".into());
    }
    let parent = path.parent().ok_or("invalid parent")?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != uid
        || metadata.mode() & 0o777 != 0o700
        || fs::canonicalize(parent)? != parent
    {
        return Err("socket parent must be a private owned directory".into());
    }
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err("socket path occupied".into()),
    }
}
struct SocketGuard {
    path: PathBuf,
    identity: (u64, u64),
}
impl Drop for SocketGuard {
    fn drop(&mut self) {
        if let Ok(m) = fs::symlink_metadata(&self.path)
            && (m.dev(), m.ino()) == self.identity
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}
async fn serve(path: PathBuf, config: Arc<Config>) -> Result<(), Box<dyn std::error::Error>> {
    let (a, _) = UnixStream::pair()?;
    let uid = a.peer_cred()?.uid();
    validate_socket(&path, uid)?;
    let listener = UnixListener::bind(&path)?;
    let m = fs::symlink_metadata(&path)?;
    let _guard = SocketGuard {
        path: path.clone(),
        identity: (m.dev(), m.ino()),
    };
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let slots = Arc::new(Semaphore::new(4));
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(config.expires) => break,
            _ = tokio::signal::ctrl_c() => break,
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
            accepted = listener.accept() => {
                let (socket, _) = accepted?;
                if socket.peer_cred().map(|c| c.uid() != uid).unwrap_or(true) { continue }
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue };
                let cfg = config.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    let deadline = (Instant::now() + DEADLINE).min(cfg.expires);
                    let service = service_fn(move |req| handle(req, cfg.clone()));
                    let mut builder = http1::Builder::new();
                    builder.keep_alive(false).max_buf_size(32 * 1024).max_headers(32);
                    let _ = timeout_at(deadline, builder.serve_connection(TokioIo::new(socket), service)).await;
                });
            }
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // No credential values are accepted via argv. The proxy owns TLS and holder authentication.
    let mut args = std::env::args_os().skip(1);
    let socket = PathBuf::from(args.next().ok_or("socket path required")?);
    let endpoint = args
        .next()
        .ok_or("backend endpoint required")?
        .into_string()
        .map_err(|_| "invalid endpoint")?;
    let credential = PathBuf::from(args.next().ok_or("backend credential file required")?);
    let authority = args
        .next()
        .ok_or("authority required")?
        .into_string()
        .map_err(|_| "invalid authority")?;
    let host = args
        .next()
        .ok_or("external host required")?
        .into_string()
        .map_err(|_| "invalid host")?;
    let origin = args
        .next()
        .ok_or("external origin required")?
        .into_string()
        .map_err(|_| "invalid origin")?;
    let sources = args
        .map(|s| s.into_string().map_err(|_| "invalid source"))
        .collect::<Result<Vec<_>, _>>()?;
    if host.is_empty() || host.contains(['\r', '\n', '/']) || origin != format!("https://{host}") {
        return Err("origin must be HTTPS and match host".into());
    }
    let addr: std::net::SocketAddr = endpoint.parse()?;
    if !addr.ip().is_loopback() {
        return Err("backend must be loopback".into());
    }
    let policy = QueryPolicy::new(authority, sources).map_err(|_| "invalid policy")?;
    let client = FabricQueryClient::with_bin(None, endpoint, credential);
    serve(
        socket,
        Arc::new(Config {
            policy,
            client,
            origin,
            host,
            expires: Instant::now() + LIFETIME,
        }),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unix_http_queries_real_canonical_server_and_expires() {
        use fabric::canonical::{
            self, ForwardRecord, Request as CanonicalRequest, Response as CanonicalResponse,
        };
        use fabric_server::{Config as ServerConfig, NodeGrant, Server};
        use std::sync::atomic::{AtomicBool, Ordering};

        struct ServerGuard {
            stop: Arc<AtomicBool>,
            worker: Option<std::thread::JoinHandle<Result<(), String>>>,
        }
        impl Drop for ServerGuard {
            fn drop(&mut self) {
                self.stop.store(true, Ordering::SeqCst);
                if let Some(worker) = self.worker.take() {
                    worker
                        .join()
                        .expect("canonical thread panicked")
                        .expect("canonical server failed");
                }
            }
        }
        let root = tempfile::tempdir_in(std::env::var_os("TMPDIR").expect("disk TMPDIR")).unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let reader = root.path().join("reader");
        let node = root.path().join("node");
        let wrong = root.path().join("wrong");
        for (path, contents) in [
            (&reader, "synthetic-reader-secret"),
            (&node, "synthetic-node-secret"),
            (&wrong, "synthetic-wrong-secret"),
        ] {
            fs::write(path, contents).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let server = Server::open(ServerConfig {
            listen: "127.0.0.1:0".into(),
            storage: root.path().join("canonical"),
            projection: root.path().join("projection"),
            quota_bytes: 32 << 20,
            projection_quota_bytes: 32 << 20,
            reader_credential_file: reader.clone(),
            nodes: vec![NodeGrant {
                origin: "alpha".into(),
                credential_file: node,
            }],
            decision_writers: vec![],
            node_quota_bytes: Default::default(),
            engine_endpoint: None,
            engine_auth_token_file: None,
            allow_unauthenticated_loopback_engine: false,
            max_connections: 16,
        })
        .unwrap();
        let endpoint = server.address();
        let stop = server.stop_handle();
        let _server = ServerGuard {
            stop,
            worker: Some(std::thread::spawn(move || server.serve())),
        };
        let call = |token: &str, request: CanonicalRequest| {
            canonical::exchange(
                &endpoint,
                token,
                &request,
                std::time::Instant::now() + Duration::from_secs(5),
            )
            .unwrap()
        };
        let CanonicalResponse::Progress { authority, .. } =
            call("synthetic-reader-secret", CanonicalRequest::Progress)
        else {
            panic!("missing canonical authority")
        };
        let raw = r#"{"version":2,"registry":"fabrico11y/2","identity":{"producer":"fixture","generation":"generation","sequence":1},"timestamp_ns":1,"resource":"svc","data":{"kind":"tool_execution","tool":"test","run_id":"same-local-name","duration_ns":5,"status":"ok"}}"#;
        assert!(matches!(
            call(
                "synthetic-node-secret",
                CanonicalRequest::Ingest {
                    origin: "alpha".into(),
                    records: vec![ForwardRecord {
                        source_id: 1,
                        raw: raw.into()
                    }]
                }
            ),
            CanonicalResponse::Committed { through: 1, .. }
        ));

        async fn request(
            path: &Path,
            origin: Option<&str>,
            body: &serde_json::Value,
        ) -> (u16, String) {
            let bytes = serde_json::to_vec(body).unwrap();
            let origin = origin
                .map(|value| format!("Origin: {value}\r\n"))
                .unwrap_or_default();
            let header = format!(
                "POST /api/v1/query HTTP/1.1\r\nHost: example.test\r\n{origin}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                bytes.len()
            );
            let mut stream = UnixStream::connect(path).await.unwrap();
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(&bytes).await.unwrap();
            let mut out = Vec::new();
            tokio::time::timeout(Duration::from_secs(8), stream.read_to_end(&mut out))
                .await
                .unwrap()
                .unwrap();
            let text = String::from_utf8(out).unwrap();
            let (head, body) = text.split_once("\r\n\r\n").expect("HTTP response");
            let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
            (status, body.to_owned())
        }
        async fn start(path: PathBuf, config: Arc<Config>) -> tokio::task::JoinHandle<()> {
            let server_path = path.clone();
            let task = tokio::spawn(async move {
                serve(server_path, config).await.expect("gateway failed");
            });
            let deadline = Instant::now() + Duration::from_secs(3);
            while !path.exists() {
                assert!(Instant::now() < deadline);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            task
        }
        let socket = root.path().join("gateway.sock");
        let task = start(
            socket.clone(),
            Arc::new(Config {
                policy: QueryPolicy::new(authority.clone(), vec!["alpha".into()]).unwrap(),
                client: FabricQueryClient::with_bin(None, endpoint.clone(), reader),
                origin: "https://example.test".into(),
                host: "example.test".into(),
                expires: Instant::now() + Duration::from_secs(10),
            }),
        )
        .await;
        let mut q: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../fixtures/request.json")).unwrap();
        q["authority"] = authority.clone().into();
        q["budgets"]["timeout_ms"] = 5000.into();
        let (status, _) = request(&socket, None, &q).await;
        assert_eq!(status, 403);
        let mut foreign = q.clone();
        foreign["scope"] = serde_json::json!(["other"]);
        let (status, _) = request(&socket, Some("https://example.test"), &foreign).await;
        assert_eq!(status, 403);
        let (status, body) = request(&socket, Some("https://example.test"), &q).await;
        assert_eq!(status, 200, "{body}");
        let list: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(list["coverage"]["canonical_authority"], authority);
        assert_eq!(list["coverage"]["scope"], serde_json::json!(["alpha"]));
        assert_eq!(list["rows"].as_array().unwrap().len(), 1);
        let mut summary = q.clone();
        summary["operation"] = serde_json::json!({"operation":"run_summary"});
        summary["filter"]["run"] = list["rows"][0]["run"].clone();
        summary["snapshot"] = list["coverage"]["snapshot"].clone();
        let (status, body) = request(&socket, Some("https://example.test"), &summary).await;
        assert_eq!(status, 200, "{body}");
        let details: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            details["coverage"]["snapshot"],
            list["coverage"]["snapshot"]
        );
        assert_eq!(details["rows"][0]["run"], list["rows"][0]["run"]);
        task.abort();
        let _ = task.await;

        let bad_socket = root.path().join("bad.sock");
        let bad_task = start(
            bad_socket.clone(),
            Arc::new(Config {
                policy: QueryPolicy::new(authority, vec!["alpha".into()]).unwrap(),
                client: FabricQueryClient::with_bin(None, endpoint, wrong),
                origin: "https://example.test".into(),
                host: "example.test".into(),
                expires: Instant::now() + Duration::from_secs(3),
            }),
        )
        .await;
        let (status, body) = request(&bad_socket, Some("https://example.test"), &q).await;
        assert_eq!(status, 502);
        assert!(
            !body.contains("synthetic-wrong-secret") && !body.contains("synthetic-reader-secret")
        );
        tokio::time::timeout(Duration::from_secs(5), bad_task)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !bad_socket.exists(),
            "expired listener must release its socket"
        );
    }

    #[tokio::test]
    async fn unix_http_rejects_untrusted_requests_before_backend() {
        let root = tempfile::tempdir_in(std::env::var_os("TMPDIR").expect("disk TMPDIR")).unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.path().join("gateway.sock");
        let config = Arc::new(Config {
            policy: QueryPolicy::new("auth-1".into(), vec!["alpha".into()]).unwrap(),
            client: FabricQueryClient::with_bin(None, "127.0.0.1:1", root.path().join("absent")),
            origin: "https://example.test".into(),
            host: "example.test".into(),
            expires: Instant::now() + Duration::from_secs(5),
        });
        let server_path = path.clone();
        let task = tokio::spawn(async move {
            let _ = serve(server_path, config).await;
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !path.exists() {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        async fn send(path: &Path, input: &str) -> String {
            let mut stream = UnixStream::connect(path).await.unwrap();
            stream.write_all(input.as_bytes()).await.unwrap();
            let mut out = Vec::new();
            tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut out))
                .await
                .unwrap()
                .unwrap();
            String::from_utf8(out).unwrap()
        }
        let missing = "POST /api/v1/query HTTP/1.1\r\nHost: example.test\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
        assert!(send(&path, missing).await.starts_with("HTTP/1.1 403"));
        let wrong = missing.replace(
            "Host: example.test",
            "Host: example.test\r\nOrigin: https://wrong.test",
        );
        assert!(send(&path, &wrong).await.starts_with("HTTP/1.1 403"));
        let invalid = "POST /api/v1/query HTTP/1.1\r\nHost: example.test\r\nOrigin: https://example.test\r\nContent-Type: application/json\r\nContent-Length: xyz\r\n\r\n{}";
        assert!(send(&path, invalid).await.starts_with("HTTP/1.1 400"));
        let body = String::from_utf8(include_bytes!("../../fixtures/request.json").to_vec())
            .unwrap()
            .replace("alpha", "other");
        let scoped = format!(
            "POST /api/v1/query HTTP/1.1\r\nHost: example.test\r\nOrigin: https://example.test\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        assert!(send(&path, &scoped).await.starts_with("HTTP/1.1 403"));
        let role_body = String::from_utf8(include_bytes!("../../fixtures/request.json").to_vec())
            .unwrap()
            .replace("list_runs", "evidence");
        let role = format!(
            "POST /api/v1/query HTTP/1.1\r\nHost: example.test\r\nOrigin: https://example.test\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            role_body.len(),
            role_body
        );
        assert!(send(&path, &role).await.starts_with("HTTP/1.1 403"));
        task.abort();
        let _ = task.await;
    }
}
