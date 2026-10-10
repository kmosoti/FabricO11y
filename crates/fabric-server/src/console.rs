//! Same-origin operator API. Access is decided before body extraction; read
//! authority is checked again before releasing results. No master-token fallback.
use crate::access::{Access, Action, Authority, Scope, Signal};
use crate::control::{DesiredConfig, Status};
use crate::http::AppState;
use crate::query::{Query, QueryError};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::io::{self, Read};
use std::path::Path as FsPath;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use webauthn_rs::prelude::{PublicKeyCredential, RegisterPublicKeyCredential};

const COOKIE: &str = "__Host-fabric-session";
const RESPONSE_CAP: usize = 8 * 1024 * 1024;
const CURSOR_CAP: usize = 1024;

#[derive(Clone)]
pub struct Console {
    app: AppState,
    access: Arc<Mutex<Access>>,
    queries: Arc<Semaphore>,
    requests: Arc<Semaphore>,
    ceremonies: Arc<Semaphore>,
    cursors: Arc<Mutex<BTreeMap<String, Cursor>>>,
    auth_budget: Arc<Mutex<(u64, u32)>>,
    private_auth_budget: Arc<Mutex<(u64, u32)>>,
    assets: Arc<Assets>,
    policy: Value,
}
struct Cursor {
    principal: String,
    policy: u64,
    shape: String,
    raw: String,
    snapshot: String,
    expires: u64,
}
#[derive(Default)]
pub struct Assets {
    files: BTreeMap<String, (Bytes, &'static str)>,
    headers: HeaderMap,
}
#[derive(Deserialize)]
struct AssetIdentity {
    sha256: String,
    bytes: usize,
}
impl Assets {
    pub fn load(dir: &FsPath) -> io::Result<Self> {
        let read = |name: &str, cap: usize| -> io::Result<Vec<u8>> {
            let path = dir.join(name);
            if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
                return Err(io::Error::other("symlink in console assets"));
            }
            let mut bytes = Vec::new();
            std::fs::File::open(path)?
                .take((cap + 1) as u64)
                .read_to_end(&mut bytes)?;
            if bytes.len() > cap {
                return Err(io::Error::other("console asset size cap"));
            }
            Ok(bytes)
        };
        let manifest: BTreeMap<String, AssetIdentity> =
            serde_json::from_slice(&read("asset-manifest.json", 64 * 1024)?)?;
        if manifest.len() > 64
            || !["index.html", "manifest.webmanifest", "service-worker.js"]
                .iter()
                .all(|n| manifest.contains_key(*n))
        {
            return Err(io::Error::other("incomplete console assets"));
        }
        let headers: BTreeMap<String, String> =
            serde_json::from_slice(&read("console-headers.json", 64 * 1024)?)?;
        let mut out = Self::default();
        let mut total = 0usize;
        for (name, identity) in manifest {
            if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
                return Err(io::Error::other("invalid console asset name"));
            }
            let bytes = read(&name, RESPONSE_CAP)?;
            total = total
                .checked_add(bytes.len())
                .ok_or_else(|| io::Error::other("console assets overflow"))?;
            if total > RESPONSE_CAP
                || bytes.len() != identity.bytes
                || hex(&Sha256::digest(&bytes)) != identity.sha256
            {
                return Err(io::Error::other("console asset identity mismatch"));
            }
            let mime = if name.ends_with(".wasm") {
                "application/wasm"
            } else if name.ends_with(".js") {
                "text/javascript; charset=utf-8"
            } else if name.ends_with(".css") {
                "text/css; charset=utf-8"
            } else if name.ends_with(".png") {
                "image/png"
            } else if name.ends_with(".svg") {
                "image/svg+xml"
            } else if name.ends_with(".webmanifest") {
                "application/manifest+json"
            } else {
                "text/html; charset=utf-8"
            };
            out.files.insert(name, (Bytes::from(bytes), mime));
        }
        for (name, value) in headers {
            let name =
                axum::http::HeaderName::from_bytes(name.as_bytes()).map_err(io::Error::other)?;
            if ![
                header::CONTENT_SECURITY_POLICY,
                header::X_CONTENT_TYPE_OPTIONS,
                header::REFERRER_POLICY,
            ]
            .contains(&name)
            {
                return Err(io::Error::other("unrecognized console asset header"));
            }
            out.headers.insert(
                name,
                HeaderValue::from_str(&value).map_err(io::Error::other)?,
            );
        }
        if !out.headers.contains_key(header::CONTENT_SECURITY_POLICY) {
            return Err(io::Error::other("console CSP required"));
        }
        Ok(out)
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}
fn random() -> io::Result<String> {
    let mut bytes = [0; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(hex(&bytes))
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |v| v.as_secs())
}
fn err(status: StatusCode, message: &'static str) -> Response {
    (status, Json(json!({"error":message}))).into_response()
}
fn denied() -> Response {
    err(StatusCode::FORBIDDEN, "access denied")
}
fn failure(e: io::Error) -> Response {
    if e.kind() == io::ErrorKind::PermissionDenied {
        denied()
    } else {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "access operation unavailable",
        )
    }
}
fn text<'a>(headers: &'a HeaderMap, key: &str) -> &'a str {
    headers.get(key).and_then(|v| v.to_str().ok()).unwrap_or("")
}
fn csrf(headers: &HeaderMap) -> &str {
    text(headers, "x-fabric-csrf")
}
fn origin(headers: &HeaderMap) -> &str {
    text(headers, "origin")
}
fn credentials(headers: &HeaderMap) -> Option<(&str, bool)> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let cookies = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok());
    let mut session = None;
    for cookie in cookies {
        for field in cookie.split(';') {
            if let Some((name, value)) = field.trim().split_once('=')
                && name == COOKIE
                && session.replace(value).is_some()
            {
                return None;
            }
        }
    }
    match (bearer, session) {
        (Some(b), None) => b.strip_prefix("Bearer ").map(|t| (t, false)),
        (None, Some(s)) => Some((s, true)),
        _ => None,
    }
}
impl Console {
    pub fn new(
        app: AppState,
        access: Access,
        assets: Assets,
        config: &crate::config::Config,
    ) -> Self {
        Self {
            app,
            access: Arc::new(Mutex::new(access)),
            queries: Arc::new(Semaphore::new(2)),
            requests: Arc::new(Semaphore::new(16)),
            ceremonies: Arc::new(Semaphore::new(8)),
            cursors: Arc::new(Mutex::new(BTreeMap::new())),
            auth_budget: Arc::new(Mutex::new((0, 0))),
            private_auth_budget: Arc::new(Mutex::new((0, 0))),
            assets: Arc::new(assets),
            policy: json!({"retention_bytes":config.retention_bytes,"retention_s":config.retention_s,"journal_bytes":config.journal_bytes,"journal_file_bytes":config.journal_file_bytes,"seal_workers":config.seal_workers}),
        }
    }
    pub fn router(self) -> Router {
        let private = Router::new()
            .route("/v1/console/session", get(session))
            .route("/v1/console/logout", post(logout))
            .route("/v1/console/query", post(query))
            .route("/v1/console/status", get(status))
            .route("/v1/console/nodes", get(nodes).post(enroll))
            .route("/v1/console/nodes/{name}/config", put(configure))
            .route("/v1/console/nodes/{name}/{action}", post(node_action))
            .route("/v1/console/principals", get(principals).post(invite))
            .route("/v1/console/workloads", post(workload))
            .route("/v1/console/workloads/{id}/rotate", post(rotate_workload))
            .route("/v1/console/delegations", post(delegate))
            .route("/v1/console/principals/{id}/scope", put(set_scope))
            .route("/v1/console/principals/{id}/disable", post(disable))
            .route(
                "/v1/console/credentials/{id}/revoke",
                post(revoke_credential),
            )
            .route("/v1/console/audit", get(audit))
            .route("/v1/console/passkeys", get(passkeys))
            .route("/v1/console/passkeys/start", post(add_passkey))
            .route("/v1/console/passkeys/{id}/revoke", post(revoke_passkey))
            .route("/v1/console/sessions", get(sessions))
            .route("/v1/console/sessions/{id}/revoke", post(revoke_session))
            .layer(middleware::from_fn_with_state(self.clone(), authorize));
        let public = Router::new()
            .route("/v1/console/auth/register/start", post(register_start))
            .route("/v1/console/auth/register/finish", post(register_finish))
            .route("/v1/console/auth/login/start", post(login_start))
            .route("/v1/console/auth/login/finish", post(login_finish))
            .route("/v1/console/auth/invite/start", post(invite_start))
            .layer(middleware::from_fn_with_state(self.clone(), auth_admission));
        let legacy = crate::http::router(self.app.clone()).layer(middleware::from_fn(block_master));
        private
            .merge(public)
            .route(
                "/console",
                get(|| async { axum::response::Redirect::permanent("/console/") }),
            )
            .route("/console/", get(index))
            .route("/console/{*asset}", get(asset))
            .layer(DefaultBodyLimit::max(64 * 1024))
            .with_state(self)
            .merge(legacy)
            .layer(middleware::from_fn(security_headers))
    }
}
async fn block_master(request: Request, next: Next) -> Response {
    if request.uri().path().starts_with("/v1/admin/") {
        err(StatusCode::UNAUTHORIZED, "use a scoped console credential")
    } else {
        next.run(request).await
    }
}
async fn security_headers(request: Request, next: Next) -> Response {
    let api = request.uri().path().starts_with("/v1/");
    let mut response = next.run(request).await;
    if api {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response.headers_mut().insert(
        header::STRICT_TRANSPORT_SECURITY,
        HeaderValue::from_static("max-age=31536000"),
    );
    response.headers_mut().insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    response.headers_mut().insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    // Static shell responses carry their build's script hashes. Non-shell
    // errors and JSON never need an executable document context.
    if !response
        .headers()
        .contains_key(header::CONTENT_SECURITY_POLICY)
    {
        response.headers_mut().insert(header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'"));
    }
    response
}
async fn auth_admission(State(s): State<Console>, request: Request, next: Next) -> Response {
    let Ok(_permit) = s.ceremonies.clone().try_acquire_owned() else {
        return err(
            StatusCode::TOO_MANY_REQUESTS,
            "authentication admission is full",
        );
    };
    if origin(request.headers()) != s.access.lock().unwrap().origin() {
        return denied();
    }
    if text(request.headers(), "x-fabric-client-version") != "1" {
        return err(StatusCode::UPGRADE_REQUIRED, "console update required");
    }
    {
        let mut budget = s.auth_budget.lock().unwrap();
        let time = now();
        if budget.0 != time {
            *budget = (time, 0);
        }
        if budget.1 >= 8 {
            return err(StatusCode::TOO_MANY_REQUESTS, "authentication rate limit");
        }
        budget.1 += 1;
    }
    tokio::time::timeout(std::time::Duration::from_secs(15), next.run(request))
        .await
        .unwrap_or_else(|_| err(StatusCode::REQUEST_TIMEOUT, "request deadline exceeded"))
}
async fn authorize(State(s): State<Console>, request: Request, next: Next) -> Response {
    let actor = Arc::new(Mutex::new(None));
    let response = authorize_request(s.clone(), request, next, actor.clone()).await;
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        // Do not persist attacker-controlled paths, bodies, credential values or
        // detailed policy failures. Access bounds and counts denial suppression.
        let actor = actor.lock().unwrap();
        if s.access
            .lock()
            .unwrap()
            .record_denial(actor.as_ref(), "console.request", None, now())
            .is_err()
        {
            return err(StatusCode::SERVICE_UNAVAILABLE, "access audit unavailable");
        }
    }
    response
}
async fn authorize_request(
    s: Console,
    mut request: Request,
    next: Next,
    actor: Arc<Mutex<Option<Authority>>>,
) -> Response {
    let is_read = request.method() == Method::GET;
    let Ok(_request_permit) = s.requests.clone().try_acquire_owned() else {
        return err(StatusCode::TOO_MANY_REQUESTS, "request admission is full");
    };
    {
        let mut budget = s.private_auth_budget.lock().unwrap();
        let time = now();
        if budget.0 != time {
            *budget = (time, 0);
        }
        if budget.1 >= 32 {
            return err(
                StatusCode::TOO_MANY_REQUESTS,
                "credential verification rate limit",
            );
        }
        budget.1 += 1;
    }
    if text(request.headers(), "x-fabric-client-version") != "1" {
        return err(StatusCode::UPGRADE_REQUIRED, "console update required");
    }
    let Some((token, cookie)) = credentials(request.headers()) else {
        return err(StatusCode::UNAUTHORIZED, "sign in required");
    };
    let auth = {
        let mut access = s.access.lock().unwrap();
        let found = if cookie {
            access.authenticate_session(token, now())
        } else {
            access.authenticate_workload(token, now())
        };
        let Ok(auth) = found else {
            return err(StatusCode::UNAUTHORIZED, "sign in required");
        };
        *actor.lock().unwrap() = Some(auth.clone());
        if request.method() != Method::GET
            && access
                .validate_mutation(
                    &auth,
                    csrf(request.headers()),
                    origin(request.headers()),
                    now(),
                    false,
                )
                .is_err()
        {
            return denied();
        }
        auth
    };
    // Permission checks before Bytes/JSON extraction. Body resource constraints
    // are rechecked in each handler against the same immutable authority.
    let path = request.uri().path();
    let required = if path.ends_with("/query") {
        Some(Action::TelemetryRead)
    } else if path == "/v1/console/nodes" && request.method() == Method::GET
        || path.ends_with("/status")
    {
        Some(Action::InventoryRead)
    } else if path.ends_with("/config") {
        Some(Action::NodeConfigure)
    } else if path.ends_with("/pause") {
        Some(Action::NodePause)
    } else if path.ends_with("/resume") {
        Some(Action::NodeResume)
    } else if path == "/v1/console/nodes" {
        Some(Action::NodeEnroll)
    } else if path.starts_with("/v1/console/nodes/") {
        Some(Action::NodeRevoke)
    } else if path == "/v1/console/audit" {
        Some(Action::AuditRead)
    } else if path.starts_with("/v1/console/workloads")
        || path.starts_with("/v1/console/credentials/")
        || path.ends_with("/scope")
    {
        Some(Action::GrantManage)
    } else if path.starts_with("/v1/console/principals") {
        Some(Action::IdentityManage)
    } else {
        None
    };
    if required.is_some_and(|a| !auth.scope.actions.contains(&a)) {
        return denied();
    }
    if request.method() != Method::GET
        && (path.starts_with("/v1/console/principals")
            || path.starts_with("/v1/console/workloads")
            || path.starts_with("/v1/console/credentials")
            || path.starts_with("/v1/console/delegations")
            || path.starts_with("/v1/console/passkeys")
            || path.starts_with("/v1/console/sessions"))
        && s.access
            .lock()
            .unwrap()
            .validate_mutation(
                &auth,
                csrf(request.headers()),
                origin(request.headers()),
                now(),
                true,
            )
            .is_err()
    {
        return denied();
    }
    let query_permit = if path.ends_with("/query") {
        match Arc::clone(&s.queries).try_acquire_owned() {
            Ok(p) => Some(Arc::new(p)),
            Err(_) => {
                let mut response = err(StatusCode::SERVICE_UNAVAILABLE, "query admission is full");
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
                return response;
            }
        }
    } else {
        None
    };
    if let Some(p) = &query_permit {
        request.extensions_mut().insert(Arc::clone(p));
    }
    request.extensions_mut().insert(auth.clone());
    let answer = tokio::time::timeout(std::time::Duration::from_secs(15), next.run(request))
        .await
        .unwrap_or_else(|_| err(StatusCode::REQUEST_TIMEOUT, "request deadline exceeded"));
    drop(query_permit);
    if is_read && s.access.lock().unwrap().recheck_at(&auth, now()).is_err() {
        return denied();
    }
    answer
}
async fn index(State(s): State<Console>) -> Response {
    asset_response(&s, "index.html")
}
async fn asset(State(s): State<Console>, Path(name): Path<String>) -> Response {
    asset_response(&s, &name)
}
fn asset_response(s: &Console, name: &str) -> Response {
    let Some((body, mime)) = s.assets.files.get(name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut answer = body.clone().into_response();
    *answer.headers_mut() = s.assets.headers.clone();
    answer
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    answer
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if name == "service-worker.js" {
        answer.headers_mut().insert(
            "service-worker-allowed",
            HeaderValue::from_static("/console/"),
        );
    }
    answer
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterStart {
    bootstrap_secret: String,
    display_name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterFinish {
    ceremony_id: String,
    credential: RegisterPublicKeyCredential,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginStart {
    principal_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginFinish {
    ceremony_id: String,
    credential: PublicKeyCredential,
}
fn json_result<T: serde::Serialize>(r: io::Result<T>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => failure(e),
    }
}
async fn register_start(State(s): State<Console>, Json(r): Json<RegisterStart>) -> Response {
    json_result(s.access.lock().unwrap().start_registration(
        &r.bootstrap_secret,
        &r.display_name,
        now(),
    ))
}
async fn login_start(State(s): State<Console>, Json(r): Json<LoginStart>) -> Response {
    json_result(s.access.lock().unwrap().start_login(&r.principal_id, now()))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InviteStart {
    invitation: String,
}
async fn invite_start(State(s): State<Console>, Json(r): Json<InviteStart>) -> Response {
    json_result(
        s.access
            .lock()
            .unwrap()
            .start_invited_registration(&r.invitation, now()),
    )
}
fn session_issued(result: io::Result<crate::access::SessionIssued>) -> Response {
    match result {
        Ok(session) => {
            let cookie = format!(
                "{COOKIE}={}; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=28800",
                session.session_token
            );
            let mut response = Json(session).into_response();
            response
                .headers_mut()
                .insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
            response
        }
        Err(e) => failure(e),
    }
}
async fn register_finish(
    State(s): State<Console>,
    h: HeaderMap,
    Json(r): Json<RegisterFinish>,
) -> Response {
    let mut access = s.access.lock().unwrap();
    let result = access
        .finish_registration(&r.ceremony_id, &r.credential, now())
        .and_then(|issued| {
            access.retire_previous_session(
                credentials(&h)
                    .filter(|(_, cookie)| *cookie)
                    .map(|(token, _)| token),
                &issued,
                now(),
            )?;
            Ok(issued)
        });
    session_issued(result)
}
async fn login_finish(
    State(s): State<Console>,
    h: HeaderMap,
    Json(r): Json<LoginFinish>,
) -> Response {
    let mut access = s.access.lock().unwrap();
    let result = access
        .finish_login(&r.ceremony_id, &r.credential, now())
        .and_then(|issued| {
            access.retire_previous_session(
                credentials(&h)
                    .filter(|(_, cookie)| *cookie)
                    .map(|(token, _)| token),
                &issued,
                now(),
            )?;
            Ok(issued)
        });
    session_issued(result)
}
async fn session(State(s): State<Console>, Extension(a): Extension<Authority>) -> Response {
    json_result(s.access.lock().unwrap().session_view(&a))
}
async fn logout(
    State(s): State<Console>,
    Extension(a): Extension<Authority>,
    h: HeaderMap,
) -> Response {
    let result = s
        .access
        .lock()
        .unwrap()
        .logout(&a, csrf(&h), origin(&h), now());
    let mut r = json_result(result.map(|()| json!({"signed_out":true})));
    r.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "__Host-fabric-session=; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
        ),
    );
    r
}
async fn status(State(s): State<Console>, Extension(a): Extension<Authority>) -> Response {
    let access = s.access.lock().unwrap();
    if !access.authorize(&a, Action::InventoryRead, None, None) {
        return denied();
    }
    let mut value = json!({"api_version":1,"scope":"authorized enrollments","query_rows_max":1000,"query_response_bytes_max":RESPONSE_CAP,"sampling":"request-time; observations remain in telemetry"});
    if a.scope.installation_wide {
        value["storage"] = s.policy.clone();
        value["committed_group"] = json!(s.app.intake.committed_group());
    }
    Json(value).into_response()
}
async fn nodes(State(s): State<Console>, Extension(a): Extension<Authority>) -> Response {
    let access = s.access.lock().unwrap();
    if !access.authorize(&a, Action::InventoryRead, None, None) {
        return denied();
    }
    let nodes:Vec<_>=s.app.control.lock().unwrap().inventory().into_iter().filter(|(r,_)|a.scope.allows(Action::InventoryRead,Some(&r.enrollment_id()),None)).map(|(r,o)|json!({"name":r.name,"enrollment_id":r.enrollment_id(),"status":r.status,"desired_revision":r.revision,"applied_revision":o.applied_revision,"config_error":o.config_error,"last_poll_unix_s":o.last_poll_unix_s,"logs":r.desired.logs,"metric_interval_s":r.desired.metric_interval_s})).collect();
    Json(json!({"nodes":nodes})).into_response()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Enroll {
    name: String,
    #[serde(default)]
    logs: Vec<String>,
    #[serde(default = "interval")]
    metric_interval_s: u64,
}
fn interval() -> u64 {
    15
}
fn desired_allowed(a: &Authority, d: &DesiredConfig) -> bool {
    d.metric_interval_s >= a.scope.min_interval_s
        && d.metric_interval_s <= a.scope.max_interval_s
        && (a.scope.installation_wide
            || d.logs.iter().all(|p| a.scope.allowed_log_paths.contains(p)))
}
async fn enroll(
    State(s): State<Console>,
    Extension(a): Extension<Authority>,
    h: HeaderMap,
    Json(r): Json<Enroll>,
) -> Response {
    let d = DesiredConfig {
        logs: r.logs,
        metric_interval_s: r.metric_interval_s,
    };
    let mut access = s.access.lock().unwrap();
    if !desired_allowed(&a, &d)
        || access
            .validate_mutation(&a, csrf(&h), origin(&h), now(), false)
            .is_err()
        || !access.authorize(&a, Action::NodeEnroll, None, None)
    {
        return denied();
    }
    let mut control = s.app.control.lock().unwrap();
    if !a.scope.installation_wide
        && !a
            .scope
            .enrollment_namespace
            .as_ref()
            .is_some_and(|prefix| r.name.starts_with(prefix))
    {
        return denied();
    }
    let population = control
        .inventory()
        .into_iter()
        .filter(|(r, _)| {
            a.scope.installation_wide
                || a.scope
                    .enrollment_namespace
                    .as_ref()
                    .is_some_and(|p| r.name.starts_with(p))
        })
        .count();
    if population >= a.scope.max_enrollments {
        return denied();
    }
    let intent = match access.begin_control(&a, Action::NodeEnroll, None, &r.name, now()) {
        Ok(i) => i,
        Err(e) => return failure(e),
    };
    let result = control.enroll(&r.name, d);
    if result.as_ref().is_err_and(|e| {
        !matches!(
            e.kind(),
            io::ErrorKind::InvalidInput | io::ErrorKind::AlreadyExists | io::ErrorKind::NotFound
        )
    }) {
        access.quarantine();
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "control publication requires offline reconciliation",
        );
    }
    if access
        .finish_control(&intent, &a, &r.name, result.is_ok(), now())
        .is_err()
    {
        access.quarantine();
        return failure(io::Error::other("control audit unavailable"));
    }
    json_result(result.map(|(r,token)|json!({"name":r.name,"enrollment_id":r.enrollment_id(),"revision":r.revision,"token":token})))
}
async fn configure(
    State(s): State<Console>,
    Extension(a): Extension<Authority>,
    h: HeaderMap,
    Path(name): Path<String>,
    Json(d): Json<DesiredConfig>,
) -> Response {
    if !desired_allowed(&a, &d) {
        return denied();
    }
    mutate_node(&s, &a, &h, &name, Action::NodeConfigure, Some(d))
}
async fn node_action(
    State(s): State<Console>,
    Extension(a): Extension<Authority>,
    h: HeaderMap,
    Path((name, action)): Path<(String, String)>,
) -> Response {
    let action = match action.as_str() {
        "pause" => Action::NodePause,
        "resume" => Action::NodeResume,
        "revoke" => Action::NodeRevoke,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    mutate_node(&s, &a, &h, &name, action, None)
}
fn mutate_node(
    s: &Console,
    a: &Authority,
    h: &HeaderMap,
    name: &str,
    action: Action,
    d: Option<DesiredConfig>,
) -> Response {
    let mut access = s.access.lock().unwrap();
    if access
        .validate_mutation(a, csrf(h), origin(h), now(), false)
        .is_err()
    {
        return denied();
    }
    let mut control = s.app.control.lock().unwrap();
    let Some((record, _)) = control
        .inventory()
        .into_iter()
        .find(|(r, _)| r.name == name)
    else {
        return denied();
    };
    let id = record.enrollment_id();
    if !access.authorize(a, action, Some(&id), None) {
        return denied();
    }
    let intent = match access.begin_control(a, action, Some(&id), &id, now()) {
        Ok(i) => i,
        Err(e) => return failure(e),
    };
    let result = match d {
        Some(d) => control.set_config(name, d),
        None => control.set_status(
            name,
            match action {
                Action::NodePause => Status::Paused,
                Action::NodeResume => Status::Active,
                _ => Status::Revoked,
            },
        ),
    };
    if result.as_ref().is_err_and(|e| {
        !matches!(
            e.kind(),
            io::ErrorKind::InvalidInput | io::ErrorKind::AlreadyExists | io::ErrorKind::NotFound
        )
    }) {
        access.quarantine();
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "control publication requires offline reconciliation",
        );
    }
    if access
        .finish_control(&intent, a, &id, result.is_ok(), now())
        .is_err()
    {
        access.quarantine();
        return failure(io::Error::other("control audit unavailable"));
    }
    json_result(result.map(|r|json!({"name":r.name,"enrollment_id":r.enrollment_id(),"status":r.status,"revision":r.revision})))
}
fn page_mut(q: &mut Query) -> Option<&mut Option<String>> {
    match q {
        Query::Logs { page, .. } | Query::Metrics { page, .. } | Query::Spans { page, .. } => {
            Some(page)
        }
        _ => None,
    }
}
async fn query(
    State(s): State<Console>,
    Extension(a): Extension<Authority>,
    Extension(permit): Extension<Arc<OwnedSemaphorePermit>>,
    body: Bytes,
) -> Response {
    let mut q: Query = match serde_json::from_slice(&body) {
        Ok(q) => q,
        Err(_) => return err(StatusCode::BAD_REQUEST, "invalid structured query"),
    };
    drop(body);
    let filters_fit = match &q {
        Query::Logs { contains, .. } => contains.as_ref().is_none_or(|v| v.len() <= 4096),
        Query::Metrics { name, .. } | Query::Rate { name, .. } => name.len() <= 4096,
        Query::Spans { trace_id, name, .. } => {
            trace_id.as_ref().is_none_or(|v| v.len() <= 64)
                && name.as_ref().is_none_or(|v| v.len() <= 4096)
        }
    };
    if !filters_fit {
        return err(StatusCode::BAD_REQUEST, "query filter exceeds budget");
    }
    let (node, from, to, limit, signal) = match &q {
        Query::Logs {
            node,
            from_ns,
            to_ns,
            limit,
            ..
        } => (node, *from_ns, *to_ns, *limit, Signal::Logs),
        Query::Metrics {
            node,
            from_ns,
            to_ns,
            limit,
            ..
        } => (node, *from_ns, *to_ns, *limit, Signal::Metrics),
        Query::Spans {
            node,
            from_ns,
            to_ns,
            limit,
            ..
        } => (node, *from_ns, *to_ns, *limit, Signal::Traces),
        Query::Rate { .. } => {
            return err(
                StatusCode::BAD_REQUEST,
                "unbounded rate queries are unavailable in the console",
            );
        }
    };
    if limit == 0
        || limit as usize > a.scope.max_query_rows.min(1000)
        || to <= from
        || to - from > a.scope.max_query_window_s.saturating_mul(1_000_000_000)
    {
        return err(StatusCode::BAD_REQUEST, "query exceeds authorized bounds");
    }
    let allowed: HashSet<String> = {
        let access = s.access.lock().unwrap();
        if !access.authorize(&a, Action::TelemetryRead, None, Some(signal)) {
            return denied();
        }
        s.app
            .control
            .lock()
            .unwrap()
            .inventory()
            .into_iter()
            .filter(|(r, _)| {
                a.scope.allows(
                    Action::TelemetryRead,
                    Some(&r.enrollment_id()),
                    Some(signal),
                )
            })
            .map(|(r, _)| r.name)
            .collect()
    };
    if node.as_ref().is_some_and(|n| !allowed.contains(n)) {
        return denied();
    }
    let page = page_mut(&mut q).and_then(Option::take);
    let shape = format!("{q:?}:{:?}:{:?}", a.scope, a.on_behalf_of);
    let snapshot = if let Some(id) = page {
        let mut cursors = s.cursors.lock().unwrap();
        cursors.retain(|_, c| c.expires > now());
        let Some(c) = cursors.get(&id) else {
            return err(StatusCode::GONE, "page expired; run the query again");
        };
        if c.principal != a.principal_id || c.policy != a.policy_version || c.shape != shape {
            return denied();
        }
        *page_mut(&mut q).unwrap() = Some(c.raw.clone());
        c.snapshot.clone()
    } else {
        match random() {
            Ok(id) => id,
            Err(e) => return failure(e),
        }
    };
    let history = Arc::clone(&s.app.history);
    let committed = s.app.intake.committed_group();
    let answer = tokio::task::spawn_blocking(move || {
        let result = history.run_scoped(&q, committed, &allowed);
        drop(permit);
        result
    })
    .await;
    let mut answer = match answer {
        Ok(Ok(v)) => v,
        Ok(Err(QueryError::Gone)) => {
            return err(StatusCode::GONE, "page snapshot no longer retained");
        }
        Ok(Err(QueryError::Invalid(_))) => return err(StatusCode::BAD_REQUEST, "invalid query"),
        _ => {
            return err(
                StatusCode::SERVICE_UNAVAILABLE,
                "query coverage unavailable",
            );
        }
    };
    let mut access = s.access.lock().unwrap();
    if access.recheck_at(&a, now()).is_err() {
        return denied();
    }
    answer["snapshot"] = json!(snapshot);
    if let Some(raw) = answer["next_page"].as_str() {
        let id = match random() {
            Ok(id) => id,
            Err(e) => return failure(e),
        };
        let mut cursors = s.cursors.lock().unwrap();
        cursors.retain(|_, c| c.expires > now());
        if cursors.len() >= CURSOR_CAP {
            return err(
                StatusCode::SERVICE_UNAVAILABLE,
                "page capacity reached; retry later",
            );
        }
        cursors.insert(
            id.clone(),
            Cursor {
                principal: a.principal_id.clone(),
                policy: a.policy_version,
                shape,
                raw: raw.to_owned(),
                snapshot,
                expires: now().saturating_add(900),
            },
        );
        answer["next_page"] = json!(id);
    }
    match serde_json::to_vec(&answer) {
        Ok(bytes) if bytes.len() <= RESPONSE_CAP => {
            let mut r = bytes.into_response();
            r.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            r
        }
        _ => err(
            StatusCode::PAYLOAD_TOO_LARGE,
            "query response exceeds budget; reduce row limit or window",
        ),
    }
}
async fn principals(State(s): State<Console>, Extension(a): Extension<Authority>) -> Response {
    json_result(s.access.lock().unwrap().principals(&a))
}
async fn audit(State(s): State<Console>, Extension(a): Extension<Authority>) -> Response {
    let access = s.access.lock().unwrap();
    json_result(
        access
            .audit(&a)
            .map(|v| json!({"events":v,"suppressed_or_expired_events":access.audit_dropped()})),
    )
}
async fn passkeys(State(s): State<Console>, Extension(a): Extension<Authority>) -> Response {
    json_result(s.access.lock().unwrap().passkey_ids(&a))
}
async fn sessions(State(s): State<Console>, Extension(a): Extension<Authority>) -> Response {
    json_result(s.access.lock().unwrap().sessions(&a))
}
async fn add_passkey(
    State(s): State<Console>,
    Extension(a): Extension<Authority>,
    h: HeaderMap,
) -> Response {
    json_result(
        s.access
            .lock()
            .unwrap()
            .start_add_passkey(&a, csrf(&h), origin(&h), now()),
    )
}
async fn revoke_passkey(
    State(s): State<Console>,
    Extension(a): Extension<Authority>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    json_result(
        s.access
            .lock()
            .unwrap()
            .revoke_passkey(&a, &id, csrf(&h), origin(&h), now()),
    )
}
async fn revoke_session(
    State(s): State<Console>,
    Extension(a): Extension<Authority>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    json_result(
        s.access
            .lock()
            .unwrap()
            .revoke_session(&a, &id, csrf(&h), origin(&h), now()),
    )
}
async fn revoke_credential(
    State(s): State<Console>,
    Extension(a): Extension<Authority>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    json_result(
        s.access
            .lock()
            .unwrap()
            .revoke_credential(&a, &id, csrf(&h), origin(&h), now()),
    )
}
async fn disable(
    State(s): State<Console>,
    Extension(a): Extension<Authority>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    json_result(
        s.access
            .lock()
            .unwrap()
            .disable(&a, &id, csrf(&h), origin(&h), now()),
    )
}
async fn set_scope(
    State(s): State<Console>,
    Extension(a): Extension<Authority>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(scope): Json<Scope>,
) -> Response {
    json_result(
        s.access
            .lock()
            .unwrap()
            .set_scope(&a, &id, scope, csrf(&h), origin(&h), now()),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Workload {
    name: String,
    scope: Scope,
    #[serde(default = "credential_ttl")]
    ttl_s: u64,
}
fn credential_ttl() -> u64 {
    86400
}
async fn workload(
    State(s): State<Console>,
    Extension(a): Extension<Authority>,
    h: HeaderMap,
    Json(r): Json<Workload>,
) -> Response {
    json_result(s.access.lock().unwrap().issue_workload(
        &a,
        &r.name,
        r.scope,
        r.ttl_s,
        csrf(&h),
        origin(&h),
        now(),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rotation {
    #[serde(default = "credential_ttl")]
    ttl_s: u64,
}
async fn rotate_workload(
    State(s): State<Console>,
    Extension(a): Extension<Authority>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(r): Json<Rotation>,
) -> Response {
    json_result(s.access.lock().unwrap().rotate_workload(
        &a,
        &id,
        r.ttl_s,
        csrf(&h),
        origin(&h),
        now(),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Invite {
    name: String,
    scope: Scope,
}
async fn invite(
    State(s): State<Console>,
    Extension(a): Extension<Authority>,
    h: HeaderMap,
    Json(r): Json<Invite>,
) -> Response {
    json_result(s.access.lock().unwrap().invite_human(
        &a,
        &r.name,
        r.scope,
        csrf(&h),
        origin(&h),
        now(),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delegation {
    workload_id: String,
    scope: Scope,
    ttl_s: u64,
}
async fn delegate(
    State(s): State<Console>,
    Extension(a): Extension<Authority>,
    h: HeaderMap,
    Json(r): Json<Delegation>,
) -> Response {
    json_result(s.access.lock().unwrap().delegate(
        &a,
        &r.workload_id,
        r.scope,
        r.ttl_s,
        csrf(&h),
        origin(&h),
        now(),
    ))
}

#[cfg(test)]
#[path = "console_tests.rs"]
mod tests;
