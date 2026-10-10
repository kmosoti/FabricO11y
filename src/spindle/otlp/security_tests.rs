use super::*;
use std::net::Shutdown;
use std::thread::JoinHandle;

fn policy() -> TransportPolicy {
    TransportPolicy {
        header: Duration::from_millis(150),
        body: Duration::from_millis(150),
        commit: Duration::from_millis(150),
        response: Duration::from_millis(150),
        request: Duration::from_secs(2),
        connection: Duration::from_secs(3),
        requests: 1,
    }
}

struct Fixture {
    client: TcpStream,
    exports: Option<Receiver<Export>>,
    worker: Option<JoinHandle<io::Result<()>>>,
}

impl Fixture {
    fn new(policy: TransportPolicy, preload: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        let accepted = Instant::now();
        let (tx, rx) = sync_channel(1);
        if preload {
            let (reply, _) = sync_channel(1);
            tx.send(Export {
                body: vec![7],
                reply,
            })
            .unwrap();
        }
        let worker = std::thread::spawn(move || serve(server, &tx, accepted, policy));
        Self {
            client,
            exports: Some(rx),
            worker: Some(worker),
        }
    }

    fn request(&mut self) {
        self.client.write_all(b"POST /v1/traces HTTP/1.1\r\nContent-Type: application/x-protobuf\r\nContent-Length: 2\r\n\r\n\x0a\x00").unwrap();
    }

    fn receive(&self) -> Export {
        self.exports
            .as_ref()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
    }

    fn response(&mut self) -> String {
        let mut bytes = Vec::new();
        let result = self.client.read_to_end(&mut bytes);
        if let Err(error) = result {
            assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
        }
        String::from_utf8(bytes).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Release a blocked sender even when testing the rejected blocking-send
        // implementation, then close the socket before joining the worker.
        drop(self.exports.take());
        let _ = self.client.shutdown(Shutdown::Both);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !std::thread::panicking() {
                assert!(result.is_ok(), "OTLP fixture worker panicked");
            }
        }
    }
}

#[test]
fn security_direct_start_refuses_nonloopback() {
    assert!(start("0.0.0.0:0".parse().unwrap()).is_err());
    assert!(start("[::]:0".parse().unwrap()).is_err());
}

#[test]
fn security_connection_slots_are_bounded_and_owned() {
    let live = Arc::new(AtomicUsize::new(0));
    let permits: Vec<_> = (0..MAX_CONNECTIONS)
        .map(|_| ConnectionPermit::acquire(&live).unwrap())
        .collect();
    assert!(ConnectionPermit::acquire(&live).is_none());
    assert_eq!(live.load(Ordering::SeqCst), MAX_CONNECTIONS);
    drop(permits);
    assert_eq!(live.load(Ordering::SeqCst), 0);
}

#[test]
fn security_unstarted_and_panicking_workers_release_their_slot() {
    let live = Arc::new(AtomicUsize::new(0));
    let permit = ConnectionPermit::acquire(&live).unwrap();
    let unstarted = move || {
        let _permit = permit;
    };
    drop(unstarted);
    assert_eq!(live.load(Ordering::SeqCst), 0);
    let copy = Arc::clone(&live);
    assert!(
        std::panic::catch_unwind(move || {
            let _permit = ConnectionPermit::acquire(&copy).unwrap();
            panic!("synthetic worker failure");
        })
        .is_err()
    );
    assert_eq!(live.load(Ordering::SeqCst), 0);
}

#[test]
fn security_full_queue_is_rejected_without_waiting_for_a_consumer() {
    let mut fixture = Fixture::new(policy(), true);
    fixture.request();
    let response = fixture.response();
    assert!(response.starts_with("HTTP/1.1 503"));
    assert!(response.contains("connection: close"));
    assert_eq!(fixture.receive().body, vec![7]);
}

#[test]
fn security_only_confirmed_commit_produces_success_and_closes_at_request_cap() {
    let mut fixture = Fixture::new(policy(), false);
    fixture.request();
    fixture.receive().reply.send(Commit::Committed(42)).unwrap();
    let response = fixture.response();
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("connection: close"));
}

#[test]
fn security_waiter_timeout_does_not_ack_or_remove_an_admitted_export() {
    let mut fixture = Fixture::new(policy(), false);
    fixture.request();
    let admitted = fixture.receive();
    let response = fixture.response();
    assert!(response.starts_with("HTTP/1.1 503"));
    assert_eq!(admitted.body, vec![0x0a, 0x00]);
    // The consumer still owns the export. A later commit notification cannot
    // resurrect the timed-out HTTP waiter or change its response into an ACK.
    assert!(admitted.reply.send(Commit::Committed(42)).is_err());
}

#[test]
fn security_slow_header_progress_does_not_reset_its_deadline() {
    let mut fixture = Fixture::new(policy(), false);
    for _ in 0..12 {
        if fixture.client.write_all(b"P").is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(!fixture.response().contains("200 OK"));
    assert!(fixture.exports.as_ref().unwrap().try_recv().is_err());
}

#[test]
fn security_slow_body_progress_does_not_reset_its_deadline() {
    let mut fixture = Fixture::new(policy(), false);
    fixture.client.write_all(b"POST /v1/traces HTTP/1.1\r\nContent-Type: application/x-protobuf\r\nContent-Length: 100\r\n\r\n").unwrap();
    for _ in 0..12 {
        if fixture.client.write_all(b"x").is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(!fixture.response().contains("200 OK"));
    assert!(fixture.exports.as_ref().unwrap().try_recv().is_err());
}

#[test]
fn security_pipelined_bytes_do_not_bypass_connection_request_limit() {
    let mut fixture = Fixture::new(policy(), false);
    fixture.request();
    fixture.request();
    fixture.receive().reply.send(Commit::Committed(1)).unwrap();
    let response = fixture.response();
    assert_eq!(response.matches("HTTP/1.1").count(), 1);
    assert!(fixture.exports.as_ref().unwrap().try_recv().is_err());
}
