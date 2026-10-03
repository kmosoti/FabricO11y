//! The Spindle's loopback OTLP/HTTP trace endpoint
//! ([ADR-0025](../../docs/decisions/ADR-0025-carry-traces-as-a-third-signal.md)).
//!
//! A local application exports spans with `POST /v1/traces` and an
//! `application/x-protobuf` body, as every OpenTelemetry SDK can. The receiver checks
//! that the body is an `ExportTraceServiceRequest` of at most 1 MiB and hands it to the
//! Spindle's main loop, which commits it to the Spool; only then does the exporter get
//! `200`. A full Spool answers `503` (the exporter retries), a malformed body `400`, an
//! oversized one `413`. This is not a general OTLP receiver: one path, protobuf only,
//! loopback only, a small subset of HTTP/1.1 (Content-Length bodies, keep-alive), on
//! plain blocking threads.

use crate::spindle::runtime::Spindle;
use fabric_frame::envelope::MAX_BATCH;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::Duration;

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

/// What the main loop answers for one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Commit {
    /// Committed to the Spool at this Batch sequence.
    Committed(u64),
    /// Not committed (the Spool is full or failed); the exporter should retry.
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
    let listener = TcpListener::bind(addr)?;
    let bound = listener.local_addr()?;
    let (tx, rx) = sync_channel(QUEUE);
    let live = Arc::new(AtomicUsize::new(0));
    std::thread::Builder::new()
        .name("fabric-otlp".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                if live.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                    continue; // dropped: closes the connection
                }
                live.fetch_add(1, Ordering::SeqCst);
                let (tx, slot) = (tx.clone(), live.clone());
                let spawned = std::thread::Builder::new()
                    .name("fabric-otlp-conn".into())
                    .spawn(move || {
                        let _ = serve(stream, &tx);
                        slot.fetch_sub(1, Ordering::SeqCst);
                    });
                if spawned.is_err() {
                    // The closure never ran, so its slot is released here.
                    live.fetch_sub(1, Ordering::SeqCst);
                }
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

fn respond(stream: &mut TcpStream, status: &str, body: &[u8], close: bool) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/x-protobuf\r\ncontent-length: {}\r\n{}\r\n",
        body.len(),
        if close { "connection: close\r\n" } else { "" }
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// Serve one connection until it closes or a request is refused.
fn serve(stream: TcpStream, tx: &SyncSender<Export>) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    loop {
        let head = match read_head(&mut reader) {
            Ok(Some(head)) => head,
            Ok(None) => return Ok(()),
            Err(_) => return respond(&mut writer, "400 Bad Request", b"", true),
        };
        if head.method != "POST" || head.path.split('?').next() != Some("/v1/traces") {
            return respond(&mut writer, "404 Not Found", b"", true);
        }
        if head
            .content_type
            .as_deref()
            .is_none_or(|t| t.split(';').next().map(str::trim) != Some("application/x-protobuf"))
        {
            return respond(&mut writer, "415 Unsupported Media Type", b"", true);
        }
        let Some(length) = head.content_length else {
            return respond(&mut writer, "411 Length Required", b"", true);
        };
        if length > MAX_EXPORT {
            return respond(&mut writer, "413 Content Too Large", b"", true);
        }
        let mut body = vec![0_u8; length];
        reader.read_exact(&mut body)?;
        if length == 0 || ExportTraceServiceRequest::decode(body.as_slice()).is_err() {
            respond(&mut writer, "400 Bad Request", b"", head.close)?;
            if head.close {
                return Ok(());
            }
            continue;
        }
        let (reply, answer) = sync_channel(1);
        let answer = if tx.send(Export { body, reply }).is_err() {
            Commit::Unavailable("the Spindle is stopping".into())
        } else {
            answer
                .recv_timeout(COMMIT_WAIT)
                .unwrap_or(Commit::Unavailable("commit timed out".into()))
        };
        match answer {
            // An empty ExportTraceServiceResponse: full success.
            Commit::Committed(_) => respond(&mut writer, "200 OK", b"", head.close)?,
            Commit::Unavailable(_) => {
                respond(&mut writer, "503 Service Unavailable", b"", head.close)?
            }
        }
        if head.close {
            return Ok(());
        }
    }
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
