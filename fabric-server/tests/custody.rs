//! Golden wire fixtures and independent canonical custody boundaries.
use fabric::{
    canonical::{self, ForwardRecord, Request, Response},
    query::{Budgets, Filter, Operation, QueryRequest},
};
use fabric_server::{Config, NodeGrant, Server, ledger::Ledger};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
const RAW: &str = r#" { "version":2,"registry":"fabrico11y/2","identity":{"producer":"golden","generation":"a","sequence":18446744073709551615},"timestamp_ns":18446744073709551615,"resource":"service","data":{"kind":"metric","name":"finite","unit":"1","value":-0.0,"metric_type":"gauge","reset":false}} "#;
fn raw(n: u64) -> String {
    format!(
        r#"{{"version":2,"registry":"fabrico11y/2","identity":{{"producer":"fixture","generation":"generation","sequence":{n}}},"timestamp_ns":{n},"resource":"svc","data":{{"kind":"tool_execution","tool":"test","run_id":"same-local-name","duration_ns":18446744073709551615,"status":"ok"}}}}"#
    )
}
fn row(id: u64, raw: String) -> ForwardRecord {
    ForwardRecord { source_id: id, raw }
}
#[test]
fn original_bytes_conflicts_atomic_batches_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("canonical");
    let mut ledger = Ledger::open(&state, 8 << 20).unwrap();
    let authority = ledger.authority.clone();
    let first = row(1, RAW.into());
    assert_eq!(
        ledger
            .ingest("alpha", std::slice::from_ref(&first))
            .unwrap(),
        1
    );
    assert_eq!(
        ledger
            .ingest("alpha", std::slice::from_ref(&first))
            .unwrap(),
        1
    );
    assert!(
        ledger
            .ingest("alpha", &[row(2, raw(2)), row(4, raw(4))])
            .is_err()
    );
    assert_eq!(ledger.count().unwrap(), 1);
    assert!(ledger.ingest("alpha", &[row(2, RAW.into())]).is_err());
    assert_eq!(ledger.frontiers().unwrap()["alpha"], 1);
    assert!(
        ledger
            .ingest("alpha", &[row(1, RAW.trim().into())])
            .is_err()
    );
    assert_eq!(ledger.ingest("beta", &[row(1, RAW.into())]).unwrap(), 1);
    assert_eq!(
        ledger
            .ingest("alpha", &[row(2, raw(2)), row(2, raw(2))])
            .unwrap(),
        2
    );
    assert_eq!(ledger.replay(0, 8192, 8 << 20).unwrap().len(), 3);
    assert_eq!(ledger.read("alpha", 0, 1, 32).unwrap()[0].raw, RAW);
    drop(ledger);
    let ledger = Ledger::open(&state, 8 << 20).unwrap();
    assert_eq!(ledger.authority, authority);
    assert_eq!(ledger.evidence(1).unwrap().unwrap().raw, RAW);
    let observation =
        fabric::observation::decode_versioned(ledger.evidence(1).unwrap().unwrap().raw.as_bytes())
            .unwrap();
    if let fabric::observation::Data::Metric { value, .. } = observation.data() {
        assert_eq!(value.to_bits(), (-0.0f64).to_bits())
    } else {
        panic!("golden shape")
    }
    assert!(
        Ledger::open(&state, 8 << 20).is_err(),
        "exclusive canonical owner"
    );
}
#[test]
fn duplicate_retry_still_works_at_quota() {
    let dir = tempfile::tempdir().unwrap();
    let mut ledger = Ledger::open(&dir.path().join("canonical"), 1 << 20).unwrap();
    let mut first = None;
    let mut refused = false;
    for sequence in 1..100 {
        let value=serde_json::json!({"version":2,"registry":"fabrico11y/2","identity":{"producer":"quota","generation":"fixed","sequence":sequence},"timestamp_ns":sequence,"resource":"svc","data":{"kind":"log","level":"info","message":"x".repeat(32768)}}).to_string();
        let record = row(sequence, value);
        match ledger.ingest("node", std::slice::from_ref(&record)) {
            Ok(_) => {
                first.get_or_insert(record);
            }
            Err(_) => {
                refused = true;
                break;
            }
        }
    }
    assert!(refused);
    let first = first.unwrap();
    let through = ledger.frontiers().unwrap()["node"];
    assert_eq!(ledger.ingest("node", &[first]).unwrap(), through);
    assert!(ledger.disk_bytes().unwrap() <= 1 << 20);
}
fn key(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, format!("test-only-private-{name}-credential")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}
struct Receiver {
    config: Config,
    endpoint: String,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<Result<(), String>>>,
}
impl Receiver {
    fn new(root: &Path, origins: &[&str]) -> Self {
        let config = Config {
            listen: "127.0.0.1:0".into(),
            storage: root.join("canonical"),
            projection: root.join("projection"),
            quota_bytes: 32 << 20,
            projection_quota_bytes: 32 << 20,
            reader_credential_file: key(root, "reader"),
            nodes: origins
                .iter()
                .map(|origin| NodeGrant {
                    origin: (*origin).into(),
                    credential_file: key(root, origin),
                })
                .collect(),
            engine_endpoint: None,
            max_connections: 16,
        };
        Self::start(config)
    }
    fn start(config: Config) -> Self {
        let server = Server::open(config.clone()).unwrap();
        let endpoint = server.address();
        let stop = server.stop_handle();
        let worker = thread::spawn(move || server.serve());
        Self {
            config,
            endpoint,
            stop,
            worker: Some(worker),
        }
    }
    fn token(&self, name: &str) -> String {
        fs::read_to_string(if name == "reader" {
            self.config.reader_credential_file.clone()
        } else {
            self.config
                .nodes
                .iter()
                .find(|n| n.origin == name)
                .unwrap()
                .credential_file
                .clone()
        })
        .unwrap()
    }
    fn call(&self, name: &str, request: Request) -> Response {
        canonical::exchange(
            &self.endpoint,
            &self.token(name),
            &request,
            Instant::now() + Duration::from_secs(12),
        )
        .unwrap()
    }
    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(w) = self.worker.take() {
            w.join().unwrap().unwrap();
        }
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        self.shutdown()
    }
}
fn query(scope: Vec<String>) -> QueryRequest {
    QueryRequest {
        version: 1,
        query_id: "golden-query".into(),
        authority: None,
        scope,
        snapshot: Default::default(),
        filter: Filter::default(),
        operation: Operation::ListRuns,
        budgets: Budgets::default(),
        cursor: None,
    }
}
#[test]
fn authenticated_origin_scope_precedes_aggregation_and_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let server = Receiver::new(dir.path(), &["alpha", "beta"]);
    assert!(matches!(
        server.call(
            "alpha",
            Request::Ingest {
                origin: "beta".into(),
                records: vec![row(1, raw(1))]
            }
        ),
        Response::Error { .. }
    ));
    for origin in ["alpha", "beta"] {
        assert!(matches!(
            server.call(
                origin,
                Request::Ingest {
                    origin: origin.into(),
                    records: vec![row(1, raw(1))]
                }
            ),
            Response::Committed { through: 1, .. }
        ));
    }
    let q = query(vec!["alpha".into()]);
    let result = server.call("reader", Request::Query { request: q.clone() });
    let Response::Query { response } = result else {
        panic!("{result:?}")
    };
    assert_eq!(response.rows.len(), 1);
    assert_eq!(response.coverage.scope, vec!["alpha"]);
    assert!(response.coverage.complete);
    assert_eq!(response.coverage.snapshot.len(), 1);
    assert!(
        !serde_json::to_string(&response.coverage)
            .unwrap()
            .contains("beta")
    );
    assert!(matches!(
        server.call("alpha", Request::Query { request: q }),
        Response::Error { .. }
    ));
    assert!(matches!(
        server.call(
            "reader",
            Request::Query {
                request: query(vec!["ungranted".into()])
            }
        ),
        Response::Error { .. }
    ));
    assert!(matches!(
        server.call(
            "alpha",
            Request::Read {
                origin: "beta".into(),
                after: 0,
                through: 1,
                limit: 32
            }
        ),
        Response::Error { .. }
    ));
    let Response::Query { response } = server.call(
        "reader",
        Request::Query {
            request: query(vec!["alpha".into(), "beta".into()]),
        },
    ) else {
        panic!("all scopes")
    };
    assert_eq!(
        response.rows.len(),
        2,
        "legacy run names must not globally join"
    );
}
#[test]
fn projection_write_failure_preserves_canonical_admission_and_readback() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = Receiver::new(dir.path(), &["alpha"]);
    let collision = server.config.projection.join("staging-segment-1-0.parquet");
    fs::write(&collision, b"owned interrupted-write fixture").unwrap();
    assert!(matches!(
        server.call(
            "alpha",
            Request::Ingest {
                origin: "alpha".into(),
                records: vec![row(1, raw(1))]
            }
        ),
        Response::Committed { .. }
    ));
    let result = server.call(
        "reader",
        Request::Query {
            request: query(vec!["alpha".into()]),
        },
    );
    fs::remove_file(collision).unwrap();
    assert!(
        matches!(result,Response::Error{ref error} if error.contains("analytical projection unavailable")),
        "{result:?}"
    );
    assert!(matches!(
        server.call(
            "alpha",
            Request::Ingest {
                origin: "alpha".into(),
                records: vec![row(2, raw(2))]
            }
        ),
        Response::Committed { through: 2, .. }
    ));
    let Response::Rows { rows } = server.call(
        "alpha",
        Request::Read {
            origin: "alpha".into(),
            after: 0,
            through: 2,
            limit: 32,
        },
    ) else {
        panic!("retained history")
    };
    assert_eq!(rows.len(), 2);
    // This fixture deliberately leaves a persistent projector error.
    server.stop.store(true, Ordering::SeqCst);
    let error = server.worker.take().unwrap().join().unwrap().unwrap_err();
    assert_eq!(
        error,
        std::io::Error::from_raw_os_error(rustix::io::Errno::EXIST.raw_os_error()).to_string()
    );
}
#[test]
fn real_sdk_handoff_reclamation_restart_and_remote_read() {
    use fabric::{
        run::RunContext,
        sdk::{Admission, RunClient},
        spindle::{self, Daemon, custody::Store},
    };
    let dir = tempfile::tempdir().unwrap();
    let edge = dir.path().join("edge");
    fs::create_dir(&edge).unwrap();
    fs::set_permissions(&edge, fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = spindle::Config {
        socket: edge.join("socket"),
        storage: edge.join("store"),
        quota_bytes: 8 << 20,
        forwarding: None,
        max_connections: 16,
        client_timeout_ms: 500,
        files: vec![],
        journal: None,
        host: None,
    };
    let store = Store::open(&config.storage, config.quota_bytes).unwrap();
    let origin = store.origin().to_owned();
    drop(store);
    let receiver_root = dir.path().join("receiver");
    fs::create_dir(&receiver_root).unwrap();
    fs::set_permissions(&receiver_root, fs::Permissions::from_mode(0o700)).unwrap();
    let server = Receiver::new(&receiver_root, &[&origin]);
    let Response::Progress { authority, .. } = server.call("reader", Request::Progress) else {
        panic!("authority")
    };
    config.forwarding = Some(spindle::ForwardConfig {
        endpoint: server.endpoint.clone(),
        credential_file: server.config.nodes[0].credential_file.clone(),
        authority: authority.clone(),
        reclaim_confirmed: true,
    });
    let start = |cfg: spindle::Config| {
        let mut daemon = Daemon::open(cfg).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let exit = stop.clone();
        let worker = thread::spawn(move || {
            while !exit.load(Ordering::SeqCst) {
                daemon.tick().unwrap();
                thread::sleep(Duration::from_millis(1));
            }
        });
        (stop, worker)
    };
    let (stop, worker) = start(config.clone());
    let context = RunContext::new().unwrap();
    let mut cfg = fabric::sdk::Config::local(config.socket.clone(), "actual-sdk".into()).unwrap();
    cfg.max_record = 2048;
    cfg.slots = 4;
    let client = RunClient::new(cfg, context.clone()).unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        match client.try_record(
            fabric::observation::Data::ToolExecution {
                tool: "test".into(),
                run_id: context.run_id.clone(),
                duration_ns: u64::MAX,
                status: fabric::observation::Outcome::Ok,
            },
            u64::MAX,
        ) {
            Admission::Accepted { sequence: 1 } => break,
            Admission::Contended if Instant::now() < until => thread::yield_now(),
            other => panic!("{other:?}"),
        }
    }
    let receipt = client
        .flush_to_fabric(Instant::now() + Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(receipt.authority, authority);
    assert_eq!(receipt.origin, origin);
    assert_eq!(receipt.through, 1);
    client.close();
    stop.store(true, Ordering::SeqCst);
    worker.join().unwrap();
    let (stop, worker) = start(config.clone());
    let result = spindle::transport::request(
        &config.socket,
        &spindle::Request::Read { after: 0, limit: 1 },
        Instant::now() + Duration::from_secs(2),
    )
    .unwrap();
    let spindle::Reply::Records { records } = result else {
        panic!("{result:?}")
    };
    assert_eq!(records[0].0, 1);
    assert_eq!(records[0].1.context(), Some(&context));
    stop.store(true, Ordering::SeqCst);
    worker.join().unwrap();
}

#[test]
fn continuous_ingest_and_prefix_reads_share_baseline_sqlite_connection() {
    use std::sync::atomic::AtomicU64;
    let root = tempfile::tempdir().unwrap();
    let origins = [
        "continuous-a",
        "continuous-b",
        "continuous-c",
        "continuous-d",
    ];
    let receiver = Receiver::new(root.path(), &origins);
    let committed: [AtomicU64; 4] = std::array::from_fn(|_| AtomicU64::new(0));
    thread::scope(|scope| {
        for (index, origin) in origins.iter().enumerate() {
            let receiver = &receiver;
            let committed = &committed[index];
            scope.spawn(move || {
                for first in (1..=128).step_by(8) {
                    let records = (first..first + 8).map(|id| row(id, raw(id))).collect();
                    match receiver.call(
                        origin,
                        Request::Ingest {
                            origin: (*origin).into(),
                            records,
                        },
                    ) {
                        Response::Committed { through, .. } => {
                            committed.store(through, Ordering::Release);
                        }
                        other => panic!("continuous admission failed: {other:?}"),
                    }
                }
            });
            scope.spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(10);
                while committed.load(Ordering::Acquire) < 32 {
                    assert!(Instant::now() < deadline, "continuous prefix unavailable");
                    thread::sleep(Duration::from_millis(1));
                }
                for _ in 0..16 {
                    match receiver.call(
                        origin,
                        Request::Read {
                            origin: (*origin).into(),
                            after: 0,
                            through: 32,
                            limit: 32,
                        },
                    ) {
                        Response::Rows { rows } => {
                            assert_eq!(rows.len(), 32);
                            for (offset, record) in rows.iter().enumerate() {
                                assert_eq!(record.origin, *origin);
                                assert_eq!(record.source_id, offset as u64 + 1);
                                assert_eq!(record.raw, raw(offset as u64 + 1));
                            }
                        }
                        other => panic!("acknowledged prefix read failed under writes: {other:?}"),
                    }
                }
            });
        }
    });
}

#[test]
fn maximum_batch_retries_keep_exact_bytes_and_conflicts_do_not_advance() {
    let root = tempfile::tempdir().unwrap();
    let receiver = Receiver::new(root.path(), &["bounded"]);
    let records = (1..=canonical::MAX_BATCH as u64)
        .map(|id| row(id, raw(id)))
        .collect::<Vec<_>>();
    for _ in 0..2 {
        assert!(
            matches!(receiver.call("bounded", Request::Ingest { origin: "bounded".into(), records: records.clone() }), Response::Committed { through, .. } if through == canonical::MAX_BATCH as u64)
        );
    }
    let mut conflict = records.clone();
    conflict.last_mut().unwrap().raw.push(' ');
    assert!(matches!(
        receiver.call(
            "bounded",
            Request::Ingest {
                origin: "bounded".into(),
                records: conflict
            }
        ),
        Response::Error { .. }
    ));
    match receiver.call(
        "bounded",
        Request::Read {
            origin: "bounded".into(),
            after: 0,
            through: canonical::MAX_BATCH as u64,
            limit: canonical::MAX_BATCH,
        },
    ) {
        Response::Rows { rows } => {
            assert_eq!(rows.len(), records.len());
            for (stored, original) in rows.iter().zip(records) {
                assert_eq!(stored.source_id, original.source_id);
                assert_eq!(stored.raw, original.raw);
            }
        }
        other => panic!("bounded retry history unavailable: {other:?}"),
    }
}
