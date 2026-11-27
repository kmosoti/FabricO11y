//! Bounded authenticated transport for authoritative custody and typed investigation.
pub use crate::canonical_auth::{
    AUTH_HANDSHAKE_VERSION, ClientHello, ClientProof, MAX_HANDSHAKE_FRAME, MAX_SERVER_PROOFS,
    ServerChallenge, accept_authenticated_sync, build_client_proof, build_server_challenge,
    derive_server_proof, is_valid_nonce_hex, random_nonce_hex, verify_client_proof,
    verify_request_mac, verify_server_challenge,
};
use crate::{observation, query};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

pub const MAX_FRAME: usize = 1024 * 1024;
// Forwarding count and decoded-byte caps apply together; reclamation has its own bound.
pub const MAX_BATCH: usize = 512;
pub const MAX_BATCH_BYTES: usize = 256 * 1024;
pub const MAX_PROJECTION_READY_TIMEOUT_MS: u32 = 30_000;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardRecord {
    pub source_id: u64,
    pub raw: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CanonicalRow {
    #[serde(with = "query::decimal")]
    pub canonical_id: u64,
    pub origin: String,
    #[serde(with = "query::decimal")]
    pub source_id: u64,
    pub raw: String,
}
/// Scope and budget for an exact receipt visibility check at a canonical authority.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionReadyRequest {
    pub authority: String,
    pub origin: String,
    #[serde(with = "query::decimal")]
    pub source_id: u64,
    pub identity: observation::Identity,
    pub timeout_ms: u32,
    pub wait: bool,
}
fn valid_receipt_fields(
    authority: &str,
    origin: &str,
    source_id: u64,
    identity: &observation::Identity,
) -> bool {
    let valid = |value: &str| !value.is_empty() && value.len() <= 128 && !value.contains('\0');
    valid(authority)
        && valid(origin)
        && valid(&identity.producer)
        && valid(&identity.generation)
        && identity.sequence != 0
        && source_id != 0
        && source_id <= i64::MAX as u64
}
impl ProjectionReadyRequest {
    pub fn validate(&self) -> Result<(), String> {
        if !valid_receipt_fields(
            &self.authority,
            &self.origin,
            self.source_id,
            &self.identity,
        ) || !(1..=MAX_PROJECTION_READY_TIMEOUT_MS).contains(&self.timeout_ms)
        {
            return Err("invalid canonical projection readiness request".into());
        }
        Ok(())
    }
}

/// Exact canonical lookup bound to the expected authority and custody receipt.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactReceiptRequest {
    pub authority: String,
    pub origin: String,
    #[serde(with = "query::decimal")]
    pub source_id: u64,
    pub identity: observation::Identity,
}
impl ExactReceiptRequest {
    pub fn validate(&self) -> Result<(), String> {
        if !valid_receipt_fields(
            &self.authority,
            &self.origin,
            self.source_id,
            &self.identity,
        ) {
            return Err("invalid canonical projection readiness request".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionStatus {
    Visible,
    Pending,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionReason {
    CommittedRow,
    AwaitingProjection,
    AuthorityMismatch,
    UnknownOrigin,
    RecordUnavailable,
    IdentityMismatch,
    CustodyEvidenceUnavailable,
    ProjectionEvidenceUnavailable,
    ProjectedRowUnavailable,
    BackendUnsupported,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectionReadiness {
    pub status: ProjectionStatus,
    pub authority: Option<String>,
    pub reason: ProjectionReason,
    #[serde(with = "query::optional_decimal")]
    pub custody_frontier: Option<u64>,
    #[serde(with = "query::optional_decimal")]
    pub projection_frontier: Option<u64>,
}

/// Dedicated proposal carrier in a v3 JSON log, encoded as a JSON string.
pub const PROPOSAL_MARKER: &str = "fabrico11y.colloquium.proposal.v1";
pub const MAX_ACCEPT_BODY: usize = 128 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct PlanScope(pub String);
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanProposal {
    pub version: u8,
    pub scope: PlanScope,
    pub proposal_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptPlan {
    pub version: u8,
    pub authority: String,
    pub command_id: String,
    pub scope: PlanScope,
    pub expected_head: Option<i64>,
    pub origin: String,
    pub source_id: u64,
    pub proposal_raw: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlanDecision {
    pub version: u8,
    pub authority: String,
    pub scope: PlanScope,
    pub command_id: String,
    pub writer: String,
    pub previous_head: Option<i64>,
    pub head: i64,
    pub proposal_id: String,
}

/// A short-lived, receiver-scoped grant to request bytes, not an availability promise.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentTicket {
    pub version: u32,
    pub authority: String,
    pub origin: String,
    pub digest: String,
    pub start: u64,
    pub end: u64,
    /// Opaque seconds on this receiver instance’s monotonic clock, not Unix time.
    pub expires: u64,
    pub mac: String,
}

/// One origin-scoped exact receipt in a fixed-barrier batch.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptBatchItem {
    #[serde(with = "query::decimal")]
    pub source_id: u64,
    pub identity: observation::Identity,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptBatchRequest {
    pub authority: String,
    pub origin: String,
    #[serde(with = "query::decimal")]
    pub source_barrier: u64,
    pub timeout_ms: u32,
    pub include_digest: bool,
    pub items: Vec<ReceiptBatchItem>,
}
impl ReceiptBatchRequest {
    pub fn validate(&self) -> Result<(), String> {
        let valid = |s: &str| !s.is_empty() && s.len() <= 128 && !s.contains('\0');
        if !valid(&self.authority)
            || !valid(&self.origin)
            || self.source_barrier == 0
            || self.source_barrier > i64::MAX as u64
            || self.items.is_empty()
            || self.items.len() > 256
            || !(1..=MAX_PROJECTION_READY_TIMEOUT_MS).contains(&self.timeout_ms)
        {
            return Err("invalid receipt batch".into());
        }
        for item in &self.items {
            if item.source_id == 0
                || item.source_id > i64::MAX as u64
                || item.identity.sequence == 0
                || !valid(&item.identity.producer)
                || !valid(&item.identity.generation)
            {
                return Err("invalid receipt batch item".into());
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptBatchStatus {
    Ready,
    Pending,
    Missing,
    Mismatch,
    Unavailable,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptBatchOutcome {
    pub item: ReceiptBatchItem,
    pub status: ReceiptBatchStatus,
    pub reason: ProjectionReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_sha256: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptBatchReadiness {
    pub authority: String,
    pub origin: String,
    #[serde(with = "query::decimal")]
    pub source_barrier: u64,
    #[serde(with = "query::optional_decimal")]
    pub custody_frontier: Option<u64>,
    #[serde(with = "query::optional_decimal")]
    pub projection_frontier: Option<u64>,
    pub outcomes: Vec<ReceiptBatchOutcome>,
    pub segment_attempts: u64,
    pub segment_bytes_reserved: u64,
    pub segment_rows_reserved: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanCursorFailure {
    Invalid,
    Expired,
    Revoked,
    Gap,
    Capacity,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanPageRequest {
    pub scope: PlanScope,
    pub cursor: Option<String>,
    pub limit: u8,
    pub ttl_secs: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanPage {
    pub authority: String,
    pub scope: PlanScope,
    pub frontier: i64,
    pub decisions: Vec<PlanDecision>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Ingest {
        origin: String,
        records: Vec<ForwardRecord>,
    },
    Read {
        origin: String,
        after: u64,
        through: u64,
        limit: usize,
    },
    Lookup {
        origin: String,
        identity: observation::Identity,
    },
    Progress,
    PlanPage {
        request: PlanPageRequest,
    },
    RevokePlanCursors {
        scope: PlanScope,
    },
    SubscribePlan {
        scope: PlanScope,
    },
    PollPlanHint {
        scope: PlanScope,
        subscription: String,
    },
    AcceptPlan {
        body: String,
    },
    ExactReceiptLookup {
        request: ExactReceiptRequest,
    },
    ProjectionReady {
        request: ProjectionReadyRequest,
    },
    IssueSegmentTicket {
        version: u32,
        authority: String,
        origin: String,
        digest: String,
        start: u64,
        end: u64,
    },
    ReadSegmentChunk {
        ticket: SegmentTicket,
        offset: u64,
        length: u32,
    },
    ReceiptBatchReady {
        request: ReceiptBatchRequest,
    },
    Capabilities,
    Query {
        request: query::QueryRequest,
    },
    QueryWithOperationVersion {
        request: query::QueryRequest,
        operation_version: u32,
    },
    Plan {
        request: query::QueryRequest,
    },
    Evidence {
        #[serde(with = "query::decimal")]
        canonical_id: u64,
    },
    Cancel {
        query_id: String,
    },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Committed {
        authority: String,
        origin: String,
        through: u64,
    },
    Rows {
        rows: Vec<CanonicalRow>,
    },
    Found {
        row: Option<CanonicalRow>,
    },
    PlanPage {
        page: PlanPage,
    },
    PlanCursorFailure {
        reason: PlanCursorFailure,
    },
    PlanCursorsRevoked,
    PlanSubscribed {
        subscription: String,
        frontier: i64,
    },
    PlanHint {
        pending: bool,
    },
    PlanAccepted {
        decision: String,
    },
    ExactReceiptFound {
        authority: String,
        row: Option<CanonicalRow>,
    },
    Progress {
        authority: String,
        #[serde(with = "query::snapshot_decimal")]
        frontiers: BTreeMap<String, u64>,
    },
    ProjectionReady {
        readiness: ProjectionReadiness,
    },
    SegmentTicket {
        ticket: SegmentTicket,
    },
    SegmentChunk {
        offset: u64,
        bytes: Vec<u8>,
    },
    ReceiptBatchReady {
        readiness: ReceiptBatchReadiness,
    },
    Capabilities {
        authority: String,
        schemas: Vec<u32>,
        origins: Vec<String>,
        max_frame: usize,
        max_batch_records: usize,
    },
    Query {
        response: query::QueryResponse,
    },
    Plan {
        plan: query::QueryPlan,
    },
    Cancelled {
        query_id: String,
    },
    Error {
        error: String,
    },
}
pub fn remaining(deadline: Instant) -> Result<Duration, String> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| "deadline; outcome unknown".into())
}
pub fn write_frame(stream: &mut TcpStream, bytes: &[u8], deadline: Instant) -> Result<(), String> {
    if bytes.len() > MAX_FRAME {
        return Err("canonical frame limit".into());
    }
    let header = (bytes.len() as u32).to_be_bytes();
    for mut part in [header.as_slice(), bytes] {
        while !part.is_empty() {
            stream
                .set_write_timeout(Some(remaining(deadline)?))
                .map_err(|e| e.to_string())?;
            let n = stream
                .write(part)
                .map_err(|e| format!("write; outcome unknown: {e}"))?;
            if n == 0 {
                return Err("closed; outcome unknown".into());
            }
            part = &part[n..];
        }
    }
    Ok(())
}
fn read_exact(
    stream: &mut TcpStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> Result<(), String> {
    while !bytes.is_empty() {
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(|e| e.to_string())?;
        let n = stream
            .read(bytes)
            .map_err(|e| format!("read; outcome unknown: {e}"))?;
        if n == 0 {
            return Err("closed; outcome unknown".into());
        }
        bytes = &mut bytes[n..];
    }
    Ok(())
}
pub fn read_frame(stream: &mut TcpStream, deadline: Instant) -> Result<Vec<u8>, String> {
    let mut header = [0; 4];
    read_exact(stream, &mut header, deadline)?;
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME {
        return Err("canonical frame limit".into());
    }
    let mut bytes = vec![0; len];
    read_exact(stream, &mut bytes, deadline)?;
    observation::check_json_depth(&bytes, MAX_FRAME, observation::MAX_DEPTH + 6)?;
    Ok(bytes)
}
pub fn exchange(
    endpoint: &str,
    credential: &str,
    request: &Request,
    deadline: Instant,
) -> Result<Response, String> {
    let address: SocketAddr = endpoint
        .parse()
        .map_err(|_| "expected loopback address:port")?;
    if !address.ip().is_loopback() {
        return Err("canonical transport requires authenticated loopback tunnel".into());
    }
    if credential.is_empty() || credential.len() > 256 {
        return Err("invalid credential length".into());
    }
    if let Request::Ingest { records, .. } = request
        && (records.len() > MAX_BATCH
            || records
                .iter()
                .try_fold(0usize, |n, r| n.checked_add(r.raw.len()))
                .is_none_or(|n| n > MAX_BATCH_BYTES))
    {
        return Err("canonical batch limit".into());
    }
    let client_nonce = random_nonce_hex()?;
    let mut stream = TcpStream::connect_timeout(&address, remaining(deadline)?)
        .map_err(|e| format!("connect: {e}"))?;
    let _ = stream.set_nodelay(true);

    let hello = ClientHello {
        version: AUTH_HANDSHAKE_VERSION,
        client_nonce: client_nonce.clone(),
    };
    let hello_bytes = serde_json::to_vec(&hello).map_err(|e| e.to_string())?;
    write_frame(&mut stream, &hello_bytes, deadline)?;

    let challenge_bytes = read_frame(&mut stream, deadline)?;
    if challenge_bytes.len() > MAX_HANDSHAKE_FRAME {
        return Err("canonical handshake frame limit".into());
    }
    let challenge: ServerChallenge =
        serde_json::from_slice(&challenge_bytes).map_err(|e| e.to_string())?;
    verify_server_challenge(credential, &client_nonce, &challenge)?;

    let request_bytes = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    if request_bytes.len() > MAX_FRAME {
        return Err("canonical frame limit".into());
    }
    let proof = build_client_proof(
        credential,
        &client_nonce,
        &challenge.server_nonce,
        &request_bytes,
    );
    let proof_bytes = serde_json::to_vec(&proof).map_err(|e| e.to_string())?;
    write_frame(&mut stream, &proof_bytes, deadline)?;
    write_frame(&mut stream, &request_bytes, deadline)?;
    serde_json::from_slice(&read_frame(&mut stream, deadline)?).map_err(|e| e.to_string())
}

#[cfg(test)]
mod auth_tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[test]
    fn rogue_listener_never_receives_raw_credential_or_request() {
        let secret = "super-secret-canonical-credential-12345";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let saw_extra_frame = Arc::new(AtomicBool::new(false));
        let saw_raw_secret = Arc::new(AtomicBool::new(false));
        let extra_flag = saw_extra_frame.clone();
        let secret_flag = saw_raw_secret.clone();

        let rogue = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let (mut stream, _) = listener.accept().unwrap();
            let hello_bytes = read_frame(&mut stream, deadline).unwrap();
            if String::from_utf8_lossy(&hello_bytes).contains(secret) {
                secret_flag.store(true, Ordering::SeqCst);
            }
            let hello: ClientHello = serde_json::from_slice(&hello_bytes).unwrap();
            assert!(is_valid_nonce_hex(&hello.client_nonce));

            // Rogue sends a challenge generated without knowledge of `secret`.
            let (_, bogus) =
                build_server_challenge(&hello.client_nonce, ["wrong-rogue-secret"]).unwrap();
            write_frame(&mut stream, &serde_json::to_vec(&bogus).unwrap(), deadline).unwrap();

            // Client must verify server challenge and abort without sending proof or request.
            let mut buf = [0u8; 64];
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            match stream.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    if String::from_utf8_lossy(&buf[..n]).contains(secret) {
                        secret_flag.store(true, Ordering::SeqCst);
                    }
                    extra_flag.store(true, Ordering::SeqCst);
                }
                Err(_) => {}
            }
        });

        let err = exchange(
            &addr,
            secret,
            &Request::Ingest {
                origin: "secret-origin".into(),
                records: vec![ForwardRecord {
                    source_id: 1,
                    raw: "{\"secret\":\"telemetry\"}".into(),
                }],
            },
            Instant::now() + Duration::from_secs(3),
        )
        .unwrap_err();
        rogue.join().unwrap();
        assert!(err.contains("mutual authentication"), "{err}");
        assert!(
            !saw_raw_secret.load(Ordering::SeqCst),
            "raw credential crossed wire"
        );
        assert!(
            !saw_extra_frame.load(Ordering::SeqCst),
            "client sent proof/request to rogue listener"
        );
    }

    #[test]
    fn mutual_hmac_challenge_response_never_transmits_raw_credential() {
        let secret = "shared-loopback-secret-abcdef123456";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let peer = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let (mut stream, _) = listener.accept().unwrap();
            let hello_bytes = read_frame(&mut stream, deadline).unwrap();
            assert!(!String::from_utf8_lossy(&hello_bytes).contains(secret));
            let hello: ClientHello = serde_json::from_slice(&hello_bytes).unwrap();
            let (server_nonce, challenge) =
                build_server_challenge(&hello.client_nonce, ["other-secret", secret]).unwrap();
            let challenge_bytes = serde_json::to_vec(&challenge).unwrap();
            assert!(!String::from_utf8_lossy(&challenge_bytes).contains(secret));
            write_frame(&mut stream, &challenge_bytes, deadline).unwrap();

            let proof_bytes = read_frame(&mut stream, deadline).unwrap();
            assert!(!String::from_utf8_lossy(&proof_bytes).contains(secret));
            let proof: ClientProof = serde_json::from_slice(&proof_bytes).unwrap();
            assert!(verify_client_proof(
                secret,
                &hello.client_nonce,
                &server_nonce,
                &proof.proof
            ));

            let req_bytes = read_frame(&mut stream, deadline).unwrap();
            assert!(!String::from_utf8_lossy(&req_bytes).contains(secret));
            assert!(verify_request_mac(
                secret,
                &hello.client_nonce,
                &server_nonce,
                &req_bytes,
                &proof.request_mac
            ));
            let mut tampered = req_bytes.clone();
            tampered[0] ^= 1;
            assert!(!verify_request_mac(
                secret,
                &hello.client_nonce,
                &server_nonce,
                &tampered,
                &proof.request_mac
            ));

            let req: Request = serde_json::from_slice(&req_bytes).unwrap();
            assert!(matches!(req, Request::Progress));
            let resp = Response::Progress {
                authority: "auth-1".into(),
                frontiers: BTreeMap::new(),
            };
            write_frame(&mut stream, &serde_json::to_vec(&resp).unwrap(), deadline).unwrap();
        });

        let resp = exchange(
            &addr,
            secret,
            &Request::Progress,
            Instant::now() + Duration::from_secs(3),
        )
        .unwrap();
        peer.join().unwrap();
        assert!(matches!(resp, Response::Progress { authority, .. } if authority == "auth-1"));
    }
}
