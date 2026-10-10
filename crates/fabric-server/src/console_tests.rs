//! Fixed source-isolation counterexample: another enrollment and another signal
//! have later timestamps. Filtering just returned rows leaks both in metadata.
//! Sessions here are synthetic policy fixtures, not a WebAuthn crypto oracle.
use super::*;
use crate::access::AccessConfig;
use crate::control::Control;
use crate::query::{History, Plan};
use crate::store::{CommitMode, Entry, Group, Store};
use fabric_frame::envelope::Batch;
use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
};
use opentelemetry_proto::tonic::common::v1::{AnyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, metric, number_data_point,
};
use prost::Message;
use std::sync::atomic::{AtomicU64, Ordering};
use tower::ServiceExt;
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    console: Option<Console>,
    root: std::path::PathBuf,
    join: Option<std::thread::JoinHandle<()>>,
    session: crate::access::SessionIssued,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.console.take());
        self.join.take().unwrap().join().unwrap();
        if !std::thread::panicking() {
            std::fs::remove_dir_all(&self.root).unwrap();
        } else {
            eprintln!("console failure fixture retained: {}", self.root.display());
        }
    }
}
fn entry(label: &str, id: u8, time: u64, received: u64) -> Entry {
    let logs = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: (0..3)
                    .map(|i| LogRecord {
                        observed_time_unix_nano: time + i,
                        body: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(format!("{label}-{i}"))),
                        }),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    let metrics = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![Metric {
                    name: "private-metric".into(),
                    data: Some(metric::Data::Gauge(Gauge {
                        data_points: vec![NumberDataPoint {
                            time_unix_nano: 9999,
                            value: Some(number_data_point::Value::AsInt(9007199254740993)),
                            ..Default::default()
                        }],
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    Entry {
        label: label.into(),
        batch: Batch {
            version: 1,
            node_id: vec![id; 16],
            generation: 1,
            sequence: 1,
            logs,
            metrics,
            ..Default::default()
        }
        .encode_to_vec(),
        received_unix_nano: received,
    }
}
fn fixture() -> Fixture {
    fixture_storage(true)
}
fn fixture_storage(sealed: bool) -> Fixture {
    let root = std::path::PathBuf::from(
        std::env::var("FABRIC_SCRATCH_ROOT").expect("contained scratch required"),
    )
    .join(format!(
        "console-access-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&root).unwrap();
    let mut control = Control::open(&root).unwrap();
    let (alpha, _) = control
        .enroll(
            "alpha",
            DesiredConfig {
                logs: vec![],
                metric_interval_s: 15,
            },
        )
        .unwrap();
    control
        .enroll(
            "secret",
            DesiredConfig {
                logs: vec![],
                metric_interval_s: 15,
            },
        )
        .unwrap();
    let group = Group {
        group_sequence: 1,
        entries: vec![entry("alpha", 1, 100, 200), entry("secret", 2, 800, 900)],
    };
    std::fs::create_dir(root.join("journal")).unwrap();
    let mut log = fabric_frame::frame::FrameLog::open(
        &root.join("journal"),
        16 * 1024 * 1024,
        4 * 1024 * 1024,
        |_, _| Ok(()),
    )
    .unwrap();
    log.append(&group.encode_to_vec()).unwrap();
    drop(log);
    if sealed {
        crate::segment::build(&root, 1, &[group]).unwrap();
    }
    let (intake, join) = Store::open(&root, 16 * 1024 * 1024, CommitMode::GROUPED)
        .unwrap()
        .spawn_joinable()
        .unwrap();
    let mut access = Access::open(
        &root.join("access"),
        AccessConfig {
            origin: "https://localhost".into(),
            rp_id: "localhost".into(),
            audience: "https://localhost".into(),
        },
        now(),
    )
    .unwrap();
    let mut scope = Scope::owner();
    scope.installation_wide = false;
    scope.enrollments = [alpha.enrollment_id()].into();
    scope.signals = [Signal::Logs].into();
    scope.actions = [Action::TelemetryRead, Action::InventoryRead].into();
    let session = access.fixture_session(scope, now()).unwrap();
    let app = AppState {
        intake,
        control: Arc::new(Mutex::new(control)),
        admin_token_sha256: Sha256::digest(b"legacy-master").into(),
        history: Arc::new(History::with_plan(&root, Plan::Scan)),
    };
    let config = crate::config::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        tls_cert: root.join("cert"),
        tls_key: root.join("key"),
        state_dir: root.clone(),
        admin_token_file: root.join("admin"),
        journal_bytes: 16 * 1024 * 1024,
        journal_file_bytes: 65536,
        retention_s: 86400,
        retention_bytes: 100000000000,
        query_plan: Plan::Scan,
        seal_workers: 1,
        console_dir: None,
        access_origin: Some("https://localhost".into()),
        access_rp_id: Some("localhost".into()),
    };
    Fixture {
        console: Some(Console::new(app, access, Assets::default(), &config)),
        root,
        join: Some(join),
        session,
    }
}
async fn call(
    f: &Fixture,
    path: &str,
    body: Option<Value>,
    csrf_valid: bool,
) -> (StatusCode, Value) {
    let mut request = axum::http::Request::builder()
        .method(if body.is_some() { "POST" } else { "GET" })
        .uri(path)
        .header("x-fabric-client-version", "1")
        .header("Origin", "https://localhost")
        .header("Cookie", format!("{COOKIE}={}", f.session.session_token))
        .header(
            "x-fabric-csrf",
            if csrf_valid {
                &f.session.csrf_token
            } else {
                "wrong"
            },
        )
        .header("Content-Type", "application/json");
    if body.is_none() {
        request = request.method("GET");
    }
    let response = f
        .console
        .as_ref()
        .unwrap()
        .clone()
        .router()
        .oneshot(
            request
                .body(axum::body::Body::from(
                    body.map_or_else(String::new, |v| v.to_string()),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), RESPONSE_CAP)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
#[tokio::test]
async fn scoped_rows_metadata_signals_inventory_and_csrf() {
    let f = fixture();
    let query = json!({"kind":"logs","from_ns":0,"to_ns":10000,"limit":2});
    let (status, answer) = call(&f, "/v1/console/query", Some(query.clone()), true).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["rows"].as_array().unwrap().len(), 2);
    assert_eq!(answer["rows"][0]["body"], "alpha-0");
    assert_eq!(answer["freshness"], json!({"alpha":102}));
    assert_eq!(answer["retained_from_ns"], 200);
    assert_eq!(answer["retained_to_ns"], 200);
    assert!(!answer.to_string().contains("secret"));
    assert!(!answer["snapshot"].as_str().unwrap().starts_with("g1-"));
    let token = answer["next_page"].as_str().unwrap();
    assert_eq!(token.len(), 64);
    let mut next = query;
    next["page"] = json!(token);
    let (status, page) = call(&f, "/v1/console/query", Some(next), true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["rows"][0]["body"], "alpha-2");
    assert_eq!(page["snapshot"], answer["snapshot"]);
    let (_, inventory) = call(&f, "/v1/console/nodes", None, true).await;
    assert_eq!(inventory["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(inventory["nodes"][0]["name"], "alpha");
    assert_eq!(call(&f,"/v1/console/query",Some(json!({"kind":"metrics","name":"private-metric","from_ns":0,"to_ns":10000,"limit":2})),true).await.0,StatusCode::FORBIDDEN);
    assert_eq!(
        call(
            &f,
            "/v1/console/query",
            Some(json!({"kind":"logs","node":"secret","from_ns":0,"to_ns":10000,"limit":2})),
            true
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &f,
            "/v1/console/query",
            Some(json!({"kind":"logs","from_ns":0,"to_ns":10000,"limit":2})),
            false
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&f, "/v1/console/nodes/alpha/pause", Some(json!({})), true)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn pending_journal_uses_the_same_authorized_evidence_as_sealed_storage() {
    let f = fixture_storage(false);
    let (status, answer) = call(
        &f,
        "/v1/console/query",
        Some(json!({"kind":"logs","from_ns":0,"to_ns":10000,"limit":3})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["freshness"], json!({"alpha":102}));
    assert_eq!(answer["retained_from_ns"], 200);
    assert_eq!(answer["retained_to_ns"], 200);
    assert_eq!(
        answer["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["body"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["alpha-0", "alpha-1", "alpha-2"]
    );
    assert!(!answer.to_string().contains("secret"));
}

#[tokio::test]
async fn opaque_cursor_rejects_other_principal_and_expiration() {
    let mut f = fixture();
    let mut query = json!({"kind":"logs","from_ns":0,"to_ns":10000,"limit":1});
    let (status, first) = call(&f, "/v1/console/query", Some(query.clone()), true).await;
    assert_eq!(status, StatusCode::OK);
    let page = first["next_page"].as_str().unwrap();
    query["page"] = json!(page);
    // A distinct, even more privileged account cannot reuse the first account's handle.
    f.session = f
        .console
        .as_ref()
        .unwrap()
        .access
        .lock()
        .unwrap()
        .fixture_session(Scope::owner(), now())
        .unwrap();
    assert_eq!(
        call(&f, "/v1/console/query", Some(query.clone()), true)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    f.console
        .as_ref()
        .unwrap()
        .cursors
        .lock()
        .unwrap()
        .get_mut(page)
        .unwrap()
        .expires = 0;
    assert_eq!(
        call(&f, "/v1/console/query", Some(query), true).await.0,
        StatusCode::GONE
    );
}
#[tokio::test]
async fn legacy_master_and_unbounded_rate_are_rejected_on_scoped_router() {
    let f = fixture();
    let request = axum::http::Request::builder()
        .uri("/v1/admin/nodes")
        .header("Authorization", "Bearer legacy-master")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = f
        .console
        .as_ref()
        .unwrap()
        .clone()
        .router()
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        call(
            &f,
            "/v1/console/query",
            Some(json!({"kind":"rate","name":"private-metric","from_ns":0,"to_ns":10000})),
            true
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &f,
            "/v1/console/query",
            Some(json!({"kind":"logs","from_ns":0,"to_ns":10000,"limit":1001})),
            true
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}
#[tokio::test]
async fn unknown_credentials_are_rejected_without_reading_a_body() {
    let f = fixture();
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/console/query")
        .header("x-fabric-client-version", "1")
        .header("Content-Length", "999999999")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = f
        .console
        .as_ref()
        .unwrap()
        .clone()
        .router()
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/batches")
        .header("Content-Length", "999999999")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = f
        .console
        .as_ref()
        .unwrap()
        .clone()
        .router()
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn delegated_get_cannot_publish_after_parent_logout() {
    // Counterexample origin: GET admission authenticated the delegation, but
    // parent logout between admission and handler return did not bump policy.
    // Final read publication must recheck the exact delegation parent session.
    let mut f = fixture_storage(false);
    let console = f.console.take().unwrap();
    let (human, delegated) = {
        let mut access = console.access.lock().unwrap();
        let scope = access
            .authenticate_session(&f.session.session_token, now())
            .unwrap()
            .scope;
        let human = access.fixture_session(Scope::owner(), now()).unwrap();
        let auth = access
            .authenticate_session(&human.session_token, now())
            .unwrap();
        let workload = access
            .issue_workload(
                &auth,
                "barrier-reader",
                scope.clone(),
                3600,
                &human.csrf_token,
                "https://localhost",
                now(),
            )
            .unwrap();
        let auth = access
            .authenticate_session(&human.session_token, now())
            .unwrap();
        let delegated = access
            .delegate(
                &auth,
                &workload.principal_id,
                scope,
                300,
                &human.csrf_token,
                "https://localhost",
                now(),
            )
            .unwrap();
        (human, delegated)
    };
    let admitted = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let route_admitted = admitted.clone();
    let route_release = release.clone();
    let router = Router::new()
        .route(
            "/v1/console/status",
            get(move || {
                let admitted = route_admitted.clone();
                let release = route_release.clone();
                async move {
                    admitted.notify_one();
                    release.notified().await;
                    Json(json!({"must_not_publish":"private inventory"}))
                }
            }),
        )
        .layer(middleware::from_fn_with_state(console.clone(), authorize));
    let request = Request::builder()
        .uri("/v1/console/status")
        .header("x-fabric-client-version", "1")
        .header("authorization", format!("Bearer {}", delegated.token))
        .body(axum::body::Body::empty())
        .unwrap();
    let pending = tokio::spawn(router.oneshot(request));
    tokio::time::timeout(std::time::Duration::from_secs(3), admitted.notified())
        .await
        .unwrap();
    {
        let mut access = console.access.lock().unwrap();
        let auth = access
            .authenticate_session(&human.session_token, now())
            .unwrap();
        access
            .logout(&auth, &human.csrf_token, "https://localhost", now())
            .unwrap();
    }
    release.notify_one();
    let response = pending.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("private inventory"));
    drop(console);
}

#[tokio::test]
async fn workload_get_cannot_publish_after_expiry() {
    // Same controlled barrier, with a real wall-clock deadline rather than a
    // mocked future clock that could accidentally test rollback quarantine.
    let mut f = fixture_storage(false);
    let console = f.console.take().unwrap();
    let credential = {
        let mut access = console.access.lock().unwrap();
        let scope = access
            .authenticate_session(&f.session.session_token, now())
            .unwrap()
            .scope;
        let human = access.fixture_session(Scope::owner(), now()).unwrap();
        let auth = access
            .authenticate_session(&human.session_token, now())
            .unwrap();
        access
            .issue_workload(
                &auth,
                "expires-during-get",
                scope,
                2,
                &human.csrf_token,
                "https://localhost",
                now(),
            )
            .unwrap()
    };
    let admitted = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let route_admitted = admitted.clone();
    let route_release = release.clone();
    let router = Router::new()
        .route(
            "/v1/console/status",
            get(move || {
                let admitted = route_admitted.clone();
                let release = route_release.clone();
                async move {
                    admitted.notify_one();
                    release.notified().await;
                    Json(json!({"must_not_publish":"private inventory"}))
                }
            }),
        )
        .layer(middleware::from_fn_with_state(console.clone(), authorize));
    let request = Request::builder()
        .uri("/v1/console/status")
        .header("x-fabric-client-version", "1")
        .header("authorization", format!("Bearer {}", credential.token))
        .body(axum::body::Body::empty())
        .unwrap();
    let pending = tokio::spawn(router.oneshot(request));
    tokio::time::timeout(std::time::Duration::from_secs(3), admitted.notified())
        .await
        .unwrap();
    while now() < credential.expires_unix_s {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    release.notify_one();
    let response = pending.await.unwrap().unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("private inventory"));
    drop(console);
}
