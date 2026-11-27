use super::*;
use crate::{
    observation::Outcome,
    spindle::{Reply, Request},
};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{
        fs::{PermissionsExt, symlink},
        net::{UnixListener, UnixStream},
    },
};

fn directory() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn report(id: u64) -> BatchReport {
    let context = RunContext {
        run_id: format!("aaaa0000-0000-0000-0000-{id:012x}"),
    };
    BatchReport {
        report_id: format!("bbbb0000-0000-0000-0000-{id:012x}"),
        data: Data::AgentRun {
            agent: "fixture".into(),
            run_id: context.run_id.clone(),
            duration_ns: 3,
            status: Outcome::Ok,
        },
        context,
        timestamp_ns: 10,
    }
}

fn recv(listener: &UnixListener) -> (UnixStream, Vec<u8>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("accept: {error}"),
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
    let mut bytes = vec![0; u32::from_be_bytes(header) as usize];
    stream.read_exact(&mut bytes).unwrap();
    (stream, bytes)
}

fn send(mut stream: UnixStream, response: &Reply) {
    let bytes = serde_json::to_vec(response).unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&bytes).unwrap();
}

#[test]
fn capacity_refusal_is_pre_submit_and_does_not_stage_a_report() {
    let base = directory();
    let socket = base.path().join("server.sock");
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
        send(
            stream,
            &Reply::GenerationCapacity {
                used: 128,
                limit: 128,
            },
        );
    });
    let config = Config::local(socket, "batch-test".into()).unwrap();
    let mut client = BatchClient::open(config, &base.path().join("outbox")).unwrap();
    let mut request = serde_json::to_value(report(1)).unwrap();
    request["command"] = "record".into();
    let reply = batch_adapter_reply(&mut client, &serde_json::to_vec(&request).unwrap());
    assert_eq!(reply["code"], "generation_capacity");
    assert_eq!(reply["delivery_state"], "refused_before_acceptance");
    assert_eq!(
        reply["capacity"],
        serde_json::json!({"used":128,"limit":128})
    );
    assert_eq!(client.status().unwrap().pending_report_id, None);
    peer.join().unwrap();
}

#[test]
fn invalid_receipt_leaves_original_frame_for_startup_replay() {
    let base = directory();
    let socket = base.path().join("server.sock");
    let path = base.path().join("outbox");
    let mut config = Config::local(socket.clone(), "batch-test".into()).unwrap();
    // This tests replay after durable I/O, including on shared hosted disks.
    config.delivery_timeout = Duration::from_secs(5);
    let listener = UnixListener::bind(socket).unwrap();
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
                origin: "fake-origin".into(),
                through: 0,
            },
        );
        let (stream, request) = recv(&listener);
        assert!(
            matches!(serde_json::from_slice(&request).unwrap(), Request::LeaseForOrigin { producer, expected_origin } if producer == "batch-test" && expected_origin == "fake-origin")
        );
        send(
            stream,
            &Reply::Lease {
                generation: "test-generation".into(),
                expires_unix_s: wall_time().unwrap() + 86400,
            },
        );
        let (stream, first) = recv(&listener);
        let first_identity = observation::decode_versioned(&first)
            .unwrap()
            .identity()
            .clone();
        let mut invalid = first_identity.clone();
        invalid.sequence += 1;
        send(
            stream,
            &Reply::Receipt {
                receipt: Receipt {
                    id: 8,
                    identity: invalid,
                    duplicate: false,
                },
            },
        );
        let (stream, replay) = recv(&listener);
        assert_eq!(first, replay);
        drop(stream);
        let (stream, replay) = recv(&listener);
        assert_eq!(first, replay);
        send(
            stream,
            &Reply::Receipt {
                receipt: Receipt {
                    id: 8,
                    identity: first_identity.clone(),
                    duplicate: true,
                },
            },
        );
        let (stream, second) = recv(&listener);
        let second = observation::decode_versioned(&second).unwrap();
        assert_eq!(second.context().unwrap(), &report(2).context);
        assert_eq!(second.identity().generation, first_identity.generation);
        assert_eq!(second.identity().sequence, 2);
        send(
            stream,
            &Reply::Receipt {
                receipt: Receipt {
                    id: 9,
                    identity: second.identity().clone(),
                    duplicate: false,
                },
            },
        );
    });

    let mut client = BatchClient::open(config.clone(), &path).unwrap();
    let first = report(1);
    let request = serde_json::to_value(&first).unwrap();
    let mut request = request.as_object().unwrap().clone();
    request.insert("command".into(), "record".into());
    let reply = batch_adapter_reply(&mut client, &serde_json::to_vec(&request).unwrap());
    assert_eq!(reply["report_id"], first.report_id);
    assert_eq!(reply["code"], "mismatched_receipt", "{reply}");
    assert_eq!(reply["delivery_state"], "outcome_unknown");
    assert_eq!(
        client.status().unwrap().pending_report_id,
        Some(first.report_id.clone())
    );
    assert_eq!(client.record(report(2)), Err(BatchError::Full));
    let mut changed = first.clone();
    changed.timestamp_ns += 1;
    assert_eq!(client.record(changed.clone()), Err(BatchError::Conflict));
    drop(client);

    let mut client = BatchClient::open(config.clone(), &path).unwrap();
    let status = client.status().unwrap();
    assert_eq!(status.confirmed, 1);
    assert_eq!(status.pending_report_id, None);
    assert_eq!(status.origin.as_deref(), Some("fake-origin"));
    let recovered = client.record(first.clone()).unwrap();
    assert!(recovered.duplicate);
    assert_eq!(recovered.id, 8);
    assert_eq!(client.record(changed), Err(BatchError::Conflict));
    let next = client.record(report(2)).unwrap();
    assert_eq!(next.id, 9);
    peer.join().unwrap();
    drop(client);

    let mut client = BatchClient::open(config, &path).unwrap();
    assert_eq!(client.record(first).unwrap(), recovered);
    assert_eq!(client.flush_local(Instant::now()).unwrap(), Some(next));
}

#[test]
fn expired_pending_survives_reopen_and_refuses_fresh_reports() {
    let base = directory();
    let path = base.path().join("outbox");
    let config = Config::local(base.path().join("absent"), "batch-test".into()).unwrap();
    let mut outbox = Outbox::open(&path, &config.producer, &config.resource).unwrap();
    let report = report(1);
    let lease = Lease {
        origin: "original-origin".into(),
        generation: "original-generation".into(),
        expires: wall_time().unwrap() - 1,
        expired: false,
        sequence: 0,
    };
    outbox.save_lease(&lease).unwrap();
    let frame = ObservationV3 {
        version: observation::VERSION_V3,
        registry: observation::REGISTRY_V3.into(),
        context: report.context.clone(),
        identity: Identity {
            producer: config.producer.clone(),
            generation: lease.generation.clone(),
            sequence: 1,
        },
        timestamp_ns: report.timestamp_ns,
        resource: config.resource.clone(),
        data: report.data.clone(),
    };
    let bytes = serde_json::to_vec(&frame).unwrap();
    outbox
        .stage(
            &Pending {
                report_id: report.report_id.clone(),
                digest: report.digest().unwrap(),
                bytes: bytes.clone(),
            },
            1,
        )
        .unwrap();
    drop(outbox);

    let mut client = BatchClient::open(config.clone(), &path).unwrap();
    assert_eq!(
        client.status().unwrap().pending_report_id,
        Some(report.report_id.clone())
    );
    assert!(client.status().unwrap().pending_expired);
    assert_eq!(client.flush_local(Instant::now()), Err(BatchError::Expired));
    let mut other = report.clone();
    other.report_id = "cccc0000-0000-0000-0000-000000000001".into();
    assert_eq!(client.record(other), Err(BatchError::Expired));
    assert_eq!(client.record(report), Err(BatchError::Expired));
    drop(client);

    let outbox = Outbox::open(&path, &config.producer, &config.resource).unwrap();
    assert_eq!(outbox.pending().unwrap().unwrap().bytes, bytes);
    let lease = outbox.lease().unwrap().unwrap();
    assert_eq!(lease.generation, frame.identity.generation);
    assert_eq!(lease.sequence, 1);
    assert!(lease.expired);
}

#[test]
fn outbox_excludes_concurrent_owners_and_rejects_unsafe_paths() {
    let base = directory();
    let path = base.path().join("outbox");
    let outbox = Outbox::open(&path, "producer", "local").unwrap();
    assert!(Outbox::open(&path, "producer", "local").is_err());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(path.join("outbox.sqlite"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    drop(outbox);
    assert!(Outbox::open(&path, "other-producer", "local").is_err());
    assert!(Outbox::open(&path, "producer", "other-resource").is_err());
    let alias = base.path().join("alias");
    symlink(&path, &alias).unwrap();
    assert!(Outbox::open(&alias, "producer", "local").is_err());
    assert!(Outbox::open(&alias.join("nested"), "producer", "local").is_err());
    assert!(!path.join("nested").exists());
    fs::hard_link(path.join("outbox.sqlite"), base.path().join("second-link")).unwrap();
    assert!(Outbox::open(&path, "producer", "local").is_err());
    fs::remove_file(base.path().join("second-link")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o750)).unwrap();
    assert!(Outbox::open(&path, "producer", "local").is_err());
}
