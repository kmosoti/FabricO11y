#![forbid(unsafe_code)]

use fabric_client::{
    CanonicalResponse, FabricClient, FabricQueryClient, FabricTransport, SpindleHealth,
    SpindleReply, custody_lag,
};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixListener};

static TEST_SEQ: AtomicU64 = AtomicU64::new(0);

struct NonTmpScratch(PathBuf);

impl NonTmpScratch {
    fn new(prefix: &str) -> Self {
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
            .unwrap_or_else(|| PathBuf::from("/data/tmp"));
        std::fs::create_dir_all(&base).expect("create scratch base");
        let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = base.join(format!("{prefix}-{}-{seq}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for NonTmpScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn direct_socket_reuses_persistent_uds_and_reads_spindle_and_canonical_tcp() {
    let scratch = NonTmpScratch::new("fabric-client-direct-socket");
    let daemon_sock = scratch.path().join("daemon.sock");
    let spindle_sock = scratch.path().join("spindle.sock");

    let daemon_listener = UnixListener::bind(&daemon_sock).expect("bind daemon.sock");
    let daemon_accepts = Arc::new(AtomicUsize::new(0));
    let accepts_ref = Arc::clone(&daemon_accepts);
    let daemon_task = tokio::spawn(async move {
        let (stream, _) = daemon_listener.accept().await.expect("accept daemon");
        accepts_ref.fetch_add(1, Ordering::SeqCst);
        let (r, mut w) = stream.into_split();
        let mut reader = BufReader::new(r);
        let mut line = String::new();
        while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
            let req: serde_json::Value = serde_json::from_str(line.trim()).expect("req json");
            let reply = match req["op"].as_str() {
                Some("snapshot") => serde_json::json!({
                    "kind": "snapshot",
                    "timestamp_unix_ms": 1700000000000u64,
                    "hostname": "uds-node",
                    "memory": { "mem_total_bytes": 1000, "mem_available_bytes": 750 }
                }),
                Some("blame") => serde_json::json!({
                    "kind": "blame", "resource": "memory", "entries": []
                }),
                Some("drift") => serde_json::json!({
                    "kind": "drift", "total_findings": 0, "findings": []
                }),
                Some("doctor") => serde_json::json!({
                    "kind": "doctor", "healthy": true, "daemon_running": true, "pid": 77
                }),
                Some("lineage") => serde_json::json!({
                    "kind": "lineage", "target": "app.slice", "nodes": [], "edges": []
                }),
                Some("hot_metrics") => serde_json::json!({
                    "kind": "hot_metrics",
                    "points": [{ "host": "uds-node", "name": "host.mem.available_bytes", "timestamp_ms": 123, "value": 750.0 }]
                }),
                _ => serde_json::json!({ "kind": "error", "message": "unknown op" }),
            };
            let mut out = serde_json::to_vec(&reply).unwrap();
            out.push(b'\n');
            w.write_all(&out).await.unwrap();
            w.flush().await.unwrap();
            line.clear();
        }
    });

    let spindle_listener = UnixListener::bind(&spindle_sock).expect("bind spindle.sock");
    let spindle_task = tokio::spawn(async move {
        let (mut stream, _) = spindle_listener.accept().await.expect("accept spindle");
        let mut hdr = [0u8; 4];
        stream.read_exact(&mut hdr).await.unwrap();
        let mut body = vec![0u8; u32::from_be_bytes(hdr) as usize];
        stream.read_exact(&mut body).await.unwrap();
        let reply = SpindleReply::Status {
            status: SpindleHealth {
                origin: "spindle-uds-01".into(),
                records: 12,
                highest_record_id: 12,
                canonical_through: 10,
                ..SpindleHealth::default()
            },
        };
        let encoded = serde_json::to_vec(&reply).unwrap();
        stream
            .write_all(&(encoded.len() as u32).to_be_bytes())
            .await
            .unwrap();
        stream.write_all(&encoded).await.unwrap();
        stream.flush().await.unwrap();
    });

    let client = FabricClient::with_bin(None, None).with_transport(FabricTransport::DirectSocket {
        daemon_socket: daemon_sock,
        spindle_socket: Some(spindle_sock),
        fallback_to_cli: false,
    });
    assert!(client.is_available());
    let bundle = client.collect().await;
    assert!(
        bundle.online,
        "direct socket collect failed: {:?}",
        bundle.error
    );
    assert_eq!(bundle.source_desc, "fabric@uds-node");
    assert_eq!(bundle.doctor.as_ref().and_then(|d| d.pid), Some(77));
    assert_eq!(bundle.custody.as_ref().map(custody_lag), Some(2));
    assert_eq!(
        client.collect_lineage("app.slice").await.unwrap().target,
        "app.slice"
    );
    assert_eq!(
        client.try_query_mem_history().await.unwrap(),
        vec![(123, 750.0)]
    );
    // All 6 daemon RPCs reused a single persistent UDS connection!
    assert_eq!(daemon_accepts.load(Ordering::SeqCst), 1);

    drop(client);
    daemon_task.await.unwrap();
    spindle_task.await.unwrap();

    // Direct loopback TCP canonical query check.
    let tcp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp_listener.local_addr().unwrap();
    let cred_secret = "direct-secret-token-xyz";
    let tcp_task = tokio::spawn(async move {
        async fn read_tcp_frame(s: &mut tokio::net::TcpStream) -> Vec<u8> {
            let mut hdr = [0u8; 4];
            s.read_exact(&mut hdr).await.unwrap();
            let mut buf = vec![0u8; u32::from_be_bytes(hdr) as usize];
            s.read_exact(&mut buf).await.unwrap();
            buf
        }
        let (mut stream, _) = tcp_listener.accept().await.unwrap();
        let hello_bytes = read_tcp_frame(&mut stream).await;
        assert!(!String::from_utf8_lossy(&hello_bytes).contains(cred_secret));
        let hello: fabric::canonical::ClientHello = serde_json::from_slice(&hello_bytes).unwrap();
        let (server_nonce, challenge) =
            fabric::canonical::build_server_challenge(&hello.client_nonce, [cred_secret]).unwrap();
        let challenge_bytes = serde_json::to_vec(&challenge).unwrap();
        stream
            .write_all(&(challenge_bytes.len() as u32).to_be_bytes())
            .await
            .unwrap();
        stream.write_all(&challenge_bytes).await.unwrap();
        stream.flush().await.unwrap();

        let proof_bytes = read_tcp_frame(&mut stream).await;
        assert!(!String::from_utf8_lossy(&proof_bytes).contains(cred_secret));
        let proof: fabric::canonical::ClientProof = serde_json::from_slice(&proof_bytes).unwrap();
        assert!(fabric::canonical::verify_client_proof(
            cred_secret,
            &hello.client_nonce,
            &server_nonce,
            &proof.proof,
        ));

        let req_buf = read_tcp_frame(&mut stream).await;
        assert!(!String::from_utf8_lossy(&req_buf).contains(cred_secret));
        assert!(fabric::canonical::verify_request_mac(
            cred_secret,
            &hello.client_nonce,
            &server_nonce,
            &req_buf,
            &proof.request_mac,
        ));

        let resp = CanonicalResponse::Progress {
            authority: "auth-direct".into(),
            frontiers: BTreeMap::from([("spindle-uds-01".into(), 10)]),
        };
        let encoded = serde_json::to_vec(&resp).unwrap();
        stream
            .write_all(&(encoded.len() as u32).to_be_bytes())
            .await
            .unwrap();
        stream.write_all(&encoded).await.unwrap();
        stream.flush().await.unwrap();
    });

    let cred = scratch.path().join("direct-reader.token");
    std::fs::write(&cred, format!("{cred_secret}\n")).unwrap();
    std::fs::set_permissions(&cred, std::fs::Permissions::from_mode(0o600)).unwrap();
    let qclient =
        FabricQueryClient::with_bin(None, addr.to_string(), &cred).with_direct_socket(true);
    assert!(qclient.is_available());
    match qclient.progress_typed().await.unwrap() {
        CanonicalResponse::Progress {
            authority,
            frontiers,
        } => {
            assert_eq!(authority, "auth-direct");
            assert_eq!(frontiers.get("spindle-uds-01"), Some(&10));
        }
        other => panic!("unexpected response: {other:?}"),
    }
    tcp_task.await.unwrap();
}
