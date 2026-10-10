//! HTTPS surface. Handlers authenticate, validate and hand bytes to the
//! commit thread or to the control state; they never touch the journal.
//!
//! Node routes use a node bearer token: `POST /v1/batches`, `GET /v1/config`.
//! Admin routes use the admin token and live under `/v1/admin/`.

use crate::control::{Control, DesiredConfig, Status};
use crate::query::{History, Query, QueryError};
use crate::store::{Answer, Intake, MAX_BATCH_BYTES, Submission, identify_strand};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Extension, Router};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};

type WorkPermit = Arc<OwnedSemaphorePermit>;

#[derive(Clone)]
struct Admission {
    pool: Arc<Semaphore>,
    query: bool,
}

async fn admit_body(
    State(admission): State<Admission>,
    mut request: Request,
    next: Next,
) -> Response {
    let Ok(permit) = admission.pool.try_acquire_owned() else {
        let mut response = if admission.query {
            error(StatusCode::SERVICE_UNAVAILABLE, "query admission is full")
        } else {
            reply(StatusCode::SERVICE_UNAVAILABLE, "unavailable", None)
        };
        response.headers_mut().insert(
            header::RETRY_AFTER,
            axum::http::HeaderValue::from_static("1"),
        );
        return response;
    };
    let permit = Arc::new(permit);
    request.extensions_mut().insert(Arc::clone(&permit));
    let response = next.run(request).await;
    // Keep admission through extraction and the response even when a handler
    // does not consume its extension. Cancellation also drops this owner.
    drop(permit);
    response
}

// A canceled async waiter does not cancel started blocking work. Move its
// admission owner into the same closure; unwind and completion both release it.
fn spawn_query_work<T: Send + 'static>(
    permit: WorkPermit,
    work: impl FnOnce() -> T + Send + 'static,
) -> tokio::task::JoinHandle<T> {
    tokio::task::spawn_blocking(move || {
        let result = work();
        drop(permit);
        result
    })
}

#[derive(Clone)]
pub struct AppState {
    pub intake: Intake,
    pub control: Arc<Mutex<Control>>,
    pub admin_token_sha256: [u8; 32],
    pub history: Arc<History>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route(
            "/v1/batches",
            post(batches)
                .layer(middleware::from_fn_with_state(
                    Admission {
                        pool: Arc::new(Semaphore::new(16)),
                        query: false,
                    },
                    admit_body,
                ))
                .layer(middleware::from_fn_with_state(
                    state.clone(),
                    authenticate_batch_header,
                )),
        )
        .route("/v1/config", get(node_config))
        .route("/v1/admin/nodes", get(list_nodes).post(enroll))
        .route("/v1/admin/nodes/{name}/config", put(set_config))
        .route("/v1/admin/nodes/{name}/{action}", post(set_status))
        .route(
            "/v1/admin/query",
            post(run_query).layer(middleware::from_fn_with_state(
                Admission {
                    pool: Arc::new(Semaphore::new(2)),
                    query: true,
                },
                admit_body,
            )),
        )
        .layer(DefaultBodyLimit::max(MAX_BATCH_BYTES + 1))
        .with_state(state)
}

/// Reject unknown credentials before allocating/decoding a Batch body. The
/// handler rechecks after body receipt to linearize admission with revocation.
async fn authenticate_batch_header(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if node_for(&state, request.headers()).is_none() {
        return reply(StatusCode::UNAUTHORIZED, "unauthorized", None);
    }
    tokio::time::timeout(std::time::Duration::from_secs(15), next.run(request))
        .await
        .unwrap_or_else(|_| reply(StatusCode::REQUEST_TIMEOUT, "timeout", None))
}

async fn health() -> Response {
    (StatusCode::OK, axum::Json(json!({"status": "ok"}))).into_response()
}

fn reply(status: StatusCode, kind: &str, committed_through: Option<u64>) -> Response {
    let body = match committed_through {
        Some(through) => json!({"status": kind, "committed_through": through}),
        None => json!({"status": kind}),
    };
    (status, axum::Json(body)).into_response()
}

fn error(status: StatusCode, message: impl ToString) -> Response {
    (status, axum::Json(json!({"error": message.to_string()}))).into_response()
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn node_for(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let token = bearer(headers)?;
    state.control.lock().unwrap().authenticate(token)
}

fn is_admin(state: &AppState, headers: &HeaderMap) -> bool {
    bearer(headers).is_some_and(|token| {
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        digest == state.admin_token_sha256
    })
}

async fn batches(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(label) = node_for(&state, &headers) else {
        return reply(StatusCode::UNAUTHORIZED, "unauthorized", None);
    };
    if body.len() > MAX_BATCH_BYTES {
        return reply(StatusCode::PAYLOAD_TOO_LARGE, "too_large", None);
    }
    let Ok((strand, sequence)) = identify_strand(&body) else {
        return reply(StatusCode::BAD_REQUEST, "bad_request", None);
    };
    let (tx, rx) = oneshot::channel();
    let bytes = body.to_vec();
    drop(body);
    state.intake.submit(Submission {
        label,
        strand,
        sequence,
        bytes,
        reply: tx,
    });
    match rx.await.unwrap_or(Answer::Unavailable) {
        Answer::Ack(through) => reply(StatusCode::OK, "ack", Some(through)),
        Answer::Conflict(through) => reply(StatusCode::CONFLICT, "conflict", Some(through)),
        Answer::Gap(through) => reply(StatusCode::CONFLICT, "gap", Some(through)),
        Answer::Forbidden => reply(StatusCode::FORBIDDEN, "forbidden", None),
        Answer::Unavailable => reply(StatusCode::SERVICE_UNAVAILABLE, "unavailable", None),
    }
}

/// A node poll. `x-fabric-applied-revision` and `x-fabric-config-error`
/// report what the node runs; `if-none-match` avoids resending an unchanged view.
async fn node_config(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(name) = node_for(&state, &headers) else {
        return reply(StatusCode::UNAUTHORIZED, "unauthorized", None);
    };
    let text = |key: &str| headers.get(key).and_then(|v| v.to_str().ok());
    let applied = text("x-fabric-applied-revision")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let config_error = text("x-fabric-config-error").map(|e| e.chars().take(240).collect());
    let Some(view) = state
        .control
        .lock()
        .unwrap()
        .poll(&name, applied, config_error)
    else {
        return reply(StatusCode::UNAUTHORIZED, "unauthorized", None);
    };
    if text("if-none-match") == Some(format!("\"{}\"", view.revision).as_str()) {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    (StatusCode::OK, axum::Json(view)).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollRequest {
    name: String,
    #[serde(default)]
    logs: Vec<String>,
    #[serde(default = "default_interval")]
    metric_interval_s: u64,
}

fn default_interval() -> u64 {
    15
}

async fn enroll(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if !is_admin(&state, &headers) {
        return error(StatusCode::UNAUTHORIZED, "admin token required");
    }
    let request: EnrollRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let desired = DesiredConfig {
        logs: request.logs,
        metric_interval_s: request.metric_interval_s,
    };
    match state.control.lock().unwrap().enroll(&request.name, desired) {
        Ok((record, token)) => (
            StatusCode::CREATED,
            axum::Json(json!({"name": record.name, "revision": record.revision, "token": token})),
        )
            .into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => error(StatusCode::CONFLICT, e),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => error(StatusCode::BAD_REQUEST, e),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}

async fn list_nodes(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !is_admin(&state, &headers) {
        return error(StatusCode::UNAUTHORIZED, "admin token required");
    }
    let nodes: Vec<_> = state
        .control
        .lock()
        .unwrap()
        .inventory()
        .into_iter()
        .map(|(record, observed)| {
            json!({
                "name": record.name,
                "status": record.status,
                "desired_revision": record.revision,
                "applied_revision": observed.applied_revision,
                "config_error": observed.config_error,
                "last_poll_unix_s": observed.last_poll_unix_s,
                "logs": record.desired.logs,
                "metric_interval_s": record.desired.metric_interval_s,
            })
        })
        .collect();
    (StatusCode::OK, axum::Json(json!({"nodes": nodes}))).into_response()
}

async fn set_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    body: Bytes,
) -> Response {
    if !is_admin(&state, &headers) {
        return error(StatusCode::UNAUTHORIZED, "admin token required");
    }
    let desired: DesiredConfig = match serde_json::from_slice(&body) {
        Ok(desired) => desired,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    changed(state.control.lock().unwrap().set_config(&name, desired))
}

async fn set_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((name, action)): Path<(String, String)>,
) -> Response {
    if !is_admin(&state, &headers) {
        return error(StatusCode::UNAUTHORIZED, "admin token required");
    }
    let status = match action.as_str() {
        "pause" => Status::Paused,
        "resume" => Status::Active,
        "revoke" => Status::Revoked,
        _ => return error(StatusCode::NOT_FOUND, "unknown action"),
    };
    changed(state.control.lock().unwrap().set_status(&name, status))
}

/// Read-only query over retained history. Runs on a blocking thread so a
/// long scan never stalls request handling.
async fn run_query(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(permit): Extension<WorkPermit>,
    body: Bytes,
) -> Response {
    if !is_admin(&state, &headers) {
        return error(StatusCode::UNAUTHORIZED, "admin token required");
    }
    let query: Query = match serde_json::from_slice(&body) {
        Ok(query) => query,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let committed = state.intake.committed_group();
    let history = Arc::clone(&state.history);
    drop(body);
    match spawn_query_work(permit, move || history.run(&query, committed)).await {
        Ok(Ok(answer)) => (StatusCode::OK, axum::Json(answer)).into_response(),
        Ok(Err(QueryError::Invalid(why))) => error(StatusCode::BAD_REQUEST, why),
        Ok(Err(QueryError::Gone)) => error(StatusCode::GONE, "page snapshot no longer retained"),
        Ok(Err(QueryError::Io(e))) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[cfg(test)]
#[path = "http/admission_tests.rs"]
mod admission_tests;

fn changed(result: std::io::Result<crate::control::NodeRecord>) -> Response {
    match result {
        Ok(record) => (
            StatusCode::OK,
            axum::Json(
                json!({"name": record.name, "status": record.status, "revision": record.revision}),
            ),
        )
            .into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => error(StatusCode::NOT_FOUND, e),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => error(StatusCode::BAD_REQUEST, e),
        Err(e) => error(StatusCode::SERVICE_UNAVAILABLE, e),
    }
}
