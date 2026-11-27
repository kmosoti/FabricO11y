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
fn start(config: &Config) -> (String, Arc<AtomicBool>, thread::JoinHandle<()>) {
    let server = Server::open(config.clone()).unwrap();
    let address = server.address();
    let stop = server.stop_handle();
    let task = thread::spawn(move || server.serve().unwrap());
    (address, stop, task)
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
fn competing_acceptance_lost_reply_new_head_and_restart() {
    assert!(
        fs::read_to_string("/proc/self/cgroup")
            .unwrap()
            .contains("/fabrico11y.slice/")
    );
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
