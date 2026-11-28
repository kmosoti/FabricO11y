use super::*;
use fabric::{
    canonical::{self, AcceptPlan, ForwardRecord, PlanDecision, PlanProposal, PlanScope},
    observation::{Data, Identity, ObservationV3},
};
use std::{
    collections::BTreeMap,
    fs,
    net::{TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    sync::{Arc, Barrier, mpsc},
    thread,
    time::{Duration, Instant},
};

const READER: &str = "reader-secret-for-test";
const NODE: &str = "node-secret-for-test";
const WRITER: &str = "writer-secret-for-test";
const OTHER: &str = "other-writer-secret-for-test";

fn config(root: &Path) -> Config {
    let credential = |name: &str, text: &str| {
        let path = root.join(name);
        fs::write(&path, text).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    };
    Config {
        listen: "127.0.0.1:0".into(),
        storage: root.join("ledger"),
        projection: root.join("projection"),
        quota_bytes: 16 << 20,
        projection_quota_bytes: 4 << 20,
        reader_credential_file: credential("reader", READER),
        nodes: vec![NodeGrant {
            origin: "origin".into(),
            credential_file: credential("node", NODE),
        }],
        decision_writers: vec![
            DecisionWriterGrant {
                writer: "owner".into(),
                scope: "plan".into(),
                origin: "origin".into(),
                producer: "worker".into(),
                credential_file: credential("writer", WRITER),
            },
            DecisionWriterGrant {
                writer: "other".into(),
                scope: "other".into(),
                origin: "origin".into(),
                producer: "worker".into(),
                credential_file: credential("other", OTHER),
            },
        ],
        node_quota_bytes: BTreeMap::new(),
        engine_endpoint: None,
        engine_auth_token_file: None,
        allow_unauthenticated_loopback_engine: false,
        max_connections: 8,
    }
}
struct ObservedServer {
    address: String,
    stop: Arc<AtomicBool>,
    task: thread::JoinHandle<()>,
    reads: Arc<ReadState>,
    ledger: Arc<Mutex<Ledger>>,
}
fn start_observed(config: &Config) -> ObservedServer {
    let server = Server::open(config.clone()).unwrap();
    let address = server.address();
    let stop = server.stop_handle();
    let reads = server.reads.clone();
    let ledger = server.ledger.clone();
    let task = thread::spawn(move || server.serve().unwrap());
    ObservedServer {
        address,
        stop,
        task,
        reads,
        ledger,
    }
}
fn start(config: &Config) -> (String, Arc<AtomicBool>, thread::JoinHandle<()>) {
    let running = start_observed(config);
    (running.address, running.stop, running.task)
}
fn exchange(address: &str, credential: &str, request: &Request) -> Response {
    canonical::exchange(
        address,
        credential,
        request,
        Instant::now() + Duration::from_secs(10),
    )
    .unwrap()
}
fn proposal(sequence: u64, digit: char) -> String {
    let carrier = PlanProposal {
        version: 1,
        scope: PlanScope("plan".into()),
        proposal_id: digit.to_string().repeat(64),
    };
    let mut fields = serde_json::Map::new();
    fields.insert(
        canonical::PROPOSAL_MARKER.into(),
        serde_json::to_string(&carrier).unwrap().into(),
    );
    serde_json::to_string(&ObservationV3 {
        version: observation::VERSION_V3,
        registry: observation::REGISTRY_V3.into(),
        context: fabric::run::RunContext {
            run_id: "aaaa0000-0000-0000-0000-000000000001".into(),
        },
        identity: Identity {
            producer: "worker".into(),
            generation: "generation".into(),
            sequence,
        },
        timestamp_ns: 7,
        resource: "local".into(),
        data: Data::JsonLog { fields },
    })
    .unwrap()
}
fn command(authority: &str, id: &str, head: Option<i64>, source_id: u64, raw: &str) -> String {
    serde_json::to_string(&AcceptPlan {
        version: 1,
        authority: authority.into(),
        command_id: id.into(),
        scope: PlanScope("plan".into()),
        expected_head: head,
        origin: "origin".into(),
        source_id,
        proposal_raw: raw.into(),
    })
    .unwrap()
}
fn accept(address: &str, credential: &str, body: &str) -> Response {
    exchange(
        address,
        credential,
        &Request::AcceptPlan { body: body.into() },
    )
}
fn wrong_mac_writer(address: &str, request: &Request) -> Response {
    let mut stream = TcpStream::connect(address).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let nonce = canonical::random_nonce_hex().unwrap();
    let hello = canonical::ClientHello {
        version: canonical::AUTH_HANDSHAKE_VERSION,
        client_nonce: nonce.clone(),
    };
    canonical::write_frame(&mut stream, &serde_json::to_vec(&hello).unwrap(), deadline).unwrap();
    let challenge: canonical::ServerChallenge =
        serde_json::from_slice(&canonical::read_frame(&mut stream, deadline).unwrap()).unwrap();
    canonical::verify_server_challenge(WRITER, &nonce, &challenge).unwrap();
    let proof =
        canonical::build_client_proof(WRITER, &nonce, &challenge.server_nonce, b"other frame");
    canonical::write_frame(&mut stream, &serde_json::to_vec(&proof).unwrap(), deadline).unwrap();
    canonical::write_frame(&mut stream, &serde_json::to_vec(request).unwrap(), deadline).unwrap();
    serde_json::from_slice(&canonical::read_frame(&mut stream, deadline).unwrap()).unwrap()
}

fn result(response: Response) -> String {
    match response {
        Response::PlanAccepted { decision } => decision,
        other => panic!("expected accepted: {other:?}"),
    }
}
fn head(result: &str) -> i64 {
    serde_json::from_str::<PlanDecision>(result).unwrap().head
}

#[test]
fn reassigned_writer_cannot_replay_historical_scope_after_restart() {
    let scratch = std::env::var_os("TMPDIR").map(PathBuf::from).unwrap();
    let root = tempfile::tempdir_in(scratch).unwrap();
    let mut config = config(root.path());
    let (address, stop, server) = start(&config);
    let authority = match exchange(&address, READER, &Request::Progress) {
        Response::Progress { authority, .. } => authority,
        other => panic!("unexpected progress: {other:?}"),
    };
    let old = proposal(1, 'a');
    let new = proposal(2, 'b').replace("\\\"plan\\\"", "\\\"next\\\"");
    assert_ne!(new, proposal(2, 'b'));
    let ingest = Request::Ingest {
        origin: "origin".into(),
        records: vec![
            ForwardRecord {
                source_id: 1,
                raw: old.clone(),
            },
            ForwardRecord {
                source_id: 2,
                raw: new.clone(),
            },
        ],
    };
    assert!(matches!(
        exchange(&address, NODE, &ingest),
        Response::Committed { through: 2, .. }
    ));
    let old_body = command(&authority, "old", None, 1, &old);
    let _saved = result(accept(&address, WRITER, &old_body));
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();

    config.decision_writers[0].scope = "next".into();
    let (address, stop, server) = start(&config);
    assert!(matches!(
        accept(&address, WRITER, &old_body),
        Response::Error { .. }
    ));
    let mut current: AcceptPlan =
        serde_json::from_str(&command(&authority, "new", None, 2, &new)).unwrap();
    current.scope = PlanScope("next".into());
    let body = serde_json::to_string(&current).unwrap();
    let current_result = result(accept(&address, WRITER, &body));
    assert_eq!(result(accept(&address, WRITER, &body)), current_result);
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
}

#[test]
fn rotated_credentials_reauthenticate_existing_policy_after_restart() {
    let scratch = std::env::var_os("TMPDIR").map(PathBuf::from).unwrap();
    let root = tempfile::tempdir_in(scratch).unwrap();
    let config = config(root.path());
    let (address, stop, server) = start(&config);
    let authority = match exchange(&address, READER, &Request::Progress) {
        Response::Progress { authority, .. } => authority,
        other => panic!("unexpected progress: {other:?}"),
    };
    let raw = proposal(1, 'a');
    let ingest = Request::Ingest {
        origin: "origin".into(),
        records: vec![ForwardRecord {
            source_id: 1,
            raw: raw.clone(),
        }],
    };
    assert!(matches!(
        exchange(&address, NODE, &ingest),
        Response::Committed { through: 1, .. }
    ));
    let body = command(&authority, "rotate", None, 1, &raw);
    let saved = result(accept(&address, WRITER, &body));
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();

    const NEW_READER: &str = "replacement-reader-secret";
    const NEW_NODE: &str = "replacement-node-secret";
    const NEW_WRITER: &str = "replacement-writer-secret";
    fs::write(&config.reader_credential_file, NEW_READER).unwrap();
    fs::write(&config.nodes[0].credential_file, NEW_NODE).unwrap();
    fs::write(&config.decision_writers[0].credential_file, NEW_WRITER).unwrap();
    let (address, stop, server) = start(&config);
    for (secret, request) in [
        (READER, Request::Progress),
        (NODE, Request::Progress),
        (WRITER, Request::AcceptPlan { body: body.clone() }),
    ] {
        assert!(
            canonical::exchange(
                &address,
                secret,
                &request,
                Instant::now() + Duration::from_secs(5)
            )
            .is_err()
        );
    }
    assert!(matches!(exchange(&address, NEW_READER, &Request::Progress),
        Response::Progress { authority: actual, .. } if actual == authority));
    assert!(matches!(
        exchange(&address, NEW_NODE, &ingest),
        Response::Committed { through: 1, .. }
    ));
    assert_eq!(result(accept(&address, NEW_WRITER, &body)), saved);
    assert!(matches!(
        accept(&address, NEW_READER, &body),
        Response::Error { .. }
    ));
    assert!(matches!(
        exchange(&address, NEW_WRITER, &Request::Progress),
        Response::Error { .. }
    ));
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
}

#[test]
fn competing_acceptance_lost_reply_new_head_and_restart() {
    transaction_fixture();
}

#[test]
#[ignore = "requires a collected unit in fabrico11y.slice"]
fn competing_acceptance_confined_qualification() {
    assert!(
        fs::read_to_string("/proc/self/cgroup")
            .unwrap()
            .contains("/fabrico11y.slice/")
    );
    transaction_fixture();
}

fn transaction_fixture() {
    let scratch = std::env::var_os("TMPDIR").map(PathBuf::from).unwrap();
    let root = tempfile::tempdir_in(scratch).unwrap();
    let config = config(root.path());
    let (address, stop, server) = start(&config);
    let authority = match exchange(&address, READER, &Request::Progress) {
        Response::Progress { authority, .. } => authority,
        other => panic!("unexpected progress: {other:?}"),
    };
    let a = proposal(1, 'a');
    let b = proposal(2, 'b');
    let ingest = Request::Ingest {
        origin: "origin".into(),
        records: vec![
            ForwardRecord {
                source_id: 1,
                raw: a.clone(),
            },
            ForwardRecord {
                source_id: 2,
                raw: b.clone(),
            },
        ],
    };
    assert!(matches!(
        exchange(&address, NODE, &ingest),
        Response::Committed { through: 2, .. }
    ));
    let a_cmd = command(&authority, "race-a", None, 1, &a);
    let b_cmd = command(&authority, "race-b", None, 2, &b);
    // A writer's configured credential grants only AcceptPlan, and its
    // handshake proof cannot authorize a differently MACed request frame.
    for request in [
        Request::Progress,
        Request::Capabilities,
        Request::Ingest {
            origin: "origin".into(),
            records: vec![],
        },
    ] {
        assert!(matches!(
            exchange(&address, WRITER, &request),
            Response::Error { .. }
        ));
    }
    let tampered = Request::AcceptPlan {
        body: a_cmd.clone(),
    };
    assert!(
        matches!(wrong_mac_writer(&address, &tampered), Response::Error { error } if error == "unauthorized")
    );
    for token in [READER, NODE, OTHER] {
        assert!(matches!(
            accept(&address, token, &a_cmd),
            Response::Error { .. }
        ));
    }
    let mut wrong: AcceptPlan = serde_json::from_str(&a_cmd).unwrap();
    wrong.scope = PlanScope("other".into());
    assert!(matches!(
        accept(&address, WRITER, &serde_json::to_string(&wrong).unwrap()),
        Response::Error { .. }
    ));
    let mut changed: AcceptPlan = serde_json::from_str(&a_cmd).unwrap();
    changed.proposal_raw.push(' ');
    assert!(matches!(
        accept(&address, WRITER, &serde_json::to_string(&changed).unwrap()),
        Response::Error { .. }
    ));
    let mut wrong_record: AcceptPlan = serde_json::from_str(&a_cmd).unwrap();
    wrong_record.proposal_raw = b.clone();
    assert!(
        matches!(accept(&address, WRITER, &serde_json::to_string(&wrong_record).unwrap()), Response::Error{error} if error == "canonical proposal mismatch")
    );
    let db = rusqlite::Connection::open(config.storage.join("canonical.sqlite")).unwrap();
    // Simulate a locally committed row whose later projection is not complete.
    db.execute(
        "UPDATE records SET published=0 WHERE origin='origin' AND source_id=1",
        [],
    )
    .unwrap();
    db.execute_batch("CREATE TRIGGER abort_plan BEFORE UPDATE OF head ON plan_scope BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    assert!(matches!(
        accept(&address, WRITER, &a_cmd),
        Response::Error { error } if error.contains("injected")
    ));
    let count: u64 = db
        .query_row("SELECT count(*) FROM plan_decision", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    let initial_head: Option<i64> = db
        .query_row("SELECT head FROM plan_scope WHERE scope='plan'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(initial_head.is_none());
    db.execute_batch("DROP TRIGGER abort_plan; UPDATE records SET published=1 WHERE origin='origin' AND source_id=1;").unwrap();
    drop(db);
    let barrier = Arc::new(Barrier::new(3));
    let mut contenders = Vec::new();
    for body in [a_cmd.clone(), b_cmd.clone()] {
        let address = address.clone();
        let barrier = barrier.clone();
        contenders.push(thread::spawn(move || {
            barrier.wait();
            accept(&address, WRITER, &body)
        }));
    }
    barrier.wait();
    let outcomes: Vec<_> = contenders.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| matches!(r, Response::PlanAccepted { .. }))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| matches!(r,Response::Error{error} if error == "stale plan head"))
            .count(),
        1
    );
    let winning = outcomes
        .into_iter()
        .find(|r| matches!(r, Response::PlanAccepted { .. }))
        .unwrap();
    let first_head = head(&result(winning));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_address = listener.local_addr().unwrap().to_string();
    let target = address.clone();
    let (sent, received) = mpsc::sync_channel(1);
    let proxy = thread::spawn(move || {
        let (mut client, _) = listener.accept().unwrap();
        let mut server = TcpStream::connect(target).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        for forward in [true, false, true, true] {
            let bytes = if forward {
                canonical::read_frame(&mut client, deadline).unwrap()
            } else {
                canonical::read_frame(&mut server, deadline).unwrap()
            };
            if forward {
                canonical::write_frame(&mut server, &bytes, deadline).unwrap();
            } else {
                canonical::write_frame(&mut client, &bytes, deadline).unwrap();
            }
        }
        let response = canonical::read_frame(&mut server, deadline).unwrap();
        sent.send(response).unwrap();
    });
    let lost_cmd = command(&authority, "lost", Some(first_head), 1, &a);
    let lost = canonical::exchange(
        &proxy_address,
        WRITER,
        &Request::AcceptPlan {
            body: lost_cmd.clone(),
        },
        Instant::now() + Duration::from_secs(10),
    );
    assert!(lost.is_err());
    let raw_response = received.recv_timeout(Duration::from_secs(10)).unwrap();
    proxy.join().unwrap();
    let saved = result(serde_json::from_slice(&raw_response).unwrap());
    let saved_head = head(&saved);
    let newer = result(accept(
        &address,
        WRITER,
        &command(&authority, "newer", Some(saved_head), 2, &b),
    ));
    let newest_head = head(&newer);
    assert!(newest_head > saved_head);
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
    let (address, stop, server) = start(&config);
    assert_eq!(result(accept(&address, WRITER, &lost_cmd)), saved);
    // A different SQLite connection commits substitutions after the server
    // opened. Exact retry must recheck the durable fence and full policy.
    let policy_db = rusqlite::Connection::open(config.storage.join("canonical.sqlite")).unwrap();
    policy_db
        .execute(
            "UPDATE meta SET value='substituted' WHERE key='authority'",
            [],
        )
        .unwrap();
    assert!(
        matches!(accept(&address, WRITER, &lost_cmd), Response::Error { error } if error == "canonical authority mismatch")
    );
    policy_db
        .execute(
            "UPDATE meta SET value=?1 WHERE key='authority'",
            [&authority],
        )
        .unwrap();
    for (field, bad, original) in [
        ("owner", "other", "owner"),
        ("origin", "other-origin", "origin"),
        ("producer", "other-producer", "worker"),
    ] {
        let sql = format!("UPDATE plan_scope SET {field}=?1 WHERE scope='plan'");
        policy_db.execute(&sql, [bad]).unwrap();
        assert!(
            matches!(accept(&address, WRITER, &lost_cmd), Response::Error { .. }),
            "{field}"
        );
        policy_db.execute(&sql, [original]).unwrap();
        assert_eq!(
            result(accept(&address, WRITER, &lost_cmd)),
            saved,
            "{field}"
        );
    }
    // A second connection holds SQLite's write reservation with an uncommitted
    // policy change. The acceptance transaction cannot return a saved result.
    policy_db
        .execute_batch(
            "BEGIN IMMEDIATE; UPDATE plan_scope SET producer='other-producer' WHERE scope='plan';",
        )
        .unwrap();
    assert!(
        matches!(accept(&address, WRITER, &lost_cmd), Response::Error { error } if error.contains("database is locked"))
    );
    policy_db.execute_batch("COMMIT;").unwrap();
    assert!(matches!(
        accept(&address, WRITER, &lost_cmd),
        Response::Error { .. }
    ));
    policy_db
        .execute(
            "UPDATE plan_scope SET producer='worker' WHERE scope='plan'",
            [],
        )
        .unwrap();
    assert_eq!(result(accept(&address, WRITER, &lost_cmd)), saved);
    drop(policy_db);
    let mut altered: AcceptPlan = serde_json::from_str(&lost_cmd).unwrap();
    altered.proposal_raw = b.clone();
    altered.source_id = 2;
    assert!(
        matches!(accept(&address,WRITER,&serde_json::to_string(&altered).unwrap()),Response::Error{error} if error == "plan command conflict")
    );
    for (raw, id) in [(&a, 1_u64), (&b, 2)] {
        let observation = observation::decode_versioned(raw.as_bytes()).unwrap();
        assert!(
            matches!(exchange(&address,NODE,&Request::Lookup{origin:"origin".into(),identity:observation.identity().clone()}),Response::Found{row:Some(row)} if row.source_id==id)
        );
    }
    let db = rusqlite::Connection::open(config.storage.join("canonical.sqlite")).unwrap();
    let count: u64 = db
        .query_row("SELECT count(*) FROM plan_decision", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 3);
    let head: i64 = db
        .query_row("SELECT head FROM plan_scope WHERE scope='plan'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(head, newest_head);
    db.execute_batch("WITH RECURSIVE n(i) AS (VALUES(4) UNION ALL SELECT i+1 FROM n WHERE i<4096) INSERT INTO plan_decision(id,scope,writer,command_id,request_hash,previous_head,proposal_id,result) SELECT i,'plan','owner',printf('filler-%d',i),'hash',NULL,'proposal','{}' FROM n;").unwrap();
    drop(db);
    assert!(
        matches!(accept(&address, WRITER, &command(&authority, "full", Some(newest_head), 1, &a)), Response::Error { error } if error == "plan decision capacity exhausted")
    );
    assert_eq!(result(accept(&address, WRITER, &lost_cmd)), saved);
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
}

#[test]
fn segment_tickets_bind_real_reader_and_verified_resume() {
    let scratch = std::env::var_os("TMPDIR").unwrap();
    let root = tempfile::tempdir_in(scratch).unwrap();
    let config = config(root.path());
    let server = Server::open(config.clone()).unwrap();
    let raw = proposal(1, 'a');
    server
        .ledger
        .lock()
        .unwrap()
        .ingest(
            "origin",
            &[ForwardRecord {
                source_id: 1,
                raw: raw.clone(),
            }],
        )
        .unwrap();
    server
        .projection
        .lock()
        .unwrap()
        .apply(&[canonical::CanonicalRow {
            canonical_id: 1,
            origin: "origin".into(),
            source_id: 1,
            raw,
        }])
        .unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(config.projection.join("manifest.json")).unwrap())
            .unwrap();
    let segment = &manifest["manifest"]["segments"][0];
    let digest = segment["digest"].as_str().unwrap().to_owned();
    let bytes = fs::read(config.projection.join(segment["file"].as_str().unwrap())).unwrap();
    let address = server.address();
    let stop = server.stop_handle();
    let task = thread::spawn(move || server.serve().unwrap());
    let authority = match exchange(&address, READER, &Request::Progress) {
        Response::Progress { authority, .. } => authority,
        other => panic!("{other:?}"),
    };
    let issue = |origin: &str| Request::IssueSegmentTicket {
        version: 1,
        authority: authority.clone(),
        origin: origin.into(),
        digest: digest.clone(),
        start: 0,
        end: bytes.len() as u64,
    };
    assert!(matches!(
        exchange(&address, NODE, &issue("origin")),
        Response::Error { .. }
    ));
    assert!(matches!(
        exchange(&address, READER, &issue("unknown")),
        Response::Error { .. }
    ));
    let ticket = match exchange(&address, READER, &issue("origin")) {
        Response::SegmentTicket { ticket } => ticket,
        other => panic!("{other:?}"),
    };
    let split = bytes.len().min(100);
    let read =
        |ticket: canonical::SegmentTicket, offset: u64, length: u32| Request::ReadSegmentChunk {
            ticket,
            offset,
            length,
        };
    let first = exchange(&address, READER, &read(ticket.clone(), 0, split as u32));
    assert!(matches!(first, Response::SegmentChunk { offset: 0, bytes: b } if b == bytes[..split]));
    if split < bytes.len() {
        let len = (bytes.len() - split).min(100);
        let resumed = exchange(
            &address,
            READER,
            &read(ticket.clone(), split as u64, len as u32),
        );
        assert!(
            matches!(resumed, Response::SegmentChunk { offset, bytes: b } if offset == split as u64 && b == bytes[split..split + len])
        );
    }
    let mut tampered = ticket.clone();
    tampered.digest = "b".repeat(64);
    assert!(matches!(
        exchange(&address, READER, &read(tampered, 0, 1)),
        Response::Error { .. }
    ));
    assert!(matches!(
        exchange(
            &address,
            READER,
            &read(ticket.clone(), bytes.len() as u64, 1)
        ),
        Response::Error { .. }
    ));
    assert!(matches!(
        exchange(&address, NODE, &read(ticket.clone(), 0, 1)),
        Response::Error { .. }
    ));
    stop.store(true, Ordering::SeqCst);
    task.join().unwrap();
    let (address, stop, task) = start(&config);
    assert!(matches!(
        exchange(&address, READER, &read(ticket, 0, 1)),
        Response::Error { .. }
    ));
    stop.store(true, Ordering::SeqCst);
    task.join().unwrap();
}

#[test]
fn cursor_routes_fence_configured_and_durable_policy_after_restart() {
    use canonical::PlanPageRequest;
    assert!(
        fs::read_to_string("/proc/self/cgroup")
            .unwrap()
            .contains("/fabrico11y.slice/")
    );
    let scratch = std::env::var_os("TMPDIR").map(PathBuf::from).unwrap();
    let root = tempfile::tempdir_in(scratch).unwrap();
    let mut config = config(root.path());
    let (address, stop, server) = start(&config);
    let authority = match exchange(&address, READER, &Request::Progress) {
        Response::Progress { authority, .. } => authority,
        other => panic!("{other:?}"),
    };
    let raw = proposal(1, 'a');
    assert!(matches!(
        exchange(
            &address,
            NODE,
            &Request::Ingest {
                origin: "origin".into(),
                records: vec![ForwardRecord {
                    source_id: 1,
                    raw: raw.clone()
                }]
            }
        ),
        Response::Committed { .. }
    ));
    let saved = result(accept(
        &address,
        WRITER,
        &command(&authority, "one", None, 1, &raw),
    ));
    let second = result(accept(
        &address,
        WRITER,
        &command(&authority, "two", Some(head(&saved)), 1, &raw),
    ));
    let scope = PlanScope("plan".into());
    let make_page = |scope: PlanScope, cursor: Option<String>| Request::PlanPage {
        request: PlanPageRequest {
            scope,
            cursor,
            limit: 1,
            ttl_secs: 900,
        },
    };
    let token = match exchange(&address, WRITER, &make_page(scope.clone(), None)) {
        Response::PlanPage { page } => page.next_cursor.unwrap(),
        other => panic!("{other:?}"),
    };
    let subscription = match exchange(
        &address,
        WRITER,
        &Request::SubscribePlan {
            scope: scope.clone(),
        },
    ) {
        Response::PlanSubscribed { subscription, .. } => subscription,
        other => panic!("{other:?}"),
    };
    let routes = || {
        [
            make_page(scope.clone(), Some(token.clone())),
            Request::SubscribePlan {
                scope: scope.clone(),
            },
            Request::PollPlanHint {
                scope: scope.clone(),
                subscription: subscription.clone(),
            },
            Request::RevokePlanCursors {
                scope: scope.clone(),
            },
        ]
    };
    let db = rusqlite::Connection::open(config.storage.join("canonical.sqlite")).unwrap();
    for (field, bad, original) in [
        ("owner", "other", "owner"),
        ("origin", "changed", "origin"),
        ("producer", "changed", "worker"),
    ] {
        let sql = format!("UPDATE plan_scope SET {field}=?1 WHERE scope='plan'");
        db.execute(&sql, [bad]).unwrap();
        for request in routes() {
            assert!(
                matches!(exchange(&address, WRITER, &request), Response::Error { .. }),
                "{field}: {request:?}"
            );
        }
        db.execute(&sql, [original]).unwrap();
    }
    db.execute("UPDATE meta SET value='changed' WHERE key='authority'", [])
        .unwrap();
    for request in routes() {
        assert!(
            matches!(exchange(&address, WRITER, &request), Response::Error { .. }),
            "authority: {request:?}"
        );
    }
    db.execute(
        "UPDATE meta SET value=?1 WHERE key='authority'",
        [&authority],
    )
    .unwrap();
    assert!(matches!(
        exchange(
            &address,
            WRITER,
            &make_page(scope.clone(), Some(token.clone()))
        ),
        Response::PlanPage { .. }
    ));
    assert_eq!(
        result(accept(
            &address,
            WRITER,
            &command(&authority, "two", Some(head(&saved)), 1, &raw)
        )),
        second
    );
    drop(db);
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
    config.decision_writers[0].scope = "next".into();
    let (address, stop, server) = start(&config);
    for request in routes() {
        assert!(
            matches!(exchange(&address, WRITER, &request), Response::Error { .. }),
            "reassigned: {request:?}"
        );
    }
    assert!(matches!(
        exchange(&address, WRITER, &make_page(PlanScope("next".into()), None)),
        Response::PlanPage { .. }
    ));
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
}

#[test]
fn scoped_cursor_survives_restart_and_hints_coalesce() {
    use canonical::{PlanCursorFailure as Failure, PlanPageRequest};
    assert!(
        fs::read_to_string("/proc/self/cgroup")
            .unwrap()
            .contains("/fabrico11y.slice/")
    );
    let scratch = std::env::var_os("TMPDIR").map(PathBuf::from).unwrap();
    let root = tempfile::tempdir_in(scratch).unwrap();
    let config = config(root.path());
    let ObservedServer {
        address,
        stop,
        task: server,
        reads,
        ledger,
    } = start_observed(&config);
    let authority = match exchange(&address, READER, &Request::Progress) {
        Response::Progress { authority, .. } => authority,
        other => panic!("{other:?}"),
    };
    let a = proposal(1, 'a');
    assert!(matches!(
        exchange(
            &address,
            NODE,
            &Request::Ingest {
                origin: "origin".into(),
                records: vec![ForwardRecord {
                    source_id: 1,
                    raw: a.clone()
                }]
            }
        ),
        Response::Committed { .. }
    ));
    let scope = PlanScope("plan".into());
    let subscribe = Request::SubscribePlan {
        scope: scope.clone(),
    };
    for credential in [READER, NODE, OTHER] {
        assert!(matches!(
            exchange(&address, credential, &subscribe),
            Response::Error { .. }
        ));
    }
    // Block registration at the hint mutex after it has captured the frontier.
    // The following real acceptance must wait for that registration, so its
    // signal cannot pass between frontier capture and subscription insertion.
    let hints = reads.plan_hints.lock().unwrap();
    let sub_addr = address.clone();
    let sub_scope = scope.clone();
    let subscribing = thread::spawn(move || {
        exchange(
            &sub_addr,
            WRITER,
            &Request::SubscribePlan { scope: sub_scope },
        )
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if ledger.try_lock().is_err() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "subscription did not acquire ledger"
        );
        thread::sleep(Duration::from_millis(2));
    }
    let accept_addr = address.clone();
    let accept_cmd = command(&authority, "cursor-race", None, 1, &a);
    let (done_tx, done_rx) = mpsc::channel();
    let accepting = thread::spawn(move || {
        let response = accept(&accept_addr, WRITER, &accept_cmd);
        done_tx.send(response).unwrap();
    });
    assert!(
        done_rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "commit crossed registration"
    );
    drop(hints);
    let subscription = match subscribing.join().unwrap() {
        Response::PlanSubscribed {
            subscription,
            frontier,
        } => {
            assert_eq!(frontier, 0);
            subscription
        }
        other => panic!("{other:?}"),
    };
    let race = result(done_rx.recv_timeout(Duration::from_secs(5)).unwrap());
    accepting.join().unwrap();
    let mut head_id = Some(head(&race));
    for id in 0..2 {
        let accepted = result(accept(
            &address,
            WRITER,
            &command(&authority, &format!("cursor-{id}"), head_id, 1, &a),
        ));
        head_id = Some(head(&accepted));
    }
    let poll = || Request::PollPlanHint {
        scope: scope.clone(),
        subscription: subscription.clone(),
    };
    assert!(matches!(
        exchange(&address, WRITER, &poll()),
        Response::PlanHint { pending: true }
    ));
    assert!(matches!(
        exchange(&address, WRITER, &poll()),
        Response::PlanHint { pending: false }
    ));
    let page = |cursor| Request::PlanPage {
        request: PlanPageRequest {
            scope: scope.clone(),
            cursor,
            limit: 1,
            ttl_secs: 900,
        },
    };
    let (first, token, frontier) = match exchange(&address, WRITER, &page(None)) {
        Response::PlanPage { page } => {
            assert_eq!(page.decisions.len(), 1);
            (
                page.decisions[0].head,
                page.next_cursor.unwrap(),
                page.frontier,
            )
        }
        other => panic!("{other:?}"),
    };
    assert!(matches!(
        exchange(&address, OTHER, &page(Some(token.clone()))),
        Response::Error { .. }
    ));
    let changed = result(accept(
        &address,
        WRITER,
        &command(&authority, "after-frontier", head_id, 1, &a),
    ));
    assert!(head(&changed) > frontier);
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
    drop(ledger);
    drop(reads);
    let (address, stop, server) = start(&config);
    assert!(matches!(
        exchange(&address, WRITER, &poll()),
        Response::PlanCursorFailure {
            reason: Failure::Revoked
        }
    ));
    let second = match exchange(&address, WRITER, &page(Some(token.clone()))) {
        Response::PlanPage { page } => {
            assert_eq!(page.frontier, frontier);
            assert!(page.decisions[0].head > first);
            page.next_cursor.unwrap()
        }
        other => panic!("{other:?}"),
    };
    assert!(
        matches!(exchange(&address,WRITER,&page(Some(second))),Response::PlanPage{page} if page.decisions.len()==1 && page.next_cursor.is_none() && page.decisions[0].head==frontier)
    );
    let db = rusqlite::Connection::open(config.storage.join("canonical.sqlite")).unwrap();
    let key: String = db
        .query_row(
            "SELECT value FROM meta WHERE key='plan_cursor_key'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let expired = crate::plan_cursor::encode(
        &crate::plan_cursor::Cursor {
            epoch: 0,
            frontier,
            count: 3,
            last: first,
            expires: 0,
        },
        &key,
        &authority,
        "owner",
        "plan",
    )
    .unwrap();
    assert!(matches!(
        exchange(&address, WRITER, &page(Some(expired))),
        Response::PlanCursorFailure {
            reason: Failure::Expired
        }
    ));
    let mut tampered = token.clone();
    tampered.push('0');
    assert!(matches!(
        exchange(&address, WRITER, &page(Some(tampered))),
        Response::PlanCursorFailure {
            reason: Failure::Invalid
        }
    ));
    for _ in 0..32 {
        assert!(matches!(
            exchange(&address, WRITER, &subscribe),
            Response::PlanSubscribed { .. }
        ));
    }
    assert!(matches!(
        exchange(&address, WRITER, &subscribe),
        Response::PlanCursorFailure {
            reason: Failure::Capacity
        }
    ));
    db.execute("DELETE FROM plan_decision WHERE id=?1", [frontier])
        .unwrap();
    assert!(matches!(
        exchange(&address, WRITER, &page(Some(token.clone()))),
        Response::PlanCursorFailure {
            reason: Failure::Gap
        }
    ));
    assert!(matches!(
        exchange(
            &address,
            WRITER,
            &Request::RevokePlanCursors {
                scope: scope.clone()
            }
        ),
        Response::PlanCursorsRevoked
    ));
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
    drop(db);
    let (address, stop, server) = start(&config);
    assert!(matches!(
        exchange(&address, WRITER, &page(Some(token))),
        Response::PlanCursorFailure {
            reason: Failure::Revoked
        }
    ));
    stop.store(true, Ordering::SeqCst);
    server.join().unwrap();
}
