use super::{Reply, Request, custody::Receipt, wire::MAX_READ_PAGE};
use crate::observation::MAX_FRAME;
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    net::{
        AddressFamily, SocketAddrUnix, SocketFlags, SocketType, connect as connect_socket,
        socket_with,
    },
};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const MAX_POOLED_SOCKETS: usize = 16;

fn stream_pool() -> &'static Mutex<BTreeMap<PathBuf, UnixStream>> {
    static POOL: OnceLock<Mutex<BTreeMap<PathBuf, UnixStream>>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn take_pooled(path: &Path) -> Option<UnixStream> {
    stream_pool()
        .lock()
        .ok()
        .and_then(|mut map| map.remove(path))
}

fn return_pooled(path: &Path, stream: UnixStream) {
    if let Ok(mut map) = stream_pool().lock() {
        if map.len() >= MAX_POOLED_SOCKETS && !map.contains_key(path) {
            map.pop_first();
        }
        map.insert(path.to_path_buf(), stream);
    }
}

fn connect(path: &Path, deadline: Instant) -> Result<UnixStream, String> {
    let fd = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
        None,
    )
    .map_err(|e| e.to_string())?;
    let addr = SocketAddrUnix::new(path).map_err(|e| e.to_string())?;
    if let Err(e) = connect_socket(&fd, &addr)
        && e != rustix::io::Errno::INPROGRESS
        && e != rustix::io::Errno::AGAIN
    {
        return Err(e.to_string());
    }
    let mut fds = [PollFd::new(&fd, PollFlags::OUT)];
    let wait = deadline
        .checked_duration_since(Instant::now())
        .ok_or("deadline")?;
    let timeout = Timespec {
        tv_sec: wait.as_secs() as _,
        tv_nsec: wait.subsec_nanos() as _,
    };
    if poll(&mut fds, Some(&timeout)).map_err(|e| e.to_string())? == 0 {
        return Err("connect deadline; outcome unknown".into());
    }
    let stream = UnixStream::from(fd);
    stream.set_nonblocking(false).map_err(|e| e.to_string())?;
    Ok(stream)
}
fn remaining(deadline: Instant) -> Result<Duration, String> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or("deadline; outcome unknown".into())
}
fn write_all(s: &mut UnixStream, mut bytes: &[u8], deadline: Instant) -> Result<(), String> {
    while !bytes.is_empty() {
        s.set_write_timeout(Some(remaining(deadline)?))
            .map_err(|e| e.to_string())?;
        let n = s
            .write(bytes)
            .map_err(|e| format!("write; outcome unknown: {e}"))?;
        if n == 0 {
            return Err("closed; outcome unknown".into());
        }
        bytes = &bytes[n..]
    }
    Ok(())
}
fn read_exact(s: &mut UnixStream, mut bytes: &mut [u8], deadline: Instant) -> Result<(), String> {
    while !bytes.is_empty() {
        s.set_read_timeout(Some(remaining(deadline)?))
            .map_err(|e| e.to_string())?;
        let n = s
            .read(bytes)
            .map_err(|e| format!("read; outcome unknown: {e}"))?;
        if n == 0 {
            return Err("closed; outcome unknown".into());
        }
        bytes = &mut bytes[n..]
    }
    Ok(())
}
fn exchange_on_stream(
    stream: &mut UnixStream,
    bytes: &[u8],
    deadline: Instant,
) -> Result<Vec<u8>, String> {
    write_all(stream, &(bytes.len() as u32).to_be_bytes(), deadline)?;
    write_all(stream, bytes, deadline)?;
    let mut header = [0; 4];
    read_exact(stream, &mut header, deadline)?;
    let size = u32::from_be_bytes(header) as usize;
    if size > super::MAX_REPLY {
        return Err("reply too large".into());
    }
    let mut body = vec![0; size];
    read_exact(stream, &mut body, deadline)?;
    super::wire::check_reply(&body)?;
    Ok(body)
}
fn exchange_raw(path: &Path, bytes: &[u8], deadline: Instant) -> Result<Vec<u8>, String> {
    if bytes.len() > MAX_FRAME {
        return Err("frame too large".into());
    }
    let mut stream = connect(path, deadline)?;
    exchange_on_stream(&mut stream, bytes, deadline)
}
fn exchange_pooled_raw(path: &Path, bytes: &[u8], deadline: Instant) -> Result<Vec<u8>, String> {
    if bytes.len() > MAX_FRAME {
        return Err("frame too large".into());
    }
    if let Some(mut stream) = take_pooled(path)
        && let Ok(body) = exchange_on_stream(&mut stream, bytes, deadline)
    {
        return_pooled(path, stream);
        return Ok(body);
    }
    let mut stream = connect(path, deadline)?;
    let body = exchange_on_stream(&mut stream, bytes, deadline)?;
    return_pooled(path, stream);
    Ok(body)
}
fn exchange(path: &Path, bytes: &[u8], deadline: Instant) -> Result<Reply, String> {
    serde_json::from_slice(&exchange_raw(path, bytes, deadline)?).map_err(|e| e.to_string())
}
pub fn submit_bytes(path: &Path, bytes: &[u8], deadline: Instant) -> Result<Receipt, String> {
    // Submit frames are raw v2 observations; control frames have a command field.
    match exchange(path, bytes, deadline)? {
        Reply::Receipt { receipt } => Ok(receipt),
        Reply::Error { error } => Err(error),
        _ => Err("unexpected reply".into()),
    }
}
pub fn request(path: &Path, request: &Request, deadline: Instant) -> Result<Reply, String> {
    exchange(
        path,
        &serde_json::to_vec(request).map_err(|e| e.to_string())?,
        deadline,
    )
}

/// Reads one original payload without parsing and reserializing its JSON spelling.
pub fn read_raw(path: &Path, after: u64, deadline: Instant) -> Result<Vec<(u64, Vec<u8>)>, String> {
    read_raw_request(path, &Request::Read { after, limit: 1 }, deadline)
}

/// Reads a batched page (up to [`MAX_READ_PAGE`]) of original payloads over a pooled UDS.
pub fn read_raw_batch(
    path: &Path,
    after: u64,
    limit: usize,
    deadline: Instant,
) -> Result<Vec<(u64, Vec<u8>)>, String> {
    let req = Request::ReadBatch {
        expected_origin: None,
        after,
        limit: limit.min(MAX_READ_PAGE),
    };
    let bytes = exchange_pooled_raw(
        path,
        &serde_json::to_vec(&req).map_err(|e| e.to_string())?,
        deadline,
    )?;
    super::wire::decode_records_raw(&bytes)
}

/// Reads one original payload only from the expected local custody origin.
pub fn read_raw_for_origin(
    path: &Path,
    origin: &str,
    after: u64,
    deadline: Instant,
) -> Result<Vec<(u64, Vec<u8>)>, String> {
    read_raw_request(
        path,
        &Request::ReadForOrigin {
            expected_origin: origin.to_owned(),
            after,
            limit: 1,
        },
        deadline,
    )
}

/// Reads a batched page (up to [`MAX_READ_PAGE`]) for an expected custody origin over a pooled UDS.
pub fn read_raw_for_origin_batch(
    path: &Path,
    origin: &str,
    after: u64,
    limit: usize,
    deadline: Instant,
) -> Result<Vec<(u64, Vec<u8>)>, String> {
    let req = Request::ReadBatch {
        expected_origin: Some(origin.to_owned()),
        after,
        limit: limit.min(MAX_READ_PAGE),
    };
    let bytes = exchange_pooled_raw(
        path,
        &serde_json::to_vec(&req).map_err(|e| e.to_string())?,
        deadline,
    )?;
    super::wire::decode_records_raw(&bytes)
}

/// Returns the persistent origin of the local custody store.
pub fn read_origin(path: &Path, deadline: Instant) -> Result<String, String> {
    match request(path, &Request::FabricProgress, deadline)? {
        Reply::FabricProgress { origin, .. } => Ok(origin),
        Reply::Error { error } => Err(error),
        _ => Err("unexpected reply".into()),
    }
}

fn read_raw_request(
    path: &Path,
    request: &Request,
    deadline: Instant,
) -> Result<Vec<(u64, Vec<u8>)>, String> {
    let bytes = exchange_raw(
        path,
        &serde_json::to_vec(request).map_err(|e| e.to_string())?,
        deadline,
    )?;
    super::wire::decode_records_raw(&bytes)
}
