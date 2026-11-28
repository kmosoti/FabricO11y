use fabric::canonical::{self, Request as CanonicalRequest, Response as CanonicalResponse};
use fabric::observation::{Data, MetricType, Outcome, decode_versioned};
use fabric::sdk::{Admission, Client, Config as SdkConfig};
use fabric::spindle::{self, ForwardConfig, Reply, Request, transport};
use fabric_collect::{
    DaemonConfig, DaemonRequest, DaemonResponse, SelfPreservationGuard, query_daemon_socket,
    run_daemon, run_spindle_only_daemon,
};
use fabric_core::SpindleProvenance;
use fabric_query::probe_spindle_projection;
use fabric_query::unified::{QueryDomain, UnifiedQuerySpec, execute_unified_query};
use fabric_store::{CatalogStore, validate_non_tmp_path_on_disk, validated_storage_layout};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TEST_NAME: &str = "spindle_forwards_to_server_and_unifies_into_collector_olap";
const MAX_UNIX_SOCK_BYTES: usize = 104;

enum SliceMode {
    ReexecutedAlready,
    ConfinedInProcess,
    UnconfinedFallback,
}

fn in_fabrico11y_slice() -> bool {
    fs::read_to_string("/proc/self/cgroup")
        .map(|s| s.contains("fabrico11y.slice"))
        .unwrap_or(false)
}

fn probe_slice_and_maybe_reexec() -> SliceMode {
    if in_fabrico11y_slice() {
        return SliceMode::ConfinedInProcess;
    }
    if std::env::var_os("FABRIC_CONFINED_REEXEC").as_deref() == Some(OsStr::new("1")) {
        if std::env::var("FABRIC_REQUIRE_SLICE").as_deref() == Ok("1") {
            panic!("FABRIC_REQUIRE_SLICE=1 set, yet re-exec did not land inside fabrico11y.slice");
        }
        return SliceMode::UnconfinedFallback;
    }
    let probe = Command::new("systemd-run")
        .args([
            "--user",
            "--scope",
            "--collect",
            "--slice=fabrico11y.slice",
            "--",
            "true",
        ])
        .output();
    match probe {
        Ok(out) if out.status.success() => {
            let exe = std::env::current_exe().expect("current test executable");
            let status = Command::new("systemd-run")
                .args([
                    "--user",
                    "--scope",
                    "--collect",
                    "--slice=fabrico11y.slice",
                    "--",
                ])
                .arg(&exe)
                .args(["--exact", TEST_NAME, "--nocapture"])
                .env("FABRIC_CONFINED_REEXEC", "1")
                .status()
                .expect("re-exec under systemd-run --slice=fabrico11y.slice");
            assert!(
                status.success(),
                "confined spindle_unification execution failed: {status}"
            );
            SliceMode::ReexecutedAlready
        }
        Ok(out) => {
            if std::env::var("FABRIC_REQUIRE_SLICE").as_deref() == Ok("1") {
                panic!(
                    "FABRIC_REQUIRE_SLICE=1 requires systemd-run --user --slice=fabrico11y.slice: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            SliceMode::UnconfinedFallback
        }
        Err(err) => {
            if std::env::var("FABRIC_REQUIRE_SLICE").as_deref() == Ok("1") {
                panic!("FABRIC_REQUIRE_SLICE=1 requires systemd-run: {err}");
            }
            SliceMode::UnconfinedFallback
        }
    }
}

struct StackGuard {
    spindle_sock: PathBuf,
    server_stop: Arc<AtomicBool>,
    spindle_thread: Option<thread::JoinHandle<Result<(), String>>>,
    server_thread: Option<thread::JoinHandle<Result<(), String>>>,
}

impl StackGuard {
    fn shutdown_and_assert(mut self) {
        let _ = transport::request(
            &self.spindle_sock,
            &Request::Stop,
            Instant::now() + Duration::from_secs(2),
        );
        self.server_stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.spindle_thread.take() {
            assert!(h.join().is_ok(), "spindle thread panicked");
        }
        if let Some(h) = self.server_thread.take() {
            assert!(h.join().is_ok(), "server thread panicked");
        }
    }
}

impl Drop for StackGuard {
    fn drop(&mut self) {
        if self.spindle_thread.is_none() && self.server_thread.is_none() {
            return;
        }
        let _ = transport::request(
            &self.spindle_sock,
            &Request::Stop,
            Instant::now() + Duration::from_secs(2),
        );
        self.server_stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.spindle_thread.take() {
            let _ = h.join();
        }
        if let Some(h) = self.server_thread.take() {
            let _ = h.join();
        }
    }
}

fn non_tmp_test_root() -> tempfile::TempDir {
    let base = std::env::var_os("TMPDIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .filter(|p| !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
        .or_else(|| {
            option_env!("CARGO_TARGET_TMPDIR")
                .map(PathBuf::from)
                .filter(|p| !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
        })
        .or_else(|| {
            std::env::var_os("CARGO_TARGET_TMPDIR")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .filter(|p| !p.starts_with("/tmp") && !p.starts_with("/dev/shm"))
        })
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache").join("fabrico11y-tmp"))
        })
        .expect("non-tmp root directory available");
    fs::create_dir_all(&base).expect("create test base dir");
    let dir = tempfile::Builder::new()
        .prefix("su-")
        .tempdir_in(&base)
        .expect("create tempdir");
    validate_non_tmp_path_on_disk(dir.path()).expect("tempdir must not be under /tmp");
    dir
}

fn write_secret(path: &Path, secret: &str) {
    fs::write(path, format!("{secret}\n")).expect("write secret file");
    let mut perms = fs::metadata(path).expect("secret metadata").permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms).expect("chmod 0600");
}

fn create_private_dir(path: &Path) {
    fs::create_dir_all(path).expect("create private dir");
    let mut perms = fs::metadata(path).expect("dir metadata").permissions();
    perms.set_mode(0o700);
    fs::set_permissions(path, perms).expect("chmod 0700");
}

#[test]
fn spindle_forwards_to_server_and_unifies_into_collector_olap() {
    let confined = match probe_slice_and_maybe_reexec() {
        SliceMode::ReexecutedAlready => return,
        SliceMode::ConfinedInProcess => true,
        SliceMode::UnconfinedFallback => false,
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("build bounded tokio runtime");

    let root_guard = non_tmp_test_root();
    let root = root_guard.path().to_path_buf();

    let spindle_dir = root.join("sp");
    let sock_dir = root.join("sk");
    let server_dir = root.join("srv");
    let projection_dir = root.join("prj");
    let collector_data = root.join("cd");
    let collector_tmp = root.join("ct");
    let runtime_dir = root.join("rt");
    let spindle_sock = sock_dir.join("spindle.sock");
    create_private_dir(&sock_dir);
    create_private_dir(&collector_tmp);
    create_private_dir(&runtime_dir);
    assert!(
        spindle_sock.as_os_str().len() < MAX_UNIX_SOCK_BYTES,
        "spindle_sock path too long: {}",
        spindle_sock.display()
    );
    assert!(
        runtime_dir.join("daemon.sock").as_os_str().len() < MAX_UNIX_SOCK_BYTES,
        "daemon.sock path too long"
    );
    assert!(
        runtime_dir.join("ingest.sock").as_os_str().len() < MAX_UNIX_SOCK_BYTES,
        "ingest.sock path too long"
    );

    let self_guard = SelfPreservationGuard::new(
        collector_data.to_string_lossy().into_owned(),
        collector_tmp.to_string_lossy().into_owned(),
        Arc::new(AtomicU64::new(0)),
    );
    let initial_health = self_guard.inspect_summary();
    assert_eq!(initial_health.tmp_files_created, 0);
    assert!(!initial_health.self_shedding_active);
    if confined {
        assert!(initial_health.fabrico11y_slice_confined);
    }

    let node_cred = root.join("node.token");
    let reader_cred = root.join("reader.token");
    let node_secret = "spindle-node-secret-token-001";
    let reader_secret = "fabric-reader-secret-token-001";
    write_secret(&node_cred, node_secret);
    write_secret(&reader_cred, reader_secret);

    let origin = {
        let store =
            spindle::custody::Store::open(&spindle_dir, 64 * 1024 * 1024).expect("init custody");
        store.origin().to_owned()
    };
    let authority = {
        let ledger = fabric_server::ledger::Ledger::open(&server_dir, 128 * 1024 * 1024)
            .expect("init server ledger");
        ledger.authority.clone()
    };

    let server = fabric_server::Server::open(fabric_server::Config {
        listen: "127.0.0.1:0".into(),
        storage: server_dir,
        projection: projection_dir,
        quota_bytes: 128 * 1024 * 1024,
        projection_quota_bytes: 64 * 1024 * 1024,
        reader_credential_file: reader_cred,
        nodes: vec![fabric_server::NodeGrant {
            origin: origin.clone(),
            credential_file: node_cred.clone(),
        }],
        decision_writers: Vec::new(),
        node_quota_bytes: Default::default(),
        engine_endpoint: None,
        engine_auth_token_file: None,
        allow_unauthenticated_loopback_engine: false,
        max_connections: 16,
    })
    .expect("open fabric-server");
    let server_endpoint = server.address();
    let server_stop: Arc<AtomicBool> = server.stop_handle();
    let server_thread = thread::spawn(move || server.serve());

    let mut spindle_daemon = spindle::Daemon::open(spindle::Config {
        socket: spindle_sock.clone(),
        storage: spindle_dir,
        quota_bytes: 64 * 1024 * 1024,
        forwarding: Some(ForwardConfig {
            endpoint: server_endpoint.clone(),
            credential_file: node_cred,
            authority,
            reclaim_confirmed: false,
        }),
        max_connections: 16,
        client_timeout_ms: 1000,
        files: Vec::new(),
        journal: None,
        host: None,
    })
    .expect("open spindle daemon");
    let spindle_thread = thread::spawn(move || spindle_daemon.run());
    let stack = StackGuard {
        spindle_sock: spindle_sock.clone(),
        server_stop,
        spindle_thread: Some(spindle_thread),
        server_thread: Some(server_thread),
    };

    // Wait briefly until Spindle Unix socket is ready.
    let sock_deadline = Instant::now() + Duration::from_secs(5);
    let mut spindle_ready = false;
    while Instant::now() < sock_deadline {
        if spindle_sock.exists()
            && transport::request(
                &spindle_sock,
                &Request::Status,
                Instant::now() + Duration::from_millis(250),
            )
            .is_ok()
        {
            spindle_ready = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(spindle_ready, "spindle.sock did not become ready in time");

    let mut sdk_cfg =
        SdkConfig::local(spindle_sock.clone(), "agent-harness".into()).expect("sdk config");
    sdk_cfg.delivery_timeout = Duration::from_secs(2);
    let client = Client::new(sdk_cfg).expect("sdk client");

    let run_id = "01950000-0000-7000-8000-000000000001";
    let base_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("unix epoch")
        .as_nanos() as u64;

    let submit_with_retry = |data: Data, ts: u64| -> Admission {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match client.try_record(data.clone(), ts) {
                Admission::Contended | Admission::Full if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(1));
                }
                other => return other,
            }
        }
    };

    let first = submit_with_retry(
        Data::ToolExecution {
            tool: "cargo-check".into(),
            run_id: run_id.into(),
            duration_ns: 42_000_000,
            status: Outcome::Ok,
        },
        base_ns,
    );
    assert_eq!(first, Admission::Accepted { sequence: 1 });

    let second = submit_with_retry(
        Data::AgentRun {
            agent: "agy".into(),
            run_id: run_id.into(),
            duration_ns: 120_000_000,
            status: Outcome::Ok,
        },
        base_ns.saturating_add(1_000_000),
    );
    assert_eq!(second, Admission::Accepted { sequence: 2 });

    let receipt = client
        .flush(Instant::now() + Duration::from_secs(5))
        .expect("flush to spindle custody");
    assert!(receipt.is_some());

    // Wait until Spindle has forwarded both records to fabric-server.
    let fwd_deadline = Instant::now() + Duration::from_secs(5);
    let mut forwarded = 0u64;
    while Instant::now() < fwd_deadline {
        if let Ok(Reply::Status { status }) = transport::request(
            &spindle_sock,
            &Request::Status,
            Instant::now() + Duration::from_millis(500),
        ) {
            forwarded = status.canonical_through;
            if forwarded >= 2 {
                break;
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    assert!(
        forwarded >= 2,
        "expected spindle canonical_through >= 2, got {forwarded}"
    );

    let progress = canonical::exchange(
        &server_endpoint,
        reader_secret,
        &CanonicalRequest::Progress,
        Instant::now() + Duration::from_secs(2),
    )
    .expect("server canonical progress");
    match progress {
        CanonicalResponse::Progress { frontiers, .. } => {
            assert_eq!(frontiers.get(&origin).copied(), Some(2));
        }
        other => panic!("unexpected server response: {other:?}"),
    }

    let layout = validated_storage_layout(&collector_data).expect("validate collector path");
    let frontier = CatalogStore::read_spindle_projection_frontier(
        &layout,
        Instant::now() + Duration::from_secs(1),
    )
    .expect("read empty projection frontier");
    assert_eq!(frontier.record_id, 0);
    assert!(!collector_data.exists(), "read created collector state");

    // Pass 1: flush records 1..=2 and advance watermark to 2.
    rt.block_on(async {
        run_daemon(DaemonConfig {
            data_dir: collector_data.clone(),
            tmp_dir: collector_tmp.clone(),
            interval_ms: 250,
            max_ticks: Some(1),
            ring_budget_bytes: 2 * 1024 * 1024,
            max_disk_bytes: 64 * 1024 * 1024,
            spindle_socket: Some(spindle_sock.clone()),
            runtime_dir: Some(runtime_dir.clone()),
        })
        .await
        .expect("run converged collector daemon pass 1");
    });

    {
        let catalog = CatalogStore::open(&collector_data).expect("reopen collector catalog");
        assert_eq!(
            catalog
                .read_watermark("spindle_watermark_id")
                .expect("read spindle watermark"),
            Some(2)
        );
        let frontier = CatalogStore::read_spindle_projection_frontier(
            catalog.layout(),
            Instant::now() + Duration::from_secs(1),
        )
        .expect("read bound projection frontier");
        assert_eq!(frontier.origin.as_deref(), Some(origin.as_str()));
        assert_eq!(frontier.legacy_through, 0);
        assert_eq!(frontier.record_id, 2);
    }

    // Submit 3rd observation: Data::Metric with custom labels.
    let third = submit_with_retry(
        Data::Metric {
            name: "custom.tokens_per_sec".into(),
            unit: "tok/s".into(),
            value: 123.5,
            metric_type: MetricType::Gauge,
            reset: false,
        },
        base_ns.saturating_add(2_000_000),
    );
    assert_eq!(third, Admission::Accepted { sequence: 3 });
    let receipt3 = client
        .flush(Instant::now() + Duration::from_secs(5))
        .expect("flush metric to spindle custody");
    assert!(receipt3.is_some());
    drop(client);

    // Wait for server frontier to advance to 3 as well.
    let fwd3_deadline = Instant::now() + Duration::from_secs(5);
    let mut forwarded3 = 0u64;
    while Instant::now() < fwd3_deadline {
        if let Ok(Reply::Status { status }) = transport::request(
            &spindle_sock,
            &Request::Status,
            Instant::now() + Duration::from_millis(500),
        ) {
            forwarded3 = status.canonical_through;
            if forwarded3 >= 3 {
                break;
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    assert!(
        forwarded3 >= 3,
        "expected spindle canonical_through >= 3 after metric flush, got {forwarded3}"
    );
    let raw_after_2 = transport::read_raw(
        &spindle_sock,
        2,
        Instant::now() + Duration::from_millis(500),
    )
    .expect("read_raw after 2");
    assert_eq!(
        raw_after_2.len(),
        1,
        "expected record 3 readable raw from spindle after watermark 2, got {raw_after_2:?}"
    );
    let observation = decode_versioned(&raw_after_2[0].1).expect("decode metric");
    let identity = observation.identity();
    let metric_key = SpindleProvenance {
        origin: origin.clone(),
        record_id: raw_after_2[0].0,
        producer: identity.producer.clone(),
        generation: identity.generation.clone(),
        sequence: identity.sequence,
    };
    let metric_timestamp = (observation.timestamp_ns() / 1_000_000) as i64;
    assert!(
        !rt.block_on(probe_spindle_projection(
            &layout,
            QueryDomain::Metrics,
            metric_timestamp,
            &metric_key,
        ))
        .expect("probe before collector projects the metric")
    );

    // Pass 2: verify hot ring label preservation & Doctor health over daemon.sock while alive.
    let daemon_sock = runtime_dir.join("daemon.sock");
    rt.block_on(async {
        let daemon_handle = tokio::spawn(run_daemon(DaemonConfig {
            data_dir: collector_data.clone(),
            tmp_dir: collector_tmp.clone(),
            interval_ms: 250,
            max_ticks: Some(6),
            ring_budget_bytes: 2 * 1024 * 1024,
            max_disk_bytes: 64 * 1024 * 1024,
            spindle_socket: Some(spindle_sock.clone()),
            runtime_dir: Some(runtime_dir.clone()),
        }));

        let mut saw_hot_metric = false;
        let mut saw_healthy_doctor = false;
        let rpc_deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < rpc_deadline && (!saw_hot_metric || !saw_healthy_doctor) {
            if !saw_hot_metric
                && let Some(DaemonResponse::HotMetrics { points }) =
                    query_daemon_socket(&daemon_sock, &DaemonRequest::HotMetrics { since_ms: 0 })
                && let Some(pt) = points.iter().find(|p| p.name == "custom.tokens_per_sec")
            {
                assert!(
                    pt.labels_json.contains("\"metric_unit\":\"tok/s\""),
                    "hot ring lost custom metric labels: {}",
                    pt.labels_json
                );
                saw_hot_metric = true;
            }
            if !saw_healthy_doctor
                && let Some(DaemonResponse::Doctor(doc)) =
                    query_daemon_socket(&daemon_sock, &DaemonRequest::Doctor)
            {
                if confined {
                    assert!(doc.healthy, "daemon doctor reported unhealthy: {doc:?}");
                    assert!(doc.fabrico11y_slice_confined);
                } else {
                    assert!(
                        doc.checks
                            .iter()
                            .filter(|c| c.name != "fabrico11y_slice")
                            .all(|c| c.passed),
                        "non-slice doctor checks failed: {doc:?}"
                    );
                }
                assert_eq!(doc.tmp_files_created, 0);
                assert!(!doc.self_shedding_active);
                saw_healthy_doctor = true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(saw_hot_metric, "custom metric never observed in hot ring");
        assert!(saw_healthy_doctor, "daemon Doctor RPC never succeeded");

        daemon_handle
            .await
            .expect("join daemon task")
            .expect("run converged collector daemon pass 2");
    });

    assert!(
        !runtime_dir.join("daemon.sock").exists(),
        "daemon.sock leaked after clean shutdown"
    );
    assert!(
        !runtime_dir.join("ingest.sock").exists(),
        "ingest.sock leaked after clean shutdown"
    );

    {
        let catalog = CatalogStore::open(&collector_data).expect("reopen collector catalog");
        assert_eq!(
            catalog
                .read_watermark("spindle_watermark_id")
                .expect("read post-flush spindle watermark"),
            Some(3)
        );
    }

    rt.block_on(async {
        assert!(
            probe_spindle_projection(&layout, QueryDomain::Metrics, metric_timestamp, &metric_key)
                .await
                .expect("probe exact projection after collector restart")
        );
        let mut wrong_origin = metric_key.clone();
        wrong_origin.origin = "other-origin".into();
        assert!(
            !probe_spindle_projection(
                &layout,
                QueryDomain::Metrics,
                metric_timestamp,
                &wrong_origin,
            )
            .await
            .expect("probe conflicting source")
        );
        let qres = execute_unified_query(
            &collector_data,
            &UnifiedQuerySpec {
                domain: QueryDomain::Traces,
                since_ms: None,
                where_clauses: vec![],
                group_by: vec![],
                agg_specs: vec![],
                limit: 50,
            },
            &[],
            None,
        )
        .await
        .expect("query unified traces");
        assert!(
            qres.row_count >= 2,
            "expected >= 2 unified trace spans from spindle custody, got {}",
            qres.row_count
        );
        let names: Vec<&str> = qres
            .rows
            .iter()
            .filter_map(|r| r.get("name").and_then(|v| v.as_str()))
            .collect();
        assert!(
            names.contains(&"cargo-check"),
            "missing tool span: {names:?}"
        );
        assert!(
            names.contains(&"agent_run"),
            "missing agent span: {names:?}"
        );

        let mres = execute_unified_query(
            &collector_data,
            &UnifiedQuerySpec {
                domain: QueryDomain::Metrics,
                since_ms: None,
                where_clauses: vec!["name=custom.tokens_per_sec".into()],
                group_by: vec![],
                agg_specs: vec![],
                limit: 20,
            },
            &[],
            None,
        )
        .await
        .expect("query unified custom metric");
        assert_eq!(mres.row_count, 1, "expected custom metric in OLAP");
        let row = &mres.rows[0];
        assert_eq!(row.get("value").and_then(|v| v.as_f64()), Some(123.5));
        let labels = row
            .get("labels_json")
            .and_then(|v| v.as_str())
            .expect("labels_json in OLAP metric row");
        assert!(
            labels.contains("\"metric_unit\":\"tok/s\""),
            "OLAP metric lost labels_json: {labels}"
        );
    });

    // Project the same source into an independent read model without running a
    // second set of host probes or opening a general ingest endpoint.
    let spindle_only_data = root.join("solo");
    let spindle_only_layout = validated_storage_layout(&spindle_only_data).unwrap();
    let spindle_only_socket = spindle_only_layout.state_dir().join("daemon.sock");
    rt.block_on(async {
        let task = tokio::spawn(run_spindle_only_daemon(DaemonConfig {
            data_dir: spindle_only_data.clone(),
            tmp_dir: collector_tmp.clone(),
            interval_ms: 250,
            max_ticks: Some(6),
            ring_budget_bytes: 2 * 1024 * 1024,
            max_disk_bytes: 64 * 1024 * 1024,
            spindle_socket: Some(spindle_sock.clone()),
            runtime_dir: None,
        }));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if matches!(
                query_daemon_socket(&spindle_only_socket, &DaemonRequest::Ping),
                Some(DaemonResponse::Pong { .. })
            ) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Spindle-only RPC never became ready"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!spindle_only_layout.state_dir().join("ingest.sock").exists());
        task.await
            .expect("join Spindle-only daemon")
            .expect("run Spindle-only mode");
        assert!(
            !spindle_only_socket.exists(),
            "Spindle-only RPC socket leaked"
        );

        let rows = execute_unified_query(
            &spindle_only_data,
            &UnifiedQuerySpec {
                domain: QueryDomain::Metrics,
                since_ms: None,
                where_clauses: vec![],
                group_by: vec![],
                agg_specs: vec![],
                limit: 50,
            },
            &[],
            None,
        )
        .await
        .expect("query isolated Spindle-only metrics");
        assert_eq!(rows.row_count, 1, "mode collected unrelated host metrics");
        assert_eq!(rows.rows[0]["name"], "custom.tokens_per_sec");
        assert!(
            probe_spindle_projection(
                &spindle_only_layout,
                QueryDomain::Metrics,
                metric_timestamp,
                &metric_key,
            )
            .await
            .expect("verify committed evidence in Spindle-only mode")
        );
    });
    assert!(!spindle_only_layout.live_state_json().exists());

    let final_health = self_guard.inspect_summary();
    assert_eq!(final_health.tmp_files_created, 0);
    assert!(!final_health.self_shedding_active);

    stack.shutdown_and_assert();
}

#[test]
#[ignore = "requires collected fabrico11y.slice and FABRIC_LIVE_CURSOR_BYTES=32768"]
fn local_live_metric_cursor_socket_profile() {
    assert!(
        in_fabrico11y_slice(),
        "run inside the collected fabrico11y.slice"
    );
    assert_eq!(
        std::env::var("FABRIC_LIVE_CURSOR_BYTES").as_deref(),
        Ok("32768")
    );
    let root_guard = non_tmp_test_root();
    let root = root_guard.path();
    let socket_dir = root.join("sock");
    let collector_tmp = root.join("ct");
    for path in [&socket_dir, &collector_tmp] {
        create_private_dir(path);
    }
    let spindle_sock = socket_dir.join("spindle.sock");
    let collector_data = root.join("collector");
    let daemon_sock = validated_storage_layout(&collector_data)
        .unwrap()
        .state_dir()
        .join("daemon.sock");
    let spindle_dir = root.join("sp");
    let origin = spindle::custody::Store::open(&spindle_dir, 64 * 1024 * 1024)
        .unwrap()
        .origin()
        .to_owned();
    let mut spindle_daemon = spindle::Daemon::open(spindle::Config {
        socket: spindle_sock.clone(),
        storage: spindle_dir,
        quota_bytes: 64 * 1024 * 1024,
        forwarding: None,
        max_connections: 16,
        client_timeout_ms: 1000,
        files: Vec::new(),
        journal: None,
        host: None,
    })
    .unwrap();
    let spindle_thread = thread::spawn(move || spindle_daemon.run());
    let ready = Instant::now() + Duration::from_secs(10);
    while transport::request(
        &spindle_sock,
        &Request::Status,
        Instant::now() + Duration::from_millis(250),
    )
    .is_err()
    {
        assert!(Instant::now() < ready, "Spindle startup deadline");
        thread::sleep(Duration::from_millis(20));
    }
    let mut sdk_cfg = SdkConfig::local(spindle_sock.clone(), "live-fixture".into()).unwrap();
    sdk_cfg.delivery_timeout = Duration::from_secs(2);
    let client = Client::new(sdk_cfg).unwrap();
    let config = DaemonConfig {
        data_dir: collector_data,
        tmp_dir: collector_tmp,
        interval_ms: 250,
        max_ticks: Some(80),
        ring_budget_bytes: 2 * 1024 * 1024,
        max_disk_bytes: 64 * 1024 * 1024,
        spindle_socket: Some(spindle_sock.clone()),
        runtime_dir: None,
    };
    let collector = thread::spawn(move || {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
            .block_on(run_spindle_only_daemon(config))
    });
    let ready = Instant::now() + Duration::from_secs(10);
    while !matches!(
        query_daemon_socket(&daemon_sock, &DaemonRequest::Ping),
        Some(DaemonResponse::Pong { .. })
    ) {
        assert!(Instant::now() < ready, "collector startup deadline");
        thread::sleep(Duration::from_millis(20));
    }
    let submit = |id: usize| {
        let admission = client.try_record(
            Data::Metric {
                name: format!("live.sample.{id}"),
                unit: "units".into(),
                value: id as f64,
                metric_type: MetricType::Gauge,
                reset: false,
            },
            1_700_000_000_000_000_000,
        );
        assert!(
            matches!(admission, Admission::Accepted { .. }),
            "{admission:?}"
        );
        client
            .flush(Instant::now() + Duration::from_secs(5))
            .unwrap()
            .unwrap()
            .id
    };
    let first_id = submit(0);
    let ready = Instant::now() + Duration::from_secs(10);
    let initial = loop {
        if let Some(DaemonResponse::LiveMetricsV1(page)) = query_daemon_socket(
            &daemon_sock,
            &DaemonRequest::LiveMetricsV1 {
                origin: None,
                epoch: None,
                after: 0,
                count: 64,
                max_bytes: 32768,
            },
        ) && page.entries.iter().any(|entry| entry.record_id == first_id)
        {
            break page;
        }
        assert!(Instant::now() < ready, "live cursor readiness deadline");
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(initial.origin, origin);
    assert!(!initial.gap);
    let epoch = initial.epoch.clone();
    let mut after = initial.next_after;
    let mut delays_ms = Vec::new();
    for sample in 1..=5 {
        thread::sleep(Duration::from_millis(100));
        // Measurement starts immediately before SDK submission and includes the durable flush.
        let start = Instant::now();
        let id = submit(sample);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(DaemonResponse::LiveMetricsV1(page)) = query_daemon_socket(
                &daemon_sock,
                &DaemonRequest::LiveMetricsV1 {
                    origin: Some(origin.clone()),
                    epoch: Some(epoch.clone()),
                    after,
                    count: 64,
                    max_bytes: 32768,
                },
            ) {
                assert!(!page.gap);
                if !page.entries.is_empty() {
                    assert_eq!(page.entries.len(), 1);
                    assert_eq!(page.entries[0].record_id, id);
                    assert_eq!(page.entries[0].point.name, format!("live.sample.{sample}"));
                    assert_eq!(page.next_after, id);
                    after = id;
                    delays_ms.push(start.elapsed().as_millis());
                    break;
                }
            }
            assert!(Instant::now() < deadline, "cursor visibility deadline");
            thread::sleep(Duration::from_millis(20));
        }
    }
    for (bad_origin, bad_epoch) in [
        ("other".to_string(), epoch.clone()),
        (origin.clone(), "bad".to_string()),
    ] {
        assert!(matches!(
            query_daemon_socket(
                &daemon_sock,
                &DaemonRequest::LiveMetricsV1 {
                    origin: Some(bad_origin),
                    epoch: Some(bad_epoch),
                    after,
                    count: 1,
                    max_bytes: 32768
                }
            ),
            Some(DaemonResponse::Error { .. })
        ));
    }
    // Force a bounded-wrap gap using real accepted records without timing this burst.
    for i in 0..132 {
        let _ = submit(1000 + i);
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(DaemonResponse::LiveMetricsV1(page)) = query_daemon_socket(
            &daemon_sock,
            &DaemonRequest::LiveMetricsV1 {
                origin: Some(origin.clone()),
                epoch: Some(epoch.clone()),
                after,
                count: 64,
                max_bytes: 32768,
            },
        ) && page.gap
        {
            assert!(page.highest_lost > after);
            assert_eq!(
                page.highest_lost_reason,
                Some(fabric_collect::live_cursor::LossReason::Evicted)
            );
            assert!(page.oldest_retained.is_some());
            break;
        }
        assert!(Instant::now() < deadline, "cursor wrap deadline");
        thread::sleep(Duration::from_millis(20));
    }
    let status = fs::read_to_string("/proc/self/status").unwrap();
    let vmhwm = status
        .lines()
        .find(|line| line.starts_with("VmHWM:"))
        .unwrap_or("unavailable");
    let fds = fs::read_dir("/proc/self/fd").unwrap().count();
    println!(
        "fixture submit-through-visibility ms={delays_ms:?}; combined test-process {vmhwm}; combined open-fds sample={fds}; interval_ms=250"
    );
    drop(client);
    assert!(collector.join().unwrap().is_ok());
    assert!(matches!(
        transport::request(
            &spindle_sock,
            &Request::Stop,
            Instant::now() + Duration::from_secs(5)
        ),
        Ok(Reply::Stopped)
    ));
    assert!(spindle_thread.join().unwrap().is_ok());
}

#[test]
#[ignore = "requires collected fabrico11y.slice and FABRIC_LIVE_CURSOR_BYTES=32768"]
fn isolated_cli_live_metric_cursor_socket_profile() {
    isolated_cursor_profile(false, false);
}

#[test]
#[ignore = "requires collected fabrico11y.slice and FABRIC_LIVE_CURSOR_BYTES=32768"]
fn isolated_cli_sustained_cursor_socket_profile() {
    isolated_cursor_profile(true, false);
}

#[test]
#[ignore = "requires collected fabrico11y.slice and exact CLI and MCP executables"]
fn isolated_cursor_cli_mcp_real_parity() {
    isolated_cursor_profile(false, true);
}

fn isolated_cursor_profile(sustained: bool, parity: bool) {
    assert!(
        in_fabrico11y_slice(),
        "run inside the collected fabrico11y.slice"
    );
    assert_eq!(
        std::env::var("FABRIC_LIVE_CURSOR_BYTES").as_deref(),
        Ok("32768")
    );
    let root_guard = non_tmp_test_root();
    let root = root_guard.path();
    let socket_dir = root.join("sock");
    let collector_tmp = root.join("ct");
    for path in [&socket_dir, &collector_tmp] {
        create_private_dir(path);
    }
    let spindle_sock = socket_dir.join("spindle.sock");
    let collector_data = root.join("collector");
    let daemon_sock = validated_storage_layout(&collector_data)
        .unwrap()
        .state_dir()
        .join("daemon.sock");
    let spindle_dir = root.join("sp");
    let origin = spindle::custody::Store::open(&spindle_dir, 64 * 1024 * 1024)
        .unwrap()
        .origin()
        .to_owned();
    let mut spindle_daemon = spindle::Daemon::open(spindle::Config {
        socket: spindle_sock.clone(),
        storage: spindle_dir,
        quota_bytes: 64 * 1024 * 1024,
        forwarding: None,
        max_connections: 16,
        client_timeout_ms: 1000,
        files: Vec::new(),
        journal: None,
        host: None,
    })
    .unwrap();
    let spindle_thread = thread::spawn(move || spindle_daemon.run());
    let ready = Instant::now() + Duration::from_secs(10);
    while transport::request(
        &spindle_sock,
        &Request::Status,
        Instant::now() + Duration::from_millis(250),
    )
    .is_err()
    {
        assert!(Instant::now() < ready, "Spindle startup deadline");
        thread::sleep(Duration::from_millis(20));
    }
    let mut sdk_cfg = SdkConfig::local(spindle_sock.clone(), "live-fixture".into()).unwrap();
    sdk_cfg.delivery_timeout = Duration::from_secs(2);
    let client = Client::new(sdk_cfg).unwrap();
    struct OwnedChild(std::process::Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let executable = std::env::var_os("FABRIC_QUALIFY_CLI")
        .expect("FABRIC_QUALIFY_CLI must name the exact built fabric executable");
    let mut collector = OwnedChild(
        std::process::Command::new(executable)
            .arg("--data-dir")
            .arg(&collector_data)
            .arg("--tmp-dir")
            .arg(&collector_tmp)
            .args([
                "daemon",
                "--spindle-only",
                "--interval-ms",
                "250",
                "--max-ticks",
                "80",
                "--ring-budget-bytes",
                "2097152",
                "--max-disk-bytes",
                "67108864",
                "--spindle-socket",
            ])
            .arg(&spindle_sock)
            .env("FABRIC_LIVE_CURSOR_BYTES", "32768")
            .env_remove("FABRIC_RUNTIME_DIR")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let pid = collector.0.id();
    let stop_sample = Arc::new(AtomicBool::new(false));
    let max_fds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let peak_kib = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let sample_stop = stop_sample.clone();
    let sample_fds = max_fds.clone();
    let sample_peak = peak_kib.clone();
    let sampler = thread::spawn(move || {
        while !sample_stop.load(Ordering::Relaxed) {
            if let Ok(status) = fs::read_to_string(format!("/proc/{pid}/status"))
                && let Some(kib) = status.lines().find_map(|line| {
                    line.strip_prefix("VmHWM:")
                        .and_then(|v| v.split_whitespace().next())
                        .and_then(|v| v.parse::<u64>().ok())
                })
            {
                sample_peak.fetch_max(kib, Ordering::Relaxed);
            }
            if let Ok(fds) = fs::read_dir(format!("/proc/{pid}/fd")) {
                sample_fds.fetch_max(fds.count(), Ordering::Relaxed);
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
    let ready = Instant::now() + Duration::from_secs(10);
    while !matches!(
        query_daemon_socket(&daemon_sock, &DaemonRequest::Ping),
        Some(DaemonResponse::Pong { .. })
    ) {
        assert!(Instant::now() < ready, "collector startup deadline");
        thread::sleep(Duration::from_millis(20));
    }
    let submit = |id: usize| {
        let admission = client.try_record(
            Data::Metric {
                name: format!("live.sample.{id}"),
                unit: "units".into(),
                value: id as f64,
                metric_type: MetricType::Gauge,
                reset: false,
            },
            1_700_000_000_000_000_000,
        );
        assert!(
            matches!(admission, Admission::Accepted { .. }),
            "{admission:?}"
        );
        client
            .flush(Instant::now() + Duration::from_secs(5))
            .unwrap()
            .unwrap()
            .id
    };
    let first_id = submit(0);
    let ready = Instant::now() + Duration::from_secs(10);
    let initial = loop {
        if let Some(DaemonResponse::LiveMetricsV1(page)) = query_daemon_socket(
            &daemon_sock,
            &DaemonRequest::LiveMetricsV1 {
                origin: None,
                epoch: None,
                after: 0,
                count: 64,
                max_bytes: 32768,
            },
        ) && page.entries.iter().any(|entry| entry.record_id == first_id)
        {
            break page;
        }
        assert!(Instant::now() < ready, "live cursor readiness deadline");
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(initial.origin, origin);
    assert!(!initial.gap);
    let epoch = initial.epoch.clone();
    let mut after = initial.next_after;
    let delays_ms = if sustained {
        // Schedule production independently of cursor polling; record actual starts and
        // correlate returned IDs to the monotonic observation time after both finish.
        let (starts, ids, seen) = thread::scope(|scope| {
            let writer = scope.spawn(|| {
                let schedule = Instant::now() + Duration::from_millis(100);
                let mut starts = Vec::new();
                let mut ids = Vec::new();
                for sample in 1..=20 {
                    let target = schedule + Duration::from_millis((sample - 1) as u64 * 100);
                    if let Some(delay) = target.checked_duration_since(Instant::now()) {
                        thread::sleep(delay);
                    }
                    starts.push(Instant::now());
                    ids.push(submit(sample));
                }
                (starts, ids)
            });
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut seen = Vec::new();
            while seen.len() < 20 {
                if let Some(DaemonResponse::LiveMetricsV1(page)) = query_daemon_socket(
                    &daemon_sock,
                    &DaemonRequest::LiveMetricsV1 {
                        origin: Some(origin.clone()),
                        epoch: Some(epoch.clone()),
                        after,
                        count: 64,
                        max_bytes: 32768,
                    },
                ) {
                    assert!(!page.gap, "unexpected loss during scheduled production");
                    let observed = Instant::now();
                    for entry in page.entries {
                        seen.push((entry.record_id, entry.point.name, observed));
                    }
                    after = page.next_after;
                }
                assert!(Instant::now() < deadline, "sustained visibility deadline");
                thread::sleep(Duration::from_millis(20));
            }
            let (starts, ids) = writer.join().unwrap();
            (starts, ids, seen)
        });
        assert_eq!(seen.len(), 20);
        let interarrival_ms: Vec<_> = starts
            .windows(2)
            .map(|pair| pair[1].duration_since(pair[0]).as_millis())
            .collect();
        let mut delays_ms = Vec::new();
        for (index, ((id, name, observed), start)) in seen.iter().zip(&starts).enumerate() {
            assert_eq!(*id, ids[index]);
            assert_eq!(name, &format!("live.sample.{}", index + 1));
            delays_ms.push(observed.duration_since(*start).as_millis());
        }
        println!(
            "scheduled samples=20 nominal_interval_ms=100 actual_interarrival_ms={interarrival_ms:?} submit_through_visibility_ms={delays_ms:?}"
        );
        delays_ms
    } else {
        let mut delays_ms = Vec::new();
        for sample in 1..=5 {
            thread::sleep(Duration::from_millis(100));
            // Measurement starts immediately before SDK submission and includes the durable flush.
            let start = Instant::now();
            let id = submit(sample);
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(DaemonResponse::LiveMetricsV1(page)) = query_daemon_socket(
                    &daemon_sock,
                    &DaemonRequest::LiveMetricsV1 {
                        origin: Some(origin.clone()),
                        epoch: Some(epoch.clone()),
                        after,
                        count: 64,
                        max_bytes: 32768,
                    },
                ) {
                    assert!(!page.gap);
                    if !page.entries.is_empty() {
                        assert_eq!(page.entries.len(), 1);
                        assert_eq!(page.entries[0].record_id, id);
                        assert_eq!(page.entries[0].point.name, format!("live.sample.{sample}"));
                        assert_eq!(page.next_after, id);
                        after = id;
                        delays_ms.push(start.elapsed().as_millis());
                        break;
                    }
                }
                assert!(Instant::now() < deadline, "cursor visibility deadline");
                thread::sleep(Duration::from_millis(20));
            }
        }
        delays_ms
    };
    for (bad_origin, bad_epoch) in [
        ("other".to_string(), epoch.clone()),
        (origin.clone(), "bad".to_string()),
    ] {
        assert!(matches!(
            query_daemon_socket(
                &daemon_sock,
                &DaemonRequest::LiveMetricsV1 {
                    origin: Some(bad_origin),
                    epoch: Some(bad_epoch),
                    after,
                    count: 1,
                    max_bytes: 32768
                }
            ),
            Some(DaemonResponse::Error { .. })
        ));
    }
    // Force a bounded-wrap gap using real accepted records without timing this burst.
    let mut last_burst = 0;
    for i in 0..132 {
        last_burst = submit(1000 + i);
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(DaemonResponse::LiveMetricsV1(page)) = query_daemon_socket(
            &daemon_sock,
            &DaemonRequest::LiveMetricsV1 {
                origin: Some(origin.clone()),
                epoch: Some(epoch.clone()),
                after,
                count: 64,
                max_bytes: 32768,
            },
        ) && page.gap
        {
            assert!(page.highest_lost > after);
            assert_eq!(
                page.highest_lost_reason,
                Some(fabric_collect::live_cursor::LossReason::Evicted)
            );
            assert!(page.oldest_retained.is_some());
            if parity {
                if page.observed_through < last_burst {
                    assert!(Instant::now() < deadline, "cursor settled deadline");
                    thread::sleep(Duration::from_millis(20));
                    continue;
                }
                verify_real_cursor_parity(
                    root,
                    &collector_data,
                    &collector_tmp,
                    &daemon_sock,
                    &origin,
                    &epoch,
                    after,
                    &page,
                );
            }
            break;
        }
        assert!(Instant::now() < deadline, "cursor wrap deadline");
        thread::sleep(Duration::from_millis(20));
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = collector.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "collector exit deadline");
        thread::sleep(Duration::from_millis(20));
    };
    stop_sample.store(true, Ordering::Relaxed);
    sampler.join().unwrap();
    assert!(status.success(), "isolated collector failed: {status}");
    let peak_kib = peak_kib.load(Ordering::Relaxed);
    let max_fds = max_fds.load(Ordering::Relaxed);
    assert!(
        peak_kib > 0 && max_fds > 0,
        "child resource samples unavailable"
    );
    let label = if sustained {
        "sustained isolated"
    } else {
        "isolated"
    };
    println!(
        "{label} child pid={pid}; submit-through-visibility ms={delays_ms:?}; child sampled VmHWM KiB={peak_kib}; child max sampled FDs={max_fds}; poll_interval_ms=250; sample_interval_ms=20"
    );
    drop(client);
    assert!(matches!(
        transport::request(
            &spindle_sock,
            &Request::Stop,
            Instant::now() + Duration::from_secs(5)
        ),
        Ok(Reply::Stopped)
    ));
    assert!(spindle_thread.join().unwrap().is_ok());
}

#[allow(clippy::too_many_arguments)]
fn verify_real_cursor_parity(
    root: &Path,
    data: &Path,
    temp: &Path,
    socket: &Path,
    origin: &str,
    epoch: &str,
    after: u64,
    direct: &fabric_collect::live_cursor::LivePage,
) {
    use std::io::{Read, Write};
    use std::os::unix::fs::OpenOptionsExt;
    use std::process::{Child, Stdio};
    struct Guard {
        child: Child,
        receiver: std::sync::mpsc::Receiver<Result<Vec<u8>, String>>,
        drain: Option<thread::JoinHandle<()>>,
    }
    impl Guard {
        fn new(mut child: Child, lines: bool) -> Self {
            let mut output = child.stdout.take().unwrap();
            let (sender, receiver) = std::sync::mpsc::channel();
            let drain = thread::spawn(move || {
                let mut total = 0usize;
                let mut messages = 0usize;
                let mut pending = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let read = match output.read(&mut chunk) {
                        Ok(n) => n,
                        Err(error) => {
                            let _ = sender.send(Err(error.to_string()));
                            return;
                        }
                    };
                    if read == 0 {
                        if !lines {
                            let _ = sender.send(Ok(pending));
                        } else if !pending.is_empty() {
                            let _ = sender.send(Err("unterminated MCP reply".into()));
                        }
                        return;
                    }
                    total += read;
                    if total > 1_048_576 {
                        let _ = sender.send(Err("subprocess output budget".into()));
                        return;
                    }
                    for &byte in &chunk[..read] {
                        if lines && byte == b'\n' {
                            messages += 1;
                            if messages > 3 {
                                let _ = sender.send(Err("MCP reply count".into()));
                                return;
                            }
                            if sender.send(Ok(std::mem::take(&mut pending))).is_err() {
                                return;
                            }
                        } else {
                            pending.push(byte);
                            if lines && pending.len() > 262_144 {
                                let _ = sender.send(Err("MCP line budget".into()));
                                return;
                            }
                        }
                    }
                }
            });
            Self {
                child,
                receiver,
                drain: Some(drain),
            }
        }
        fn receive(&self, deadline: Instant) -> Vec<u8> {
            self.receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("subprocess output deadline")
                .expect("bounded subprocess output")
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(drain) = self.drain.take() {
                let _ = drain.join();
            }
        }
    }
    fn output_file(path: &Path) -> fs::File {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap()
    }
    fn wait_response(child: &Guard, id: u64, deadline: Instant) -> serde_json::Value {
        let value: serde_json::Value =
            serde_json::from_slice(&child.receive(deadline)).expect("MCP JSON reply");
        assert_eq!(
            value.get("id").and_then(serde_json::Value::as_u64),
            Some(id)
        );
        value
    }
    let deadline = Instant::now() + Duration::from_secs(8);
    let expected = serde_json::to_value(direct).unwrap();
    assert!(direct.gap && direct.highest_lost > after);
    let cli = std::env::var_os("FABRIC_QUALIFY_CLI").expect("exact CLI executable");
    let mut cli = Guard::new(
        Command::new(cli)
            .arg("--data-dir")
            .arg(data)
            .arg("--tmp-dir")
            .arg(temp)
            .args(["--json", "live-metrics", "--socket"])
            .arg(socket)
            .args([
                "--origin",
                origin,
                "--epoch",
                epoch,
                "--after",
                &after.to_string(),
                "--count",
                "64",
                "--max-bytes",
                "32768",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
        false,
    );
    let cli_output = cli.receive(deadline);
    let status = loop {
        if let Some(status) = cli.child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "CLI response deadline");
        thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "CLI cursor failed: {status}");
    let cli_value: serde_json::Value = serde_json::from_slice(&cli_output).unwrap();
    assert_eq!(cli_value["schema_version"], "fabric.cli.v1");
    assert_eq!(cli_value["command"], "live-metrics");
    assert_eq!(cli_value["data"], expected);

    let credential = root.join("reader.key");
    let mut key = output_file(&credential);
    key.write_all(b"fixture-reader-credential").unwrap();
    drop(key);
    let mcp = std::env::var_os("FABRIC_QUALIFY_MCP").expect("exact MCP executable");
    let mut mcp = Guard::new(
        Command::new(mcp)
            .env("FABRIC_ENDPOINT", "127.0.0.1:9")
            .env_remove("FABRIC_CREDENTIAL")
            .env_remove("FABRIC_MCP_MODE")
            .env("FABRIC_CREDENTIAL_FILE", &credential)
            .env("FABRIC_MCP_LIVE_SOCKET", socket)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
        true,
    );
    let mut input = mcp.child.stdin.take().unwrap();
    writeln!(input, "{}", serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"real-cursor-parity","version":"1"}}})).unwrap();
    input.flush().unwrap();
    let initialized = wait_response(&mcp, 1, deadline);
    assert!(initialized["result"].is_object());
    writeln!(
        input,
        "{}",
        serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    writeln!(input, "{}", serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"live_metrics","arguments":{"origin":origin,"epoch":epoch,"after":after.to_string(),"count":64,"max_bytes":32768}}})).unwrap();
    input.flush().unwrap();
    let reply = wait_response(&mcp, 2, deadline);
    let framed: serde_json::Value = serde_json::from_str(
        reply["result"]["content"][0]["text"]
            .as_str()
            .expect("MCP cursor result"),
    )
    .unwrap();
    assert_eq!(framed["untrusted_telemetry"], true);
    let envelope = &framed["telemetry_payload"];
    assert_eq!(envelope["schema_version"], "fabric.cli.v1");
    assert_eq!(envelope["command"], "live-metrics");
    assert_eq!(envelope["data"], expected);
    println!(
        "real daemon cursor parity: CLI, MCP and direct pages equal; gap={} entries={} next_after={}",
        direct.gap,
        direct.entries.len(),
        direct.next_after
    );
}
