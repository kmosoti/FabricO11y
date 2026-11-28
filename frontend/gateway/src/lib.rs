#![forbid(unsafe_code)]
//! Internal typed query boundary. Browser holder authentication and the private
//! Unix HTTP adapter are separate responsibilities; this library provides neither.
use fabric::canonical::{Request, Response};
use fabric_client::FabricQueryClient;
use fabric_query_contract::{
    API_VERSION, Operation, QueryRequest, QueryResponse, TRANSFORM_VERSION,
};
use std::{collections::BTreeSet, time::Duration};

pub const MAX_INPUT_BYTES: usize = 262_144;
pub const MAX_OUTPUT_BYTES: usize = 262_144;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidConfiguration,
    InvalidRequest,
    Forbidden,
    BackendUnavailable,
    InvalidResponse,
}

/// Provisioned independently from the browser request; it is not authentication.
pub struct QueryPolicy {
    authority: String,
    sources: BTreeSet<String>,
}
impl QueryPolicy {
    pub fn new(authority: String, sources: Vec<String>) -> Result<Self, Error> {
        let valid = |s: &str| !s.is_empty() && s.len() <= 256 && !s.contains('\0');
        if !valid(&authority)
            || sources.is_empty()
            || sources.len() > 32
            || sources.iter().any(|s| !valid(s))
        {
            return Err(Error::InvalidConfiguration);
        }
        let len = sources.len();
        let sources: BTreeSet<_> = sources.into_iter().collect();
        if sources.len() != len {
            return Err(Error::InvalidConfiguration);
        }
        Ok(Self { authority, sources })
    }

    /// Strictly decode a bounded request before consulting the canonical backend.
    pub fn authorize(&self, bytes: &[u8]) -> Result<QueryRequest, Error> {
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(Error::InvalidRequest);
        }
        let mut q: QueryRequest =
            serde_json::from_slice(bytes).map_err(|_| Error::InvalidRequest)?;
        q.validate().map_err(|_| Error::InvalidRequest)?;
        if q.authority.as_deref() != Some(&self.authority)
            || !matches!(q.operation, Operation::ListRuns | Operation::RunSummary)
            || q.scope.iter().any(|s| !self.sources.contains(s))
            || q.budgets.output_bytes as usize > MAX_OUTPUT_BYTES
        {
            return Err(Error::Forbidden);
        }
        q.scope.sort();
        if q.scope.windows(2).any(|pair| pair[0] == pair[1])
            || (matches!(q.operation, Operation::RunSummary) && q.filter.run.is_none())
        {
            return Err(Error::InvalidRequest);
        }
        Ok(q)
    }

    /// Authenticated canonical exchange; the caller must establish an independently
    /// authenticated session and hold its resource permit through response cleanup.
    pub async fn authorize_and_query(
        &self,
        client: &FabricQueryClient,
        bytes: &[u8],
    ) -> Result<Vec<u8>, Error> {
        let q = self.authorize(bytes)?;
        let timeout = Duration::from_millis(u64::from(q.budgets.timeout_ms));
        let result = tokio::time::timeout(
            timeout,
            client.exchange_direct(Request::Query { request: q.clone() }),
        )
        .await
        .map_err(|_| Error::BackendUnavailable)?
        .map_err(|_| Error::BackendUnavailable)?;
        let Response::Query { response } = result else {
            return Err(Error::InvalidResponse);
        };
        self.validate_response(&q, response)
    }

    fn validate_response(
        &self,
        q: &QueryRequest,
        response: QueryResponse,
    ) -> Result<Vec<u8>, Error> {
        let c = &response.coverage;
        let snapshot_matches = if q.snapshot.is_empty() {
            c.snapshot.len() == q.scope.len() && q.scope.iter().all(|s| c.snapshot.contains_key(s))
        } else {
            c.snapshot == q.snapshot
        };
        if response.version != API_VERSION
            || c.transform != TRANSFORM_VERSION
            || c.canonical_authority != self.authority
            || c.scope != q.scope
            || !snapshot_matches
            || c.canonical_frontier
                .keys()
                .chain(c.projection.frontiers.keys())
                .any(|s| !q.scope.contains(s))
            || c.complete == c.truncated
            || c.truncated != response.next_cursor.is_some()
            || response.rows.len() > q.budgets.rows as usize
            || (matches!(q.operation, Operation::RunSummary) && response.rows.len() > 1)
        {
            return Err(Error::InvalidResponse);
        }
        let bytes = serde_json::to_vec(&response).map_err(|_| Error::InvalidResponse)?;
        if bytes.len() > MAX_OUTPUT_BYTES || bytes.len() > q.budgets.output_bytes as usize {
            return Err(Error::InvalidResponse);
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy() -> QueryPolicy {
        QueryPolicy::new("auth-1".into(), vec!["alpha".into()]).unwrap()
    }
    fn request() -> Vec<u8> {
        include_bytes!("../../fixtures/request.json").to_vec()
    }
    fn response() -> QueryResponse {
        serde_json::from_slice(include_bytes!("../../fixtures/first.json")).unwrap()
    }
    #[test]
    fn rejects_outside_scope_authority_and_size() {
        let p = policy();
        let q = p.authorize(&request()).unwrap();
        let mut changed = q.clone();
        changed.scope = vec!["other".into()];
        assert_eq!(
            p.authorize(&serde_json::to_vec(&changed).unwrap()),
            Err(Error::Forbidden)
        );
        changed = q;
        changed.authority = Some("other".into());
        assert_eq!(
            p.authorize(&serde_json::to_vec(&changed).unwrap()),
            Err(Error::Forbidden)
        );
        assert_eq!(
            p.authorize(&vec![b' '; MAX_INPUT_BYTES + 1]),
            Err(Error::InvalidRequest)
        );
    }
    #[test]
    fn rejects_wrong_response_scope_and_snapshot() {
        let p = policy();
        let mut q = p.authorize(&request()).unwrap();
        let good = response();
        assert!(p.validate_response(&q, good.clone()).is_ok());
        let mut bad = good.clone();
        bad.coverage.canonical_authority = "other".into();
        assert_eq!(p.validate_response(&q, bad), Err(Error::InvalidResponse));
        q.snapshot.insert("alpha".into(), 4);
        assert_eq!(p.validate_response(&q, good), Err(Error::InvalidResponse));
    }
}
