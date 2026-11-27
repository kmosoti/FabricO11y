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
            None,
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

fn application_report(id: u64, producer: &str, origin: &str) -> ApplicationReport {
    ApplicationReport {
        report_id: format!("bbbb0000-0000-0000-0000-{id:012x}"),
        context: RunContext {
            run_id: format!("aaaa0000-0000-0000-0000-{id:012x}"),
        },
        scope: ReportScope::new(origin.into(), producer.into()).unwrap(),
        parent_report_id: None,
        schema: "example.result.v1".into(),
        producer_completion: ProvenancedTime {
            unix_ms: 42,
            clock: fabric_evidence::evidence_manifest::ClockProvenance {
                clock_id: "producer-clock".into(),
                source: "worker".into(),
                uncertainty_ms: None,
            },
        },
        artifact_json: "{\"label\":\"caf\\u00e9\"}\n".into(),
        timestamp_ns: 123,
    }
}

#[test]
fn application_lost_reply_replays_exact_frame_and_rejects_changed_identity() {
    let base = directory();
    let socket = base.path().join("server.sock");
    let path = base.path().join("outbox");
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
        assert!(
            matches!(serde_json::from_slice(&request).unwrap(), Request::LeaseForOrigin { expected_origin, .. } if expected_origin == "origin")
        );
        send(
            stream,
            &Reply::Lease {
                generation: "lease".into(),
                expires_unix_s: wall_time().unwrap() + 3600,
            },
        );
        let (stream, original) = recv(&listener);
        assert!(
            !original
                .windows(b"caf\\u00e9".len())
                .any(|w| w == b"caf\\u00e9")
        );
        let decoded = observation::decode_versioned(&original).unwrap();
        let identity = decoded.identity().clone();
        let Data::JsonLog { fields } = decoded.data() else {
            panic!("expected application carrier");
        };
        assert_eq!(fields.len(), 1);
        let envelope = serde_json::to_vec(&fields[APPLICATION_MARKER]).unwrap();
        let result = ApplicationResult::decode_bounded(&envelope).unwrap();
        result
            .verify_original_artifact(b"{\"label\":\"caf\\u00e9\"}\n")
            .unwrap();
        assert_eq!(
            result.canonical_payload().unwrap(),
            "{\"label\":\"café\"}".as_bytes()
        );
        drop(stream); // committed by the simulated receiver, but reply is withheld
        let (stream, replay) = recv(&listener);
        assert_eq!(original, replay);
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
    });
    let mut config = Config::local(socket, "batch-test".into()).unwrap();
    config.delivery_timeout = Duration::from_secs(5);
    let mut client = BatchClient::open(config.clone(), &path).unwrap();
    let report = application_report(1, "batch-test", "origin");
    let mut request = serde_json::to_value(&report).unwrap();
    request["command"] = "record_application".into();
    let response = batch_adapter_reply(&mut client, &serde_json::to_vec(&request).unwrap());
    assert_eq!(response["delivery_state"], "outcome_unknown", "{response}");
    assert_eq!(response["durably_staged"], true);
    assert_eq!(
        client.status().unwrap().pending_report_id.as_deref(),
        Some(report.report_id.as_str())
    );
    let pending = client.report_status(&report.report_id).unwrap().unwrap();
    assert_eq!(pending.delivery_state, "outcome_unknown");
    let timing = pending.application_timing.unwrap();
    assert_eq!(timing.producer_completion, report.producer_completion);
    assert_eq!(
        timing.outbox_staged_at.clock.source,
        "batch_outbox.stage_transaction"
    );
    drop(client);
    let mut client = BatchClient::open(config, &path).unwrap();
    assert_eq!(client.status().unwrap().confirmed, 1);
    let status = client.report_status(&report.report_id).unwrap().unwrap();
    assert_eq!(status.delivery_state, "custody_confirmed");
    assert_eq!(status.application_timing, Some(timing.clone()));
    let confirmed = client.record_application(report.clone()).unwrap();
    assert_eq!(confirmed.application_timing, Some(timing));
    assert_eq!(confirmed.receipt.id, 77);
    assert_eq!(
        confirmed.original_artifact_digest.0.hex,
        "426d8f9948cd8aa9aede8235c65e8f5fd79eb048055e962afbea5c2b5ca1b64d"
    );
    assert_eq!(
        confirmed.application_payload_digest.0.hex,
        "2d7fba14f0a7cffed454bc268d791c0f32d11f62946998a323087ccf5df29075"
    );
    let mut changed = report.clone();
    changed.artifact_json = "{\"label\":\"café\"}".into();
    assert!(matches!(
        client.record_application(changed),
        Err(BatchError::Conflict)
    ));
    let mut terminal = self::report(1);
    terminal.report_id = report.report_id;
    assert_eq!(client.record(terminal), Err(BatchError::Conflict));
    peer.join().unwrap();
}

#[test]
fn application_scope_mismatch_refuses_before_lease() {
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
                origin: "actual".into(),
                through: 0,
            },
        );
    });
    let config = Config::local(socket, "batch-test".into()).unwrap();
    let mut client = BatchClient::open(config, &base.path().join("outbox")).unwrap();
    assert!(matches!(
        client.record_application(application_report(1, "other", "actual")),
        Err(BatchError::Invalid(_))
    ));
    assert!(matches!(
        client.record_application(application_report(1, "batch-test", "wrong")),
        Err(BatchError::OriginMismatch)
    ));
    assert_eq!(client.status().unwrap().pending_report_id, None);
    peer.join().unwrap();
}

#[test]
fn application_capacity_refuses_before_staging() {
    let base = directory();
    let path = base.path().join("outbox");
    let config = Config::local(base.path().join("absent.sock"), "batch-test".into()).unwrap();
    // Close the exclusive owner before seeding this disposable store.
    drop(BatchClient::open(config.clone(), &path).unwrap());
    let mut conn = rusqlite::Connection::open(path.join("outbox.sqlite")).unwrap();
    let tx = conn.transaction().unwrap();
    {
        let mut insert = tx.prepare("INSERT INTO reports(ordinal,report_id,digest,receipt) VALUES(?1,?2,zeroblob(32),x'7b7d')").unwrap();
        for i in 1..=4096 {
            insert
                .execute(rusqlite::params![i, format!("{i:036}")])
                .unwrap();
        }
    }
    tx.commit().unwrap();
    drop(conn);
    let mut client = BatchClient::open(config, &path).unwrap();
    assert!(matches!(
        client.record_application(application_report(1, "batch-test", "origin")),
        Err(BatchError::Full)
    ));
    assert_eq!(client.status().unwrap().pending_report_id, None);
}

#[test]
fn application_frame_budget_is_checked_before_staging() {
    let base = directory();
    let socket = base.path().join("server.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let peer = thread::spawn(move || {
        let (stream, _) = recv(&listener);
        send(
            stream,
            &Reply::FabricProgress {
                authority: None,
                origin: "origin".into(),
                through: 0,
            },
        );
        let (stream, _) = recv(&listener);
        send(
            stream,
            &Reply::Lease {
                generation: "lease".into(),
                expires_unix_s: wall_time().unwrap() + 3600,
            },
        );
    });
    let mut config = Config::local(socket, "batch-test".into()).unwrap();
    config.max_record = 256;
    let mut client = BatchClient::open(config, &base.path().join("outbox")).unwrap();
    assert!(matches!(
        client.record_application(application_report(1, "batch-test", "origin")),
        Err(BatchError::Invalid(_))
    ));
    assert_eq!(client.status().unwrap().pending_report_id, None);
    peer.join().unwrap();
}

#[test]
fn application_generation_limit_is_a_pre_stage_refusal() {
    let base = directory();
    let socket = base.path().join("server.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let peer = thread::spawn(move || {
        let (stream, _) = recv(&listener);
        send(
            stream,
            &Reply::FabricProgress {
                authority: None,
                origin: "origin".into(),
                through: 0,
            },
        );
        let (stream, _) = recv(&listener);
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
    let mut input = serde_json::to_value(application_report(1, "batch-test", "origin")).unwrap();
    input["command"] = "record_application".into();
    let reply = batch_adapter_reply(&mut client, &serde_json::to_vec(&input).unwrap());
    assert_eq!(reply["code"], "generation_capacity", "{reply}");
    assert_eq!(reply["delivery_state"], "refused_before_acceptance");
    assert_eq!(reply["durably_staged"], false);
    assert_eq!(client.status().unwrap().pending_report_id, None);
    peer.join().unwrap();
}

#[test]
fn preexisting_terminal_frame_replays_and_corrupt_digest_fails_closed() {
    // Literal field ordering is from the pre-application BatchReport/ObservationV3
    // serializers. The identity hash is independently computed with Python hashlib.
    const REQUEST: &[u8] = br#"{"report_id":"bbbb0000-0000-0000-0000-000000000001","context":{"run_id":"aaaa0000-0000-0000-0000-000000000001"},"data":{"kind":"agent_run","agent":"fixture","run_id":"aaaa0000-0000-0000-0000-000000000001","duration_ns":3,"status":"ok"},"timestamp_ns":10}"#;
    const FRAME: &[u8] = br#"{"version":3,"registry":"fabrico11y/3","context":{"run_id":"aaaa0000-0000-0000-0000-000000000001"},"identity":{"producer":"batch-test","generation":"old-generation","sequence":1},"timestamp_ns":10,"resource":"local","data":{"kind":"agent_run","agent":"fixture","run_id":"aaaa0000-0000-0000-0000-000000000001","duration_ns":3,"status":"ok"}}"#;
    const DIGEST: [u8; 32] = [
        0x9d, 0x62, 0xb6, 0xdd, 0x7f, 0xf3, 0x03, 0x8d, 0x71, 0xad, 0x46, 0xaa, 0x49, 0x2a, 0x58,
        0x6d, 0xb8, 0x33, 0x02, 0x00, 0x5f, 0x5a, 0xe4, 0x14, 0x7c, 0xdd, 0x10, 0x1e, 0xb6, 0xb7,
        0x74, 0xff,
    ];
    assert_eq!(serde_json::to_vec(&report(1)).unwrap(), REQUEST);
    assert_eq!(report(1).digest().unwrap(), DIGEST);
    let base = directory();
    let socket = base.path().join("legacy.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let peer = thread::spawn(move || {
        let (stream, frame) = recv(&listener);
        assert_eq!(frame, FRAME);
        let identity = observation::decode_versioned(FRAME)
            .unwrap()
            .identity()
            .clone();
        send(
            stream,
            &Reply::Receipt {
                receipt: Receipt {
                    id: 19,
                    identity,
                    duplicate: true,
                },
            },
        );
    });
    let mut config = Config::local(socket, "batch-test".into()).unwrap();
    config.delivery_timeout = Duration::from_secs(5);
    let path = base.path().join("old-outbox");
    let mut client = BatchClient::open(config.clone(), &path).unwrap();
    client
        .outbox
        .save_lease(&Lease {
            origin: "origin".into(),
            generation: "old-generation".into(),
            expires: wall_time().unwrap() + 3600,
            expired: false,
            sequence: 0,
        })
        .unwrap();
    client
        .outbox
        .stage(
            &Pending {
                report_id: report(1).report_id,
                digest: DIGEST,
                bytes: FRAME.to_vec(),
            },
            1,
            None,
        )
        .unwrap();
    drop(client);
    let mut client = BatchClient::open(config, &path).unwrap();
    assert_eq!(client.status().unwrap().confirmed, 1);
    assert_eq!(client.record(report(1)).unwrap().id, 19);
    peer.join().unwrap();

    let path = base.path().join("corrupt-outbox");
    let config = Config::local(base.path().join("absent.sock"), "batch-test".into()).unwrap();
    let mut corrupt = BatchClient::open(config, &path).unwrap();
    corrupt
        .outbox
        .save_lease(&Lease {
            origin: "origin".into(),
            generation: "old-generation".into(),
            expires: wall_time().unwrap() + 3600,
            expired: false,
            sequence: 0,
        })
        .unwrap();
    corrupt
        .outbox
        .stage(
            &Pending {
                report_id: report(1).report_id,
                digest: [0; 32],
                bytes: FRAME.to_vec(),
            },
            1,
            None,
        )
        .unwrap();
    assert!(matches!(
        corrupt.flush_local(Instant::now() + Duration::from_secs(1)),
        Err(BatchError::Storage(_))
    ));
    assert_eq!(
        corrupt.status().unwrap().pending_report_id.as_deref(),
        Some("bbbb0000-0000-0000-0000-000000000001")
    );
}

#[test]
fn timing_stage_rollback_and_v1_migration_preserve_unknowns() {
    let base = directory();
    let path = base.path().join("v1-outbox");
    let mut outbox = Outbox::open(&path, "batch-test", "local").unwrap();
    outbox
        .save_lease(&Lease {
            origin: "origin".into(),
            generation: "lease".into(),
            expires: wall_time().unwrap() + 3600,
            expired: false,
            sequence: 0,
        })
        .unwrap();
    let pending = |id| Pending {
        report_id: report(id).report_id,
        digest: [id as u8; 32],
        bytes: vec![1],
    };
    let completion = application_report(1, "batch-test", "origin").producer_completion;
    // The sequence guard fails after both inserts; the transaction must roll back both.
    assert!(outbox.stage(&pending(1), 2, Some(&completion)).is_err());
    assert!(outbox.find(&report(1).report_id).unwrap().is_none());
    assert!(
        outbox
            .application_timing(&report(1).report_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(outbox.lease().unwrap().unwrap().sequence, 0);
    outbox.stage(&pending(1), 1, None).unwrap();
    let receipt = Receipt {
        id: 1,
        identity: Identity {
            producer: "batch-test".into(),
            generation: "lease".into(),
            sequence: 1,
        },
        duplicate: false,
    };
    outbox.confirm(&report(1).report_id, &receipt).unwrap();
    outbox.stage(&pending(2), 2, None).unwrap();
    drop(outbox);
    // v1 is precisely the old schema, with both a confirmed and a pending row.
    let db = rusqlite::Connection::open(path.join("outbox.sqlite")).unwrap();
    db.execute_batch("DROP TABLE application_timing; PRAGMA user_version=1;")
        .unwrap();
    drop(db);
    let outbox = Outbox::open(&path, "batch-test", "local").unwrap();
    assert!(matches!(
        outbox.find(&report(1).report_id).unwrap(),
        Some(SavedReport::Confirmed(_, _))
    ));
    assert!(matches!(
        outbox.find(&report(2).report_id).unwrap(),
        Some(SavedReport::Pending(_))
    ));
    assert!(
        outbox
            .application_timing(&report(1).report_id)
            .unwrap()
            .is_none()
    );
    assert!(
        outbox
            .application_timing(&report(2).report_id)
            .unwrap()
            .is_none()
    );
    assert_eq!(outbox.lease().unwrap().unwrap().sequence, 2);
    drop(outbox);
    // A failed migration cannot mark an incompatible v1 database as migrated.
    let db = rusqlite::Connection::open(path.join("outbox.sqlite")).unwrap();
    db.execute_batch("PRAGMA user_version=1;").unwrap();
    drop(db);
    assert!(Outbox::open(&path, "batch-test", "local").is_err());
    let db = rusqlite::Connection::open(path.join("outbox.sqlite")).unwrap();
    let version: u32 = db
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 1);
    let count: u32 = db
        .query_row("SELECT count(*) FROM reports", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);
}
