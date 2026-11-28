#![cfg(feature = "cli")]
use fabric_client::{
    FabricClient, FabricTransport,
    query_contract::{LiveCursorRequest, LossReason},
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixListener,
};
static SEQ: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
            .unwrap_or_else(|| PathBuf::from("/data/tmp"));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(format!(
            "cursor-client-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn request() -> LiveCursorRequest {
    LiveCursorRequest {
        origin: None,
        epoch: None,
        after: 0,
        count: 2,
        max_bytes: 1024,
    }
}
fn client(path: PathBuf) -> FabricClient {
    FabricClient::with_bin(None, None).with_transport(FabricTransport::DirectSocket {
        daemon_socket: path,
        spindle_socket: None,
        fallback_to_cli: true,
    })
}

#[tokio::test]
async fn typed_live_cursor_preserves_wire_fields_and_u64() {
    let root = Scratch::new();
    let socket = root.0.join("daemon.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        stream.read_line(&mut line).await.unwrap();
        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["op"], "live_metrics_v1");
        assert_eq!(request["max_bytes"], 1024);
        let reply = serde_json::json!({"kind":"live_metrics_v1","version":1,"origin":"origin","epoch":"a".repeat(64),"observed_through":u64::MAX,"epoch_start_after":0,"oldest_retained":u64::MAX,"highest_lost":5,"highest_lost_reason":"evicted","gap":true,"gap_reason":"evicted","next_after":u64::MAX,"entries":[{"record_id":u64::MAX,"point":{"timestamp_ms":-1,"name":"m","value":1.5,"host":"h","slice":"s","unit":"u","pid":null,"labels_json":"{}"}}]});
        stream
            .get_mut()
            .write_all(format!("{reply}\n").as_bytes())
            .await
            .unwrap();
    });
    let page = client(socket)
        .try_query_live_metrics(&request())
        .await
        .unwrap();
    assert_eq!(page.observed_through, u64::MAX);
    assert_eq!(page.next_after, u64::MAX);
    assert_eq!(page.entries[0].record_id, u64::MAX);
    assert_eq!(page.entries[0].point.timestamp_ms, -1);
    assert!(page.gap);
    assert_eq!(page.gap_reason, Some(LossReason::Evicted));
    assert_eq!(page.epoch, "a".repeat(64));
    task.await.unwrap();
}

#[tokio::test]
async fn live_cursor_rejects_oversize_and_never_falls_back() {
    let root = Scratch::new();
    let socket = root.0.join("daemon.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let task = tokio::spawn(async move {
        for _ in 0..2 {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            let _ = stream
                .get_mut()
                .write_all(format!("{}\n", "x".repeat(1024)).as_bytes())
                .await;
        }
    });
    let err = client(socket)
        .try_query_live_metrics(&request())
        .await
        .unwrap_err();
    assert!(err.contains("exceeded size limit"), "{err}");
    task.await.unwrap();
    let mut invalid = request();
    invalid.count = 0;
    assert!(
        client(root.0.join("absent"))
            .try_query_live_metrics(&invalid)
            .await
            .unwrap_err()
            .contains("bounds")
    );
    let cli = FabricClient::with_bin(None, None);
    assert!(
        cli.try_query_live_metrics(&request())
            .await
            .unwrap_err()
            .contains("direct daemon socket")
    );
}

#[tokio::test]
async fn live_cursor_surfaces_daemon_error_and_rejects_wrong_version() {
    let root = Scratch::new();
    let socket = root.0.join("daemon.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        stream.read_line(&mut line).await.unwrap();
        stream
            .get_mut()
            .write_all(b"{\"kind\":\"error\",\"message\":\"live cursor disabled\"}\n")
            .await
            .unwrap();
        line.clear();
        stream.read_line(&mut line).await.unwrap();
        let reply = serde_json::json!({"kind":"live_metrics_v1","version":2,"origin":"origin","epoch":"a".repeat(64),"observed_through":0,"epoch_start_after":0,"oldest_retained":null,"highest_lost":0,"highest_lost_reason":null,"gap":false,"gap_reason":null,"next_after":0,"entries":[]});
        stream
            .get_mut()
            .write_all(format!("{reply}\n").as_bytes())
            .await
            .unwrap();
    });
    let c = client(socket);
    assert!(
        c.try_query_live_metrics(&request())
            .await
            .unwrap_err()
            .contains("disabled")
    );
    assert!(
        c.try_query_live_metrics(&request())
            .await
            .unwrap_err()
            .contains("invalid daemon live cursor response")
    );
    task.await.unwrap();
}

#[tokio::test]
async fn rejects_skipped_unseen_entries() {
    let root = Scratch::new();
    let socket = root.0.join("daemon.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        stream.read_line(&mut line).await.unwrap();
        let reply = serde_json::json!({"kind":"live_metrics_v1","version":1,"origin":"origin","epoch":"a".repeat(64),"observed_through":10,"epoch_start_after":0,"oldest_retained":1,"highest_lost":0,"highest_lost_reason":null,"gap":false,"gap_reason":null,"next_after":10,"entries":[{"record_id":1,"point":{"timestamp_ms":0,"name":"m","value":1.0,"host":"h","slice":"s","unit":"u","pid":null,"labels_json":"{}"}}]});
        stream
            .get_mut()
            .write_all(format!("{reply}\n").as_bytes())
            .await
            .unwrap();
    });
    assert!(
        client(socket)
            .try_query_live_metrics(&request())
            .await
            .unwrap_err()
            .contains("invalid daemon live cursor response")
    );
    task.await.unwrap();
}

#[tokio::test]
async fn cancelled_call_discards_inflight_connection() {
    let root = Scratch::new();
    let socket = root.0.join("daemon.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let (first, _) = listener.accept().await.unwrap();
        let mut first = BufReader::new(first);
        let mut line = String::new();
        first.read_line(&mut line).await.unwrap();
        seen_tx.send(()).unwrap();
        let (second, _) = listener.accept().await.unwrap();
        let mut second = BufReader::new(second);
        line.clear();
        second.read_line(&mut line).await.unwrap();
        let reply = serde_json::json!({"kind":"live_metrics_v1","version":1,"origin":"origin","epoch":"a".repeat(64),"observed_through":0,"epoch_start_after":0,"oldest_retained":null,"highest_lost":0,"highest_lost_reason":null,"gap":false,"gap_reason":null,"next_after":0,"entries":[]});
        second
            .get_mut()
            .write_all(format!("{reply}\n").as_bytes())
            .await
            .unwrap();
    });
    let c = client(socket);
    let cloned = c.clone();
    let call = tokio::spawn(async move { cloned.try_query_live_metrics(&request()).await });
    seen_rx.await.unwrap();
    call.abort();
    let _ = call.await;
    let page = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        c.try_query_live_metrics(&request()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(page.next_after, 0);
    task.await.unwrap();
}
