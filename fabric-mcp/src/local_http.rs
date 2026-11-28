//! Optional one-request-per-connection HTTP/1 adapter on an owned Unix socket.
use super::{AcceptPlanInput, Investigation, PlanWriter, QueryRequest, SegmentInspectionInput};
use http_body_util::{BodyExt, Full};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header,
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use rmcp::handler::server::wrapper::Parameters;
use serde::Deserialize;
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::{UnixListener, UnixStream},
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

const INPUT_LIMIT: usize = 196_608;
const OUTPUT_LIMIT: usize = 1024 * 1024;
const CONNECTIONS: usize = 8;
const DEADLINE: Duration = Duration::from_secs(40);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    version: u32,
    request: T,
}
#[derive(Clone)]
enum Role {
    Reader(Investigation),
    Writer(PlanWriter),
}
#[derive(Clone, Copy)]
enum Operation {
    Query,
    Segment,
    Accept,
}
fn choose(method: &Method, path: &str, writer: bool) -> Result<Operation, StatusCode> {
    if *method != Method::POST {
        return Err(StatusCode::METHOD_NOT_ALLOWED);
    }
    match (path, writer) {
        ("/v1/query", false) => Ok(Operation::Query),
        ("/v1/segment-ticket", false) => Ok(Operation::Segment),
        ("/v1/accept-plan", true) => Ok(Operation::Accept),
        ("/v1/query" | "/v1/segment-ticket" | "/v1/accept-plan", _) => Err(StatusCode::FORBIDDEN),
        _ => Err(StatusCode::NOT_FOUND),
    }
}
fn input_length(headers: &hyper::HeaderMap) -> Result<usize, StatusCode> {
    if headers.contains_key(header::ORIGIN) {
        return Err(StatusCode::FORBIDDEN);
    }
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
    if n > INPUT_LIMIT {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    Ok(n)
}
fn response(status: StatusCode, bytes: impl Into<Bytes>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONNECTION, "close")
        .body(Full::new(bytes.into()))
        .expect("static response")
}
fn error(status: StatusCode) -> Response<Full<Bytes>> {
    response(status, "{\"error\":\"request rejected\"}")
}
fn parse<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, StatusCode> {
    fabric::observation::check_json_depth(body, INPUT_LIMIT, 32)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let envelope: Envelope<T> =
        serde_json::from_slice(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    if envelope.version != 1 {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(envelope.request)
}
async fn handle(
    request: Request<Incoming>,
    role: Role,
    work: Arc<Semaphore>,
    held: Arc<Mutex<Option<OwnedSemaphorePermit>>>,
    cancel: CancellationToken,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let writer = matches!(role, Role::Writer(_));
    let op = match choose(request.method(), request.uri().path(), writer) {
        Ok(v) => v,
        Err(e) => return Ok(error(e)),
    };
    if request.uri().query().is_some() {
        return Ok(error(StatusCode::BAD_REQUEST));
    }
    let length = match input_length(request.headers()) {
        Ok(v) => v,
        Err(e) => return Ok(error(e)),
    };
    let mut body = request.into_body();
    let mut bytes = Vec::with_capacity(length);
    while let Some(frame) = body.frame().await {
        let Ok(frame) = frame else {
            return Ok(error(StatusCode::BAD_REQUEST));
        };
        let Ok(data) = frame.into_data() else {
            return Ok(error(StatusCode::BAD_REQUEST));
        };
        if bytes
            .len()
            .checked_add(data.len())
            .is_none_or(|n| n > length || n > INPUT_LIMIT)
        {
            return Ok(error(StatusCode::PAYLOAD_TOO_LARGE));
        }
        bytes.extend_from_slice(&data);
    }
    if bytes.len() != length {
        return Ok(error(StatusCode::BAD_REQUEST));
    }
    // Decode the complete typed request before acquiring work or contacting the backend.
    enum Decoded {
        Query(QueryRequest),
        Segment(SegmentInspectionInput),
        Accept(AcceptPlanInput),
    }
    let decoded = match op {
        Operation::Query => parse(&bytes).map(Decoded::Query),
        Operation::Segment => parse(&bytes).map(Decoded::Segment),
        Operation::Accept => parse(&bytes).map(Decoded::Accept),
    };
    let decoded = match decoded {
        Ok(v) => v,
        Err(e) => return Ok(error(e)),
    };
    let permit = match work.try_acquire_owned() {
        Ok(v) => v,
        Err(_) => return Ok(error(StatusCode::SERVICE_UNAVAILABLE)),
    };
    *held.lock().expect("HTTP lease lock") = Some(permit);
    let result = match (role, decoded) {
        (Role::Reader(reader), Decoded::Query(request)) => reader.run_query(request, cancel).await,
        (Role::Reader(reader), Decoded::Segment(request)) => {
            reader.inspect_segment(Parameters(request)).await
        }
        (Role::Writer(writer), Decoded::Accept(request)) => {
            writer.accept_plan(Parameters(request)).await
        }
        _ => unreachable!("role was checked before parsing"),
    };
    let result = match result {
        Ok(result) => result,
        Err(_) => return Ok(error(StatusCode::BAD_REQUEST)),
    };
    let bytes = match serde_json::to_vec(&result) {
        Ok(bytes) if bytes.len() <= OUTPUT_LIMIT => bytes,
        _ => return Ok(error(StatusCode::INTERNAL_SERVER_ERROR)),
    };
    Ok(response(StatusCode::OK, bytes))
}
fn validate_path(path: &Path, uid: u32) -> Result<(), Box<dyn std::error::Error>> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path.starts_with("/tmp")
        || path.starts_with("/dev/shm")
    {
        return Err("invalid HTTP socket path".into());
    }
    let parent = path.parent().ok_or("invalid HTTP socket parent")?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != uid
        || metadata.mode() & 0o777 != 0o700
        || fs::canonicalize(parent)? != parent
    {
        return Err("HTTP socket parent must be owned, real and mode 0700".into());
    }
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err("HTTP socket path is occupied".into()),
        Err(e) => Err(e.into()),
    }
}
struct SocketGuard {
    path: PathBuf,
    identity: (u64, u64),
}
impl Drop for SocketGuard {
    fn drop(&mut self) {
        if let Ok(current) = fs::symlink_metadata(&self.path)
            && (current.dev(), current.ino()) == self.identity
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}
pub(super) async fn serve(
    path: PathBuf,
    writer: bool,
    endpoint: String,
    credential: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let (a, _) = UnixStream::pair()?;
    let uid = a.peer_cred()?.uid();
    validate_path(&path, uid)?;
    let listener = UnixListener::bind(&path)?;
    let metadata = fs::symlink_metadata(&path)?;
    let _socket_guard = SocketGuard {
        path: path.clone(),
        identity: (metadata.dev(), metadata.ino()),
    };
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let role = if writer {
        Role::Writer(PlanWriter::new(endpoint, credential))
    } else {
        Role::Reader(Investigation::new(endpoint, credential))
    };
    let connections = Arc::new(Semaphore::new(CONNECTIONS));
    let work = Arc::new(Semaphore::new(1));
    let mut tasks = JoinSet::new();
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = terminate.recv() => break,
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
            accepted = listener.accept() => {
                let (socket, _) = accepted?;
                if socket.peer_cred().map(|c| c.uid() != uid).unwrap_or(true) { continue; }
                let Ok(connection) = connections.clone().try_acquire_owned() else { continue; };
                let role = role.clone(); let work = work.clone();
                tasks.spawn(async move {
                    let _connection = connection;
                    let held = Arc::new(Mutex::new(None));
                    let token = CancellationToken::new();
                    let cancel_on_exit = token.clone().drop_guard();
                    let service = service_fn(move |request| handle(request, role.clone(), work.clone(), held.clone(), token.clone()));
                    let mut builder = http1::Builder::new();
                    builder.keep_alive(false).max_buf_size(32 * 1024).max_headers(32);
                    let _ = tokio::time::timeout(DEADLINE, builder.serve_connection(TokioIo::new(socket), service)).await;
                    drop(cancel_on_exit);
                });
            }
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_http_route_and_body_gates() {
        assert!(matches!(
            choose(&Method::POST, "/v1/query", false),
            Ok(Operation::Query)
        ));
        assert!(matches!(
            choose(&Method::POST, "/v1/accept-plan", false),
            Err(StatusCode::FORBIDDEN)
        ));
        assert!(matches!(
            choose(&Method::POST, "/v1/query", true),
            Err(StatusCode::FORBIDDEN)
        ));
        assert!(matches!(
            choose(&Method::GET, "/v1/query", false),
            Err(StatusCode::METHOD_NOT_ALLOWED)
        ));
        assert!(matches!(
            choose(&Method::POST, "/v1/unknown", false),
            Err(StatusCode::NOT_FOUND)
        ));
        let mut headers = hyper::HeaderMap::new();
        headers.insert(header::CONTENT_LENGTH, "4".parse().unwrap());
        assert_eq!(input_length(&headers), Ok(4));
        headers.append(header::CONTENT_LENGTH, "4".parse().unwrap());
        assert_eq!(input_length(&headers), Err(StatusCode::BAD_REQUEST));
        headers.remove(header::CONTENT_LENGTH);
        headers.insert(
            header::CONTENT_LENGTH,
            (INPUT_LIMIT + 1).to_string().parse().unwrap(),
        );
        assert_eq!(input_length(&headers), Err(StatusCode::PAYLOAD_TOO_LARGE));
        headers.insert(header::CONTENT_LENGTH, "4".parse().unwrap());
        headers.insert(header::TRANSFER_ENCODING, "chunked".parse().unwrap());
        assert_eq!(input_length(&headers), Err(StatusCode::BAD_REQUEST));
        headers.remove(header::TRANSFER_ENCODING);
        headers.insert(header::ORIGIN, "null".parse().unwrap());
        assert_eq!(input_length(&headers), Err(StatusCode::FORBIDDEN));
        assert_eq!(parse::<u32>(br#"{"version":1,"request":7}"#), Ok(7));
        assert!(parse::<u32>(br#"{"version":1,"request":7} {}"#).is_err());
        assert!(parse::<u32>(br#"{"version":1,"request":7,"extra":1}"#).is_err());
        assert!(parse::<u32>(br#"{"version":2,"request":7}"#).is_err());
    }
    #[tokio::test]
    async fn private_real_parent_and_occupied_path_are_required() {
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(std::env::var_os("TMPDIR").unwrap())
            .unwrap();
        let path = root.path().join("api.sock");
        let (a, _) = UnixStream::pair().unwrap();
        let uid = a.peer_cred().unwrap().uid();
        assert!(validate_path(&path, uid).is_ok());
        fs::write(&path, b"sentinel").unwrap();
        assert!(validate_path(&path, uid).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"sentinel");
        fs::remove_file(&path).unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(validate_path(&path, uid).is_err());
    }
}
