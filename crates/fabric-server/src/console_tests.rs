//! Role matrix and kept console behavior for ADR-0029: two enrollments with
//! distinct canary rows and freshness values. Sessions here are synthetic policy
//! fixtures, not a WebAuthn crypto oracle.
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
    reader: String,
    spindle: String,
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
                    name: "canary-metric".into(),
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
    let desired = || DesiredConfig {
        logs: vec![],
        metric_interval_s: 15,
    };
    let (_, spindle) = control.enroll("alpha", desired()).unwrap();
    control.enroll("secret", desired()).unwrap();
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
    let session = access.fixture_session(now()).unwrap();
    let owner = access
        .authenticate_session(&session.session_token, now())
        .unwrap();
    let reader = access
        .issue_reader(
            &owner,
            "fixture",
            3600,
            &session.csrf_token,
            "https://localhost",
            now(),
        )
        .unwrap()
        .token;
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
        reader,
        spindle,
    }
}
#[derive(Clone, Copy)]
enum Who<'a> {
    Anonymous,
    Bearer(&'a str),
    Owner(&'a str, &'a str),
}
async fn call_as(
    f: &Fixture,
    who: Who<'_>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let console = f.console.as_ref().unwrap();
    // Admission budgets have their own tests; these calls check authorization only.
    *console.auth_budget.lock().unwrap() = (0, 0);
    *console.private_auth_budget.lock().unwrap() = (0, 0);
    let mut request = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("x-fabric-client-version", "1")
        .header("Origin", "https://localhost")
        .header("Content-Type", "application/json");
    request = match who {
        Who::Anonymous => request,
        Who::Bearer(token) => request.header("Authorization", format!("Bearer {token}")),
        Who::Owner(token, csrf) => request
            .header("Cookie", format!("{COOKIE}={token}"))
            .header("x-fabric-csrf", csrf),
    };
    let response = console
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
    if path.starts_with("/v1/") {
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), RESPONSE_CAP)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
/// Owner session calls; `csrf_valid = false` replaces the CSRF token.
async fn call(
    f: &Fixture,
    path: &str,
    body: Option<Value>,
    csrf_valid: bool,
) -> (StatusCode, Value) {
    let csrf = if csrf_valid {
        &f.session.csrf_token
    } else {
        "wrong"
    };
    let who = Who::Owner(&f.session.session_token, csrf);
    call_as(
        f,
        who,
        if body.is_some() { "POST" } else { "GET" },
        path,
        body,
    )
    .await
}
#[tokio::test]
async fn owner_and_reader_read_every_source_and_signal_with_csrf_and_bounds() {
    let f = fixture();
    let query = json!({"kind":"logs","from_ns":0,"to_ns":10000,"limit":2});
    let (status, answer) = call(&f, "/v1/console/query", Some(query.clone()), true).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["rows"][0]["body"], "alpha-0");
    assert_eq!(answer["rows"][1]["body"], "alpha-1");
    // Independent fixture constants: logs observed at 100..=102 (alpha) and 800..=802 (secret).
    assert_eq!(answer["freshness"], json!({"alpha":102,"secret":802}));
    assert_eq!(answer["retained_from_ns"], 200);
    assert!(answer["retained_to_ns"].as_u64().unwrap() >= 900);
    assert!(!answer["snapshot"].as_str().unwrap().starts_with("g1-"));
    let token = answer["next_page"].as_str().unwrap();
    assert_eq!(token.len(), 64);
    let mut next = query;
    next["page"] = json!(token);
    let (status, page) = call(&f, "/v1/console/query", Some(next), true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["rows"][0]["body"], "alpha-2");
    assert_eq!(page["rows"][1]["body"], "secret-0");
    assert_eq!(page["snapshot"], answer["snapshot"]);
    let metrics =
        json!({"kind":"metrics","name":"canary-metric","from_ns":0,"to_ns":10000,"limit":4});
    let secret = json!({"kind":"logs","node":"secret","from_ns":0,"to_ns":10000,"limit":3});
    let owner = Who::Owner(&f.session.session_token, &f.session.csrf_token);
    for who in [owner, Who::Bearer(&f.reader)] {
        let (status, nodes) = call_as(&f, who, "GET", "/v1/console/nodes", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(nodes["nodes"].as_array().unwrap().len(), 2);
        let (status, m) =
            call_as(&f, who, "POST", "/v1/console/query", Some(metrics.clone())).await;
        assert_eq!(status, StatusCode::OK, "{m}");
        assert_eq!(m["rows"].as_array().unwrap().len(), 2);
        let (status, s) = call_as(&f, who, "POST", "/v1/console/query", Some(secret.clone())).await;
        assert_eq!(status, StatusCode::OK, "{s}");
        assert_eq!(s["rows"][0]["body"], "secret-0");
    }
    let wrong_csrf = call(&f, "/v1/console/query", Some(secret), false).await;
    assert_eq!(wrong_csrf.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn pending_journal_uses_the_same_evidence_as_sealed_storage() {
    let f = fixture_storage(false);
    let (status, answer) = call(
        &f,
        "/v1/console/query",
        Some(json!({"kind":"logs","from_ns":0,"to_ns":10000,"limit":6})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["freshness"], json!({"alpha":102,"secret":802}));
    assert_eq!(answer["retained_from_ns"], 200);
    assert!(answer["retained_to_ns"].as_u64().unwrap() >= 900);
    assert_eq!(
        answer["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["body"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "alpha-0", "alpha-1", "alpha-2", "secret-0", "secret-1", "secret-2"
        ]
    );
}

#[tokio::test]
async fn freshness_covers_retained_logs_beyond_the_query_window() {
    // Origin: hosted browser run 38172604195. An edge Spindle also collects its own
    // diagnostics, so a newer log can arrive after a query window was fixed. Freshness
    // is the source's newest retained observation of the signal, not its newest row.
    for sealed in [true, false] {
        let f = fixture_storage(sealed);
        let body =
            |to_ns| json!({"kind":"logs","node":"secret","from_ns":0,"to_ns":to_ns,"limit":10});
        let newest = |answer: &Value| {
            let rows = answer["rows"].as_array().unwrap();
            rows.iter()
                .map(|r| r["observed_ns"].as_u64().unwrap())
                .max()
        };
        let (status, narrow) = call(&f, "/v1/console/query", Some(body(801)), true).await;
        assert_eq!(
            (status, newest(&narrow)),
            (StatusCode::OK, Some(800)),
            "{narrow}"
        );
        assert_eq!(narrow["freshness"], json!({"secret":802}));
        // A window that covers every retained log agrees with its own rows.
        let (_, wide) = call(&f, "/v1/console/query", Some(body(10000)), true).await;
        assert_eq!(newest(&wide), Some(802));
        assert_eq!(wide["freshness"], json!({"secret":802}));
    }
}

#[tokio::test]
async fn opaque_cursor_rejects_other_credentials_and_expiration() {
    let f = fixture();
    let mut query = json!({"kind":"logs","from_ns":0,"to_ns":10000,"limit":1});
    let (status, first) = call(&f, "/v1/console/query", Some(query.clone()), true).await;
    assert_eq!(status, StatusCode::OK);
    let page = first["next_page"].as_str().unwrap();
    query["page"] = json!(page);
    // The reader sees the same data but cannot reuse the owner's page handle.
    let (status, _) = call_as(
        &f,
        Who::Bearer(&f.reader),
        "POST",
        "/v1/console/query",
        Some(query.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
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
async fn legacy_master_and_unbounded_queries_are_rejected_on_console_router() {
    let f = fixture();
    let master = Who::Bearer("legacy-master");
    let (status, _) = call_as(&f, master, "GET", "/v1/admin/nodes", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    for body in [
        json!({"kind":"rate","name":"canary-metric","from_ns":0,"to_ns":10000}),
        json!({"kind":"logs","from_ns":0,"to_ns":10000,"limit":1001}),
        json!({"kind":"logs","from_ns":0,"to_ns":86_400_000_000_001u64,"limit":1}),
    ] {
        assert_eq!(
            call(&f, "/v1/console/query", Some(body), true).await.0,
            StatusCode::BAD_REQUEST
        );
    }
}
#[tokio::test]
async fn unknown_credentials_are_rejected_without_reading_a_body() {
    let f = fixture();
    for (path, version) in [("/v1/console/query", true), ("/v1/batches", false)] {
        let mut request = axum::http::Request::builder()
            .method("POST")
            .uri(path)
            .header("Content-Length", "999999999");
        if version {
            request = request.header("x-fabric-client-version", "1");
        }
        let response = f
            .console
            .as_ref()
            .unwrap()
            .clone()
            .router()
            .oneshot(request.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Min {
    Anyone,
    Spindle,
    Reader,
    Owner,
    Never,
}
/// Method, route template, concrete path ("" = the template), JSON body ("" = none),
/// minimum credential and the status an admitted caller gets. Written from the
/// ADR-0029 matrix, not from the guard.
#[rustfmt::skip]
const MATRIX: [(&str, &str, &str, &str, Min, u16); 30] = [
    ("GET", "/v1/health", "", "", Min::Anyone, 200),
    ("GET", "/console", "", "", Min::Anyone, 308),
    ("GET", "/console/", "", "", Min::Anyone, 404),
    ("GET", "/console/{*asset}", "/console/app.js", "", Min::Anyone, 404),
    ("POST", "/v1/console/auth/register/start", "", r#"{"bootstrap_secret":"x","display_name":"y"}"#, Min::Anyone, 403),
    ("POST", "/v1/console/auth/register/finish", "", "{}", Min::Anyone, 422),
    ("POST", "/v1/console/auth/login/start", "", r#"{"principal_id":"nobody"}"#, Min::Anyone, 200),
    ("POST", "/v1/console/auth/login/finish", "", "{}", Min::Anyone, 422),
    ("POST", "/v1/batches", "", "", Min::Spindle, 400),
    ("GET", "/v1/config", "", "", Min::Spindle, 200),
    ("GET", "/v1/admin/nodes", "", "", Min::Never, 200),
    ("POST", "/v1/console/query", "", r#"{"kind":"logs","from_ns":0,"to_ns":10000,"limit":2}"#, Min::Reader, 200),
    ("GET", "/v1/console/status", "", "", Min::Reader, 200),
    ("GET", "/v1/console/nodes", "", "", Min::Reader, 200),
    ("GET", "/v1/console/session", "", "", Min::Owner, 200),
    ("POST", "/v1/console/nodes", "", r#"{"name":"matrix"}"#, Min::Owner, 200),
    ("PUT", "/v1/console/nodes/{name}/config", "/v1/console/nodes/secret/config", r#"{"logs":[],"metric_interval_s":30}"#, Min::Owner, 200),
    ("POST", "/v1/console/nodes/{name}/{action}", "/v1/console/nodes/secret/pause", "{}", Min::Owner, 200),
    ("POST", "/v1/console/nodes/{name}/{action}", "/v1/console/nodes/secret/resume", "{}", Min::Owner, 200),
    ("GET", "/v1/console/readers", "", "", Min::Owner, 200),
    ("POST", "/v1/console/readers", "", r#"{"name":"matrix","ttl_s":60}"#, Min::Owner, 200),
    ("POST", "/v1/console/readers/{id}/revoke", "/v1/console/readers/unknown/revoke", "{}", Min::Owner, 403),
    ("GET", "/v1/console/audit", "", "", Min::Owner, 200),
    ("GET", "/v1/console/passkeys", "", "", Min::Owner, 200),
    ("POST", "/v1/console/passkeys/start", "", "{}", Min::Owner, 200),
    ("POST", "/v1/console/passkeys/{id}/revoke", "/v1/console/passkeys/unknown/revoke", "{}", Min::Owner, 403),
    ("GET", "/v1/console/sessions", "", "", Min::Owner, 200),
    ("POST", "/v1/console/sessions/{id}/revoke", "/v1/console/sessions/unknown/revoke", "{}", Min::Owner, 403),
    ("POST", "/v1/console/nodes/{name}/{action}", "/v1/console/nodes/secret/revoke", "{}", Min::Owner, 200),
    ("POST", "/v1/console/logout", "", "{}", Min::Owner, 200),
];
#[tokio::test]
async fn role_matrix_admits_exactly_the_documented_credentials_on_every_route() {
    // Every route registered by the console router must be in the table.
    for chunk in include_str!("console.rs").split(".route(").skip(1) {
        let quoted = chunk.trim_start().strip_prefix('"').unwrap();
        let template = quoted.split('"').next().unwrap();
        assert!(
            MATRIX.iter().any(|row| row.1 == template),
            "{template} is missing from the role matrix"
        );
    }
    let f = fixture();
    let console = f.console.as_ref().unwrap();
    for (method, template, concrete, body, min, ok) in MATRIX {
        let path = if concrete.is_empty() {
            template
        } else {
            concrete
        };
        let body = serde_json::from_str::<Value>(body).ok();
        for (name, rank) in ["anonymous", "spindle", "reader", "owner"]
            .into_iter()
            .zip(0..)
        {
            // A fresh owner session per call: logout retires its session.
            let session = console
                .access
                .lock()
                .unwrap()
                .fixture_session(now())
                .unwrap();
            let credential = match rank {
                0 => Who::Anonymous,
                1 => Who::Bearer(&f.spindle),
                2 => Who::Bearer(&f.reader),
                _ => Who::Owner(&session.session_token, &session.csrf_token),
            };
            let admitted = match min {
                Min::Anyone => true,
                Min::Spindle => rank == 1,
                Min::Reader => rank >= 2,
                Min::Owner => rank == 3,
                Min::Never => false,
            };
            let (status, answer) = call_as(&f, credential, method, path, body.clone()).await;
            let role_denied = !admitted && rank == 2 && min == Min::Owner;
            let expected = match (admitted, role_denied) {
                (true, _) => ok,
                (false, true) => 403,
                (false, false) => 401,
            };
            assert_eq!(
                status.as_u16(),
                expected,
                "{name} {method} {path}: {answer}"
            );
            if role_denied {
                assert_eq!(answer["error"], ROLE_DENIED, "{name} {method} {path}");
            }
        }
    }
}

/// Hold a `GET /v1/console/status` inside the authorized handler while `during` runs.
async fn status_held_during(
    console: &Console,
    token: &str,
    during: impl std::future::Future<Output = ()>,
) -> (StatusCode, String) {
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
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let pending = tokio::spawn(router.oneshot(request));
    tokio::time::timeout(std::time::Duration::from_secs(3), admitted.notified())
        .await
        .unwrap();
    during.await;
    release.notify_one();
    let response = pending.await.unwrap().unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn reader_get_cannot_publish_after_revocation() {
    // Counterexample origin: GET admission authenticated the reader; revocation
    // between admission and handler return must stop publication.
    let mut f = fixture_storage(false);
    let console = f.console.take().unwrap();
    let revoke = async {
        let mut access = console.access.lock().unwrap();
        let owner = access
            .authenticate_session(&f.session.session_token, now())
            .unwrap();
        let id = access.readers(&owner).unwrap()[0].id.clone();
        access
            .revoke_reader(
                &owner,
                &id,
                &f.session.csrf_token,
                "https://localhost",
                now(),
            )
            .unwrap();
    };
    let (status, body) = status_held_during(&console, &f.reader, revoke).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!body.contains("private inventory"));
    drop(console);
}

#[tokio::test]
async fn reader_get_cannot_publish_after_expiry() {
    // Same controlled barrier, with a real wall-clock deadline rather than a
    // mocked future clock that could accidentally test rollback quarantine.
    let mut f = fixture_storage(false);
    let console = f.console.take().unwrap();
    let issued = {
        let mut access = console.access.lock().unwrap();
        let owner = access
            .authenticate_session(&f.session.session_token, now())
            .unwrap();
        let (csrf, origin) = (&f.session.csrf_token, "https://localhost");
        access
            .issue_reader(&owner, "expires-during-get", 2, csrf, origin, now())
            .unwrap()
    };
    let wait = async {
        while now() < issued.expires_unix_s {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    };
    let (status, body) = status_held_during(&console, &issued.token, wait).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!body.contains("private inventory"));
    drop(console);
}
