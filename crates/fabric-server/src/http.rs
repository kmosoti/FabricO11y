//! HTTPS surface. Handlers authenticate, validate and hand bytes to the
//! commit thread; they never touch storage.

use crate::store::{Answer, Intake, MAX_BATCH_BYTES, Submission, identify};
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Clone)]
pub struct AppState {
    pub intake: Intake,
    pub credentials: Arc<HashMap<[u8; 32], String>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/batches", post(batches))
        .layer(DefaultBodyLimit::max(MAX_BATCH_BYTES + 1))
        .with_state(state)
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

fn label_for(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    state.credentials.get(&digest).cloned()
}

async fn batches(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(label) = label_for(&state, &headers) else {
        return reply(StatusCode::UNAUTHORIZED, "unauthorized", None);
    };
    if body.len() > MAX_BATCH_BYTES {
        return reply(StatusCode::PAYLOAD_TOO_LARGE, "too_large", None);
    }
    let Ok((stream, sequence)) = identify(&body) else {
        return reply(StatusCode::BAD_REQUEST, "bad_request", None);
    };
    let (tx, rx) = oneshot::channel();
    state.intake.submit(Submission {
        label,
        stream,
        sequence,
        bytes: body.to_vec(),
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
