//! Focused batch-adapter response test using a simulated local Spindle peer.
use fabric::{
    observation,
    sdk::{BatchClient, Config, batch_adapter_reply},
    spindle::{Reply, Request, custody::Receipt},
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn recv(listener: &UnixListener) -> (UnixStream, Vec<u8>) {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(2))
            }
            Err(e) => panic!("accept: {e}"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut header = [0; 4];
    stream.read_exact(&mut header).unwrap();
    let len = u32::from_be_bytes(header) as usize;
    assert!(len <= observation::MAX_FRAME);
    let mut bytes = vec![0; len];
    stream.read_exact(&mut bytes).unwrap();
    (stream, bytes)
}
fn send(mut stream: UnixStream, reply: &Reply) {
    let bytes = serde_json::to_vec(reply).unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&bytes).unwrap();
}
fn report(id: u64) -> Value {
    json!({"command":"record_application", "report_id":format!("bbbb0000-0000-4000-8000-{id:012x}"),
        "context":{"run_id":"aaaa0000-0000-4000-8000-000000000001"},
        "scope":{"origin":"origin","producer":"batch-test"}, "parent_report_id":null,
        "schema":"example.result.v1", "producer_completion":{"unix_ms":42,"clock":{"clock_id":"producer","source":"fixture","uncertainty_ms":null}},
        "artifact_json":format!("{{\"item\":{id}}}"),"timestamp_ns":123})
}
fn submit(client: &mut BatchClient, value: &Value) -> Value {
    batch_adapter_reply(client, &serde_json::to_vec(value).unwrap())
}
#[test]
fn pending_burst_refuses_distinct_reports_then_recovers_exact_receipt() {
    let root = tempfile::tempdir_in(PathBuf::from(
        std::env::var_os("TMPDIR").expect("disk-backed TMPDIR"),
    ))
    .unwrap();
    let socket = root.path().join("spindle.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let peer = thread::spawn(move || {
        let (stream, request) = recv(&listener);
        assert!(matches!(
            serde_json::from_slice(&request).unwrap(),
            Request::FabricProgress
        ));
        send(
            stream,
            &Reply::FabricProgress {
                authority: None,
                origin: "origin".into(),
                through: 0,
            },
        );
        let (stream, request) = recv(&listener);
        assert!(matches!(
            serde_json::from_slice(&request).unwrap(),
            Request::LeaseForOrigin { .. }
        ));
        let expires = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        send(
            stream,
            &Reply::Lease {
                generation: "lease".into(),
                expires_unix_s: expires,
            },
        );
        let (stream, first) = recv(&listener);
        let identity = observation::decode_versioned(&first)
            .unwrap()
            .identity()
            .clone();
        drop(stream); // Receiver accepted A but its reply is lost.
        let (stream, replay) = recv(&listener);
        assert_eq!(
            first, replay,
            "distinct burst reports must not reach the peer"
        );
        send(
            stream,
            &Reply::Receipt {
                receipt: Receipt {
                    id: 77,
                    identity,
                    duplicate: true,
                },
            },
        );
        let (stream, second) = recv(&listener);
        assert_ne!(first, second);
        let identity = observation::decode_versioned(&second)
            .unwrap()
            .identity()
            .clone();
        send(
            stream,
            &Reply::Receipt {
                receipt: Receipt {
                    id: 78,
                    identity,
                    duplicate: false,
                },
            },
        );
    });
    let mut config = Config::local(socket, "batch-test".into()).unwrap();
    config.delivery_timeout = Duration::from_secs(5);
    let mut client = BatchClient::open(config, &root.path().join("outbox")).unwrap();
    let a = report(1);
    let b = report(2);
    let c = report(3);
    let unknown = submit(&mut client, &a);
    assert_eq!(unknown["delivery_state"], "outcome_unknown", "{unknown}");
    assert_eq!(unknown["durably_staged"], true);
    for report in [&b, &c] {
        let refused = submit(&mut client, report);
        assert_eq!(refused["code"], "full", "{refused}");
        assert_eq!(refused["delivery_state"], "refused_before_acceptance");
        assert_eq!(refused["durably_staged"], false);
        assert!(
            client
                .report_status(report["report_id"].as_str().unwrap())
                .unwrap()
                .is_none()
        );
    }
    let confirmed = submit(&mut client, &a);
    assert_eq!(
        confirmed["delivery_state"], "custody_confirmed",
        "{confirmed}"
    );
    assert_eq!(confirmed["receipt"]["id"], 77);
    let accepted = submit(&mut client, &b);
    assert_eq!(
        accepted["delivery_state"], "custody_confirmed",
        "{accepted}"
    );
    assert_eq!(accepted["receipt"]["id"], 78);
    peer.join().unwrap();
}
