use fabric::canonical::{self, ForwardRecord, Request, Response};
use fabric_server::{Config, NodeGrant, Server};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

fn scratch_dir() -> tempfile::TempDir {
    let base = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
        .unwrap_or_else(|| PathBuf::from("/data/tmp"));
    fs::create_dir_all(&base).unwrap();
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(base)
        .unwrap()
}

fn write_secret(dir: &Path, name: &str, value: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, value).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}

fn log_record(sequence: u64, msg_bytes: usize) -> ForwardRecord {
    let raw = serde_json::json!({
        "version": 2,
        "registry": "fabrico11y/2",
        "identity": {
            "producer": "sec-test",
            "generation": "gen-1",
            "sequence": sequence
        },
        "timestamp_ns": sequence * 1000,
        "resource": "svc",
        "data": {
            "kind": "log",
            "level": "info",
            "message": "x".repeat(msg_bytes),
        }
    })
    .to_string();
    ForwardRecord {
        source_id: sequence,
        raw,
    }
}

#[test]
fn omitted_nodes_enforce_per_origin_fair_share_in_canonical_and_projection() {
    let dir = scratch_dir();
    let reader_tok = "reader-fair-share-token-01234";
    let noisy_tok = "noisy-fair-share-token-012345";
    let quiet_tok = "quiet-fair-share-token-012345";
    let reader_file = write_secret(dir.path(), "reader", reader_tok);
    let noisy_file = write_secret(dir.path(), "noisy", noisy_tok);
    let quiet_file = write_secret(dir.path(), "quiet", quiet_tok);

    // 1 MiB canonical quota with two omitted enrolled nodes => ~448 KiB usable / 2 = ~224 KiB each.
    let server = Server::open(Config {
        listen: "127.0.0.1:0".into(),
        storage: dir.path().join("canonical"),
        projection: dir.path().join("projection"),
        quota_bytes: 1 << 20,
        projection_quota_bytes: 512 << 10,
        reader_credential_file: reader_file,
        nodes: vec![
            NodeGrant {
                origin: "noisy".into(),
                credential_file: noisy_file,
            },
            NodeGrant {
                origin: "quiet".into(),
                credential_file: quiet_file,
            },
        ],
        decision_writers: Vec::new(),
        node_quota_bytes: Default::default(),
        engine_endpoint: None,
        engine_auth_token_file: None,
        allow_unauthenticated_loopback_engine: false,
        max_connections: 4,
    })
    .unwrap();
    let endpoint = server.address();
    let stop = server.stop_handle();
    let worker = thread::spawn(move || server.serve());

    let mut noisy_capped = false;
    for seq in 1..=30u64 {
        let resp = canonical::exchange(
            &endpoint,
            noisy_tok,
            &Request::Ingest {
                origin: "noisy".into(),
                records: vec![log_record(seq, 24 * 1024)],
            },
            Instant::now() + Duration::from_secs(3),
        )
        .unwrap();
        match resp {
            Response::Committed { .. } => {}
            Response::Error { error } => {
                assert!(
                    error.contains("node payload quota exhausted"),
                    "unexpected error for noisy origin: {error}"
                );
                noisy_capped = true;
                break;
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }
    assert!(noisy_capped, "noisy omitted origin must hit fair-share cap");

    // Quiet sibling origin must still ingest within its untouched fair share.
    let quiet_resp = canonical::exchange(
        &endpoint,
        quiet_tok,
        &Request::Ingest {
            origin: "quiet".into(),
            records: vec![log_record(1, 24 * 1024)],
        },
        Instant::now() + Duration::from_secs(3),
    )
    .unwrap();
    assert!(matches!(quiet_resp, Response::Committed { through: 1, .. }));

    stop.store(true, Ordering::SeqCst);
    let _ = TcpStream::connect(&endpoint);
    worker.join().unwrap().unwrap();
}

#[test]
fn preauth_slowloris_connections_do_not_starve_authenticated_requests() {
    let dir = scratch_dir();
    let reader_tok = "reader-slowloris-token-012345";
    let node_tok = "node-slowloris-token-01234567";
    let reader_file = write_secret(dir.path(), "reader", reader_tok);
    let node_file = write_secret(dir.path(), "node", node_tok);

    let server = Server::open(Config {
        listen: "127.0.0.1:0".into(),
        storage: dir.path().join("canonical"),
        projection: dir.path().join("projection"),
        quota_bytes: 4 << 20,
        projection_quota_bytes: 4 << 20,
        reader_credential_file: reader_file,
        nodes: vec![NodeGrant {
            origin: "node-a".into(),
            credential_file: node_file,
        }],
        decision_writers: Vec::new(),
        node_quota_bytes: Default::default(),
        engine_endpoint: None,
        engine_auth_token_file: None,
        allow_unauthenticated_loopback_engine: false,
        max_connections: 2,
    })
    .unwrap();
    let endpoint = server.address();
    let stop = server.stop_handle();
    let worker = thread::spawn(move || server.serve());

    // Open 8 unauthenticated TCP streams and send incomplete 4-byte length frames.
    // Even though max_connections == 2, PreAuthGate isolates these from worker threads.
    let mut stalled = Vec::new();
    for _ in 0..8 {
        let mut s = TcpStream::connect(&endpoint).unwrap();
        s.write_all(&128u32.to_be_bytes()).unwrap();
        s.write_all(b"{\"ver").unwrap();
        let _ = s.flush();
        stalled.push(s);
    }

    // Legitimate authenticated request succeeds immediately without waiting 250ms.
    let started = Instant::now();
    let resp = canonical::exchange(
        &endpoint,
        reader_tok,
        &Request::Progress,
        Instant::now() + Duration::from_secs(2),
    )
    .unwrap();
    assert!(
        matches!(resp, Response::Progress { .. }),
        "unexpected: {resp:?}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "legitimate peer was delayed by pre-auth connections: {:?}",
        started.elapsed()
    );

    // Pre-auth sockets must be timed out & closed shortly after HANDSHAKE_TIMEOUT (250ms).
    thread::sleep(Duration::from_millis(400));
    for s in &mut stalled {
        s.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let mut buf = [0u8; 16];
        match s.read(&mut buf) {
            Ok(0) => {}
            Err(_) => {}
            Ok(n) => panic!("unexpected data on unauthenticated stalled socket: {n}"),
        }
    }

    stop.store(true, Ordering::SeqCst);
    let _ = TcpStream::connect(&endpoint);
    worker.join().unwrap().unwrap();
}
