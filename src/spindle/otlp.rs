//! The Spindle's loopback OTLP/HTTP trace endpoint
//! ([ADR-0025](../../docs/decisions/ADR-0025-carry-traces-as-a-third-signal.md)).
//!
//! A local application exports spans with `POST /v1/traces` and an
//! `application/x-protobuf` body. Only a confirmed durable Spool commit yields
//! `200`. Overload or unavailable confirmation yields `503` or a closed socket;
//! an enqueued export can still commit after its HTTP waiter times out.
//! This is a bounded subset of HTTP/1.1, not a general OTLP receiver. Loopback
//! does not identify a local user or prevent a hostile process reconnecting.

use crate::spindle::runtime::Spindle;
use fabric_frame::envelope::MAX_BATCH;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant};

mod deadline_io;
use deadline_io::{DeadlineStream, remaining};

/// The largest export accepted: the Batch cap less room for the log cursors every
/// Batch carries (16 cursors of at most a 4 KiB path each, with their fields).
pub const MAX_EXPORT: usize = MAX_BATCH - 96 * 1024;
/// Header bytes accepted before the body.
const MAX_HEAD: usize = 16 * 1024;
/// Connections served at once; more are closed at accept.
const MAX_CONNECTIONS: usize = 32;
/// Requests waiting for the main loop.
const QUEUE: usize = 64;
/// How long a connection waits for its request to be committed.
const COMMIT_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
struct TransportPolicy {
    header: Duration,
    body: Duration,
    commit: Duration,
    response: Duration,
    request: Duration,
    connection: Duration,
    requests: usize,
}

const TRANSPORT: TransportPolicy = TransportPolicy {
    header: Duration::from_secs(5),
    body: Duration::from_secs(10),
    commit: COMMIT_WAIT,
    response: Duration::from_secs(5),
    request: Duration::from_secs(50),
    connection: Duration::from_secs(120),
    requests: 128,
};

struct ConnectionPermit(Arc<AtomicUsize>);

impl ConnectionPermit {
    fn acquire(live: &Arc<AtomicUsize>) -> Option<Self> {
        live.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
            count.checked_add(1).filter(|next| *next <= MAX_CONNECTIONS)
        })
        .ok()?;
        Some(Self(Arc::clone(live)))
    }
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// What the main loop answers for one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Commit {
    /// Committed to the Spool at this Batch sequence.
    Committed(u64),
    /// The main loop did not return durable-success confirmation.
    Unavailable(String),
}

/// One validated export waiting for the main loop.
pub struct Export {
    pub body: Vec<u8>,
    pub reply: SyncSender<Commit>,
}

/// Validate a listen address: loopback only.
pub fn check_listen(addr: &str) -> io::Result<SocketAddr> {
    let parsed: SocketAddr = addr.parse().map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "invalid traces_listen address")
    })?;
    if !parsed.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "traces_listen must be a loopback address",
        ));
    }
    Ok(parsed)
}

/// Bind the endpoint and serve it on background threads; exports arrive on the
/// returned channel.
pub fn start(addr: SocketAddr) -> io::Result<(SocketAddr, Receiver<Export>)> {
    if !addr.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "traces_listen must be a loopback address",
        ));
    }
    let listener = TcpListener::bind(addr)?;
    let bound = listener.local_addr()?;
    let (tx, rx) = sync_channel(QUEUE);
    let live = Arc::new(AtomicUsize::new(0));
    std::thread::Builder::new()
        .name("fabric-otlp".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let accepted = Instant::now();
                let Some(permit) = ConnectionPermit::acquire(&live) else {
                    continue;
                };
                let tx = tx.clone();
                // On spawn failure the unstarted closure drops its permit.
                // On success the worker owns it through every serve exit.
                let _ = std::thread::Builder::new()
                    .name("fabric-otlp-conn".into())
                    .spawn(move || {
                        let _permit = permit;
                        let _ = serve(stream, &tx, accepted, TRANSPORT);
                    });
            }
        })?;
    Ok((bound, rx))
}

/// A parsed request head.
#[derive(Debug, PartialEq, Eq)]
pub struct Head {
    pub method: String,
    pub path: String,
    pub content_length: Option<usize>,
    pub content_type: Option<String>,
    pub close: bool,
}

/// Read one request head (request line and headers). `Ok(None)` at a clean end of
/// the connection.
pub fn read_head(reader: &mut impl BufRead) -> io::Result<Option<Head>> {
    let mut line = String::new();
    let mut total = 0;
    let mut lines = Vec::new();
    loop {
        line.clear();
        let n = reader
            .take((MAX_HEAD - total + 1) as u64)
            .read_line(&mut line)?;
        if n == 0 {
            if lines.is_empty() {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated request head",
            ));
        }
        total += n;
        if total > MAX_HEAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request head too large",
            ));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            if lines.is_empty() {
                continue; // tolerate a stray empty line between requests
            }
            break;
        }
        lines.push(trimmed.to_owned());
    }
    let mut parts = lines[0].split(' ');
    let (method, path, version) = (parts.next(), parts.next(), parts.next());
    let (Some(method), Some(path), Some(version)) = (method, path, version) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid request line",
        ));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported HTTP version",
        ));
    }
    let mut head = Head {
        method: method.to_owned(),
        path: path.to_owned(),
        content_length: None,
        content_type: None,
        close: version == "HTTP/1.0",
    };
    for header in &lines[1..] {
        let Some((name, value)) = header.split_once(':') else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid header"));
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => {
                let n = value.parse().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid content-length")
                })?;
                if head.content_length.replace(n).is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "duplicate content-length",
                    ));
                }
            }
            "content-type" => head.content_type = Some(value.to_ascii_lowercase()),
            "connection" => head.close = value.eq_ignore_ascii_case("close"),
            "transfer-encoding" => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "transfer-encoding is not supported",
                ));
            }
            _ => {}
        }
    }
    Ok(Some(head))
}

fn phase(budget: Duration, ceiling: Instant) -> Instant {
    Instant::now().checked_add(budget).unwrap_or(ceiling).min(ceiling)
}

fn respond(
    stream: &mut DeadlineStream,
    status: &str,
    body: &[u8],
    close: bool,
    deadline: Instant,
) -> io::Result<()> {
    stream.set_deadline(deadline);
    let head = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/x-protobuf\r\ncontent-length: {}\r\n{}\r\n",
        body.len(),
        if close { "connection: close\r\n" } else { "" }
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// Every blocking stage shares absolute request and connection ceilings. A
/// timed-out HTTP waiter does not cancel or invent the outcome of queued work.
fn serve(
    stream: TcpStream,
    tx: &SyncSender<Export>,
    accepted: Instant,
    policy: TransportPolicy,
) -> io::Result<()> {
    let connection_end = accepted.checked_add(policy.connection).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "OTLP connection deadline overflow")
    })?;
    let mut writer = DeadlineStream::new(stream.try_clone()?, connection_end);
    let mut reader = BufReader::new(DeadlineStream::new(stream, connection_end));
    for request_number in 0..policy.requests {
        let request_end = phase(policy.request, connection_end);
        let header_end = phase(policy.header, request_end);
        remaining(header_end)?;
        reader.get_mut().set_deadline(header_end);
        let head = match read_head(&mut reader) {
            Ok(Some(head)) => head,
            Ok(None) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::TimedOut => return Err(error),
            Err(_) => {
                return respond(&mut writer, "400 Bad Request", b"", true, phase(policy.response, request_end));
            }
        };
        // Buffered bytes can bypass the underlying socket Read implementation.
        remaining(header_end)?;
        let close = head.close || request_number + 1 == policy.requests;
        if head.method != "POST" || head.path.split('?').next() != Some("/v1/traces") {
            return respond(&mut writer, "404 Not Found", b"", true, phase(policy.response, request_end));
        }
        if head
            .content_type
            .as_deref()
            .is_none_or(|t| t.split(';').next().map(str::trim) != Some("application/x-protobuf"))
        {
            return respond(&mut writer, "415 Unsupported Media Type", b"", true, phase(policy.response, request_end));
        }
        let Some(length) = head.content_length else {
            return respond(&mut writer, "411 Length Required", b"", true, phase(policy.response, request_end));
        };
        if length > MAX_EXPORT {
            return respond(&mut writer, "413 Content Too Large", b"", true, phase(policy.response, request_end));
        }
        let body_end = phase(policy.body, request_end);
        remaining(body_end)?;
        reader.get_mut().set_deadline(body_end);
        let mut body = vec![0_u8; length];
        reader.read_exact(&mut body)?;
        remaining(body_end)?;
        if length == 0 || ExportTraceServiceRequest::decode(body.as_slice()).is_err() {
            return respond(&mut writer, "400 Bad Request", b"", true, phase(policy.response, request_end));
        }
        remaining(body_end)?;
        let (reply, answer) = sync_channel(1);
        match tx.try_send(Export { body, reply }) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                // This attempt never entered the consumer queue.
                return respond(&mut writer, "503 Service Unavailable", b"", true, phase(policy.response, request_end));
            }
        }
        let commit_end = phase(policy.commit, request_end);
        match answer.recv_timeout(remaining(commit_end)?) {
            Ok(Commit::Committed(_)) => {
                respond(&mut writer, "200 OK", b"", close, phase(policy.response, request_end))?;
            }
            Ok(Commit::Unavailable(_)) | Err(_) => {
                // A lost/expired reply leaves an admitted export's outcome
                // unknown. It may still commit; never report a false ACK.
                return respond(&mut writer, "503 Service Unavailable", b"", true, phase(policy.response, request_end));
            }
        }
        if close {
            return Ok(());
        }
    }
    Ok(())
}

/// Take waiting exports whose bodies fit one Batch together, oldest first. A body
/// that alone fills the Batch is taken alone.
pub fn take_batch(
    rx: &Receiver<Export>,
    carried: &mut Option<Export>,
    budget: usize,
) -> Vec<Export> {
    let mut out: Vec<Export> = Vec::new();
    let mut size = 0;
    loop {
        let next = match carried.take() {
            Some(export) => export,
            None => match rx.try_recv() {
                Ok(export) => export,
                Err(_) => break,
            },
        };
        if !out.is_empty() && size + next.body.len() > budget {
            *carried = Some(next);
            break;
        }
        size += next.body.len();
        out.push(next);
    }
    out
}

/// Commit every waiting export, a Batch at a time, and answer each exporter. While
/// the server has paused this node nothing is committed and exporters are told to
/// retry.
/// Returns whether any Batch was committed, so the caller can deliver it at once.
pub fn drain(node: &mut Spindle, rx: &Receiver<Export>, carried: &mut Option<Export>) -> bool {
    let mut committed = false;
    loop {
        let batch = take_batch(rx, carried, MAX_EXPORT);
        if batch.is_empty() {
            return committed;
        }
        let answer = if node.paused() {
            Commit::Unavailable("collection is paused by the server".into())
        } else {
            let bodies: Vec<&[u8]> = batch.iter().map(|e| e.body.as_slice()).collect();
            match node.commit_traces(&bodies) {
                Ok(sequence) => {
                    committed = true;
                    Commit::Committed(sequence)
                }
                Err(error) => Commit::Unavailable(error.to_string()),
            }
        };
        for export in batch {
            let _ = export.reply.send(answer.clone());
        }
    }
}

#[cfg(test)]
#[path = "otlp/security_tests.rs"]
mod security_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_addresses_are_accepted() {
        assert!(check_listen("127.0.0.1:4318").is_ok());
        assert!(check_listen("[::1]:4318").is_ok());
        assert!(check_listen("0.0.0.0:4318").is_err());
        assert!(check_listen("192.168.1.2:4318").is_err());
        assert!(
            check_listen("localhost:4318").is_err(),
            "a name is not an address"
        );
    }

    #[test]
    fn heads_parse_strictly() {
        let mut ok = "POST /v1/traces HTTP/1.1\r\nContent-Type: application/x-protobuf\r\nContent-Length: 12\r\n\r\n".as_bytes();
        let head = read_head(&mut ok).unwrap().unwrap();
        assert_eq!(
            (
                head.method.as_str(),
                head.path.as_str(),
                head.content_length
            ),
            ("POST", "/v1/traces", Some(12))
        );
        assert!(!head.close);
        for bad in [
            "POST /v1/traces HTTP/2\r\n\r\n",
            "POST /v1/traces HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n",
            "POST /v1/traces HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
            "POST /v1/traces HTTP/1.1\r\nno colon\r\n\r\n",
        ] {
            assert!(read_head(&mut bad.as_bytes()).is_err(), "{bad:?}");
        }
        let huge = format!("POST / HTTP/1.1\r\nx: {}\r\n\r\n", "a".repeat(MAX_HEAD));
        assert!(read_head(&mut huge.as_bytes()).is_err());
        assert!(read_head(&mut "".as_bytes()).unwrap().is_none());
    }

    #[test]
    fn batches_take_whole_exports_within_the_budget() {
        let (tx, rx) = sync_channel(8);
        for n in [400, 400, 400] {
            let (reply, _) = sync_channel(1);
            tx.send(Export {
                body: vec![0; n],
                reply,
            })
            .unwrap();
        }
        let mut carried = None;
        assert_eq!(take_batch(&rx, &mut carried, 1000).len(), 2);
        assert!(carried.is_some());
        assert_eq!(take_batch(&rx, &mut carried, 1000).len(), 1);
        assert!(take_batch(&rx, &mut carried, 1000).is_empty());
    }
}
