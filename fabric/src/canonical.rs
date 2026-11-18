//! Bounded authenticated transport for authoritative custody and typed investigation.
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
impl ProjectionReadyRequest {
    pub fn validate(&self) -> Result<(), String> {
        let valid = |value: &str| !value.is_empty() && value.len() <= 128 && !value.contains('\0');
        if !valid(&self.authority)
            || !valid(&self.origin)
            || !valid(&self.identity.producer)
            || !valid(&self.identity.generation)
            || self.identity.sequence == 0
            || self.source_id == 0
            || self.source_id > i64::MAX as u64
            || !(1..=MAX_PROJECTION_READY_TIMEOUT_MS).contains(&self.timeout_ms)
        {
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authenticated {
    pub credential: String,
    pub request: Request,
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
    ProjectionReady {
        request: ProjectionReadyRequest,
    },
    Capabilities,
    Query {
        request: query::QueryRequest,
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
    Progress {
        authority: String,
        #[serde(with = "query::snapshot_decimal")]
        frontiers: BTreeMap<String, u64>,
    },
    ProjectionReady {
        readiness: ProjectionReadiness,
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
    let mut stream = TcpStream::connect_timeout(&address, remaining(deadline)?)
        .map_err(|e| format!("connect: {e}"))?;
    let body = serde_json::to_vec(&serde_json::json!({"credential":credential,"request":request}))
        .map_err(|e| e.to_string())?;
    write_frame(&mut stream, &body, deadline)?;
    serde_json::from_slice(&read_frame(&mut stream, deadline)?).map_err(|e| e.to_string())
}
