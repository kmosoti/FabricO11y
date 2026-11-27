//! Versioned, Arrow-free investigation contract shared by Rust, CLI and MCP.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

fn bounded_text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.contains('\0')
}

pub const API_VERSION: u32 = 1;
pub const TRANSFORM_VERSION: &str = "fabrico11y/investigation/1";
pub type Snapshot = BTreeMap<String, u64>;

/// Wide unsigned values use decimal strings on the wire, including map values.
pub mod decimal {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        let s = String::deserialize(d)?;
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(serde::de::Error::custom("expected unsigned decimal string"));
        }
        s.parse().map_err(serde::de::Error::custom)
    }
}
pub mod snapshot_decimal {
    use super::*;
    use serde::{Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &Snapshot, s: S) -> Result<S::Ok, S::Error> {
        v.iter()
            .map(|(k, v)| (k, v.to_string()))
            .collect::<BTreeMap<_, _>>()
            .serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Snapshot, D::Error> {
        BTreeMap::<String, String>::deserialize(d)?
            .into_iter()
            .map(|(k, v)| {
                if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(serde::de::Error::custom("expected decimal prefix"));
                }
                Ok((k, v.parse().map_err(serde::de::Error::custom)?))
            })
            .collect()
    }
}
#[cfg_attr(feature = "query-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Budgets {
    pub rows: u32,
    pub output_bytes: u32,
    pub scan_rows: u32,
    pub timeout_ms: u32,
}
impl Default for Budgets {
    fn default() -> Self {
        Self {
            rows: 100,
            output_bytes: 256 * 1024,
            scan_rows: 1_000_000,
            timeout_ms: 10_000,
        }
    }
}
impl Budgets {
    pub fn validate(&self) -> Result<(), String> {
        if self.rows == 0
            || self.rows > 1000
            || self.output_bytes < 1024
            || self.output_bytes > 1024 * 1024
            || self.scan_rows == 0
            || self.scan_rows > 1_000_000
            || self.timeout_ms == 0
            || self.timeout_ms > 30_000
        {
            return Err("query budget exceeds limits".into());
        }
        Ok(())
    }
}
/// UUID-backed v3 runs share one global key; v2 keys encode all four components.
#[cfg_attr(feature = "query-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunKey {
    Global {
        run_id: String,
    },
    Legacy {
        origin: String,
        producer: String,
        generation: String,
        run_id: String,
    },
}
impl RunKey {
    pub fn encoded(&self) -> String {
        serde_json::to_string(self).expect("run key serialization")
    }
}
#[cfg_attr(feature = "query-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub run: Option<RunKey>,
    /// Inclusive lower bound, exclusive upper bound.
    #[cfg_attr(feature = "query-schema", schemars(with = "Option<String>"))]
    #[serde(default, with = "optional_decimal")]
    pub start_ns: Option<u64>,
    #[cfg_attr(feature = "query-schema", schemars(with = "Option<String>"))]
    #[serde(default, with = "optional_decimal")]
    pub end_ns: Option<u64>,
    pub resource: Option<String>,
}
pub mod optional_decimal {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => s.serialize_some(&v.to_string()),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|s| {
                if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(serde::de::Error::custom("expected unsigned decimal string"));
                }
                s.parse().map_err(serde::de::Error::custom)
            })
            .transpose()
    }
}
#[cfg_attr(feature = "query-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Capabilities,
    ListRuns,
    RunSummary,
    ToolSummary,
    ServiceSummary,
    Activity {
        #[cfg_attr(feature = "query-schema", schemars(with = "String"))]
        #[serde(with = "decimal")]
        bucket_ns: u64,
    },
    Evidence,
}
#[cfg_attr(feature = "query-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    pub version: u32,
    pub query_id: String,
    #[serde(default)]
    pub authority: Option<String>,
    /// Requested sources; receiver authorization intersects this before execution.
    pub scope: Vec<String>,
    #[cfg_attr(feature = "query-schema", schemars(with = "BTreeMap<String,String>"))]
    #[serde(default, with = "snapshot_decimal")]
    pub snapshot: Snapshot,
    #[serde(default)]
    pub filter: Filter,
    pub operation: Operation,
    #[serde(default)]
    pub budgets: Budgets,
    pub cursor: Option<String>,
}
impl QueryRequest {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != API_VERSION {
            return Err("unsupported query API version".into());
        }
        self.budgets.validate()?;
        if self
            .authority
            .as_ref()
            .is_some_and(|a| a.is_empty() || a.len() > 256)
        {
            return Err("invalid canonical authority".into());
        }
        if !bounded_text(&self.query_id, 128) {
            return Err("bounded query_id required".into());
        }
        if self.scope.is_empty()
            || self.scope.len() > 32
            || self.scope.iter().any(|s| !bounded_text(s, 256))
        {
            return Err("explicit bounded source scope required".into());
        }
        if self.snapshot.len() > 32 || self.snapshot.keys().any(|s| !self.scope.contains(s)) {
            return Err("snapshot outside scope".into());
        }
        if self
            .filter
            .resource
            .as_ref()
            .is_some_and(|s| !bounded_text(s, 256))
        {
            return Err("invalid resource filter".into());
        }
        match &self.filter.run {
            Some(RunKey::Global { run_id }) => crate::run::RunContext {
                run_id: run_id.clone(),
            }
            .validate()?,
            Some(RunKey::Legacy {
                origin,
                producer,
                generation,
                run_id,
            }) if !bounded_text(origin, 256)
                || !bounded_text(producer, 128)
                || !bounded_text(generation, 128)
                || !bounded_text(run_id, 128) =>
            {
                return Err("invalid legacy run filter".into());
            }
            Some(RunKey::Legacy { .. }) => {}
            None => {}
        }
        if self
            .filter
            .start_ns
            .zip(self.filter.end_ns)
            .is_some_and(|(a, b)| a >= b)
        {
            return Err("invalid time range".into());
        }
        if matches!(self.operation, Operation::Activity { bucket_ns: 0 }) {
            return Err("bucket must be positive".into());
        }
        if self.cursor.as_ref().is_some_and(|s| s.len() > 262144) {
            return Err("cursor too large".into());
        }
        Ok(())
    }
}
#[cfg_attr(feature = "query-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectionProgress {
    #[cfg_attr(feature = "query-schema", schemars(with = "String"))]
    #[serde(with = "decimal")]
    pub checkpoint: u64,
    #[cfg_attr(feature = "query-schema", schemars(with = "BTreeMap<String,String>"))]
    #[serde(with = "snapshot_decimal")]
    pub frontiers: Snapshot,
}
#[cfg_attr(feature = "query-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConsumedBudget {
    /// Conservatively charged logical records in selected segments, before
    /// predicate pushdown; repeated column passes do not add logical records.
    pub scanned_rows: u32,
    pub output_bytes: u32,
    pub elapsed_ms: u32,
}
#[cfg_attr(feature = "query-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Coverage {
    pub canonical_authority: String,
    pub scope: Vec<String>,
    #[cfg_attr(feature = "query-schema", schemars(with = "BTreeMap<String,String>"))]
    #[serde(with = "snapshot_decimal")]
    pub snapshot: Snapshot,
    pub transform: String,
    pub schemas: Vec<String>,
    pub projection: ProjectionProgress,
    #[cfg_attr(feature = "query-schema", schemars(with = "BTreeMap<String,String>"))]
    #[serde(with = "snapshot_decimal")]
    pub canonical_frontier: Snapshot,
    pub observed_gaps: Vec<String>,
    pub complete: bool,
    pub truncated: bool,
    pub consumed: ConsumedBudget,
}
/// Every integer value in rows (including counts, timestamps and evidence IDs) is
/// represented as a decimal JSON string. Ratios carry numerator and denominator.
#[cfg_attr(feature = "query-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct QueryResponse {
    pub version: u32,
    pub rows: Vec<serde_json::Value>,
    pub coverage: Coverage,
    pub next_cursor: Option<String>,
}
#[cfg_attr(feature = "query-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QueryPlan {
    pub version: u32,
    pub canonical_authority: String,
    pub scope: Vec<String>,
    #[cfg_attr(feature = "query-schema", schemars(with = "BTreeMap<String,String>"))]
    #[serde(with = "snapshot_decimal")]
    pub snapshot: Snapshot,
    pub schemas: Vec<String>,
    pub selected_segments: Option<u32>,
    pub estimated_scan_rows: u32,
    pub projection: ProjectionProgress,
    pub budgets: Budgets,
    pub transform: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wide_wire_values_are_strings() {
        let p = ProjectionProgress {
            checkpoint: u64::MAX,
            frontiers: BTreeMap::from([("a".into(), u64::MAX)]),
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains("\"18446744073709551615\""));
        assert_eq!(serde_json::from_str::<ProjectionProgress>(&s).unwrap(), p);
        assert!(
            serde_json::from_str::<ProjectionProgress>(
                "{\"checkpoint\":18446744073709551615,\"frontiers\":{}}"
            )
            .is_err()
        );
    }
    #[test]
    fn legacy_keys_do_not_collide() {
        let k = |origin: &str, producer: &str| {
            RunKey::Legacy {
                origin: origin.into(),
                producer: producer.into(),
                generation: "g".into(),
                run_id: "r".into(),
            }
            .encoded()
        };
        assert_ne!(k("a:b", "c"), k("a", "b:c"));
    }
}

/// Credential-bound client used unchanged by CLI and MCP. Credentials never enter
/// requests, cursors, query output or Debug formatting.
#[derive(Clone)]
pub struct Client {
    endpoint: String,
    credential: String,
}
/// Opens `path` once with `O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC`, validates ownership,
/// regular-file type, optional `0o077` privacy mask, and byte ceiling on `fstat`,
/// and reads directly from that opened descriptor (zero pathname TOCTOU).
pub fn read_credential_fd(
    path: &std::path::Path,
    require_private_mode: bool,
    max_bytes: u64,
) -> Result<String, String> {
    use rustix::fs::{Mode, OFlags};
    use std::{fs::File, io::Read, os::unix::fs::MetadataExt};
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| {
        if error == rustix::io::Errno::LOOP {
            "credential file is symlinked".to_owned()
        } else {
            format!("credential file error: {error}")
        }
    })?;
    let file = File::from(fd);
    let metadata = file
        .metadata()
        .map_err(|error| format!("credential file error: {error}"))?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || (require_private_mode && metadata.mode() & 0o077 != 0)
        || metadata.len() == 0
        || metadata.len() > max_bytes
    {
        return Err(format!(
            "credential file must be owned, {}regular, nonempty and <={max_bytes} bytes",
            if require_private_mode {
                "private, "
            } else {
                ""
            }
        ));
    }
    let mut value = String::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_string(&mut value)
        .map_err(|error| format!("credential file read error: {error}"))?;
    if value.len() as u64 > max_bytes {
        return Err(format!("credential file exceeds {max_bytes} bytes"));
    }
    let trimmed = value.trim().to_owned();
    if trimmed.is_empty() {
        return Err("credential file is empty".into());
    }
    Ok(trimmed)
}

/// Reads the private credential file used by canonical readers.
pub fn read_credential_file(path: &std::path::Path) -> Result<String, String> {
    validate_credential(read_credential_fd(path, true, 256)?)
}

fn validate_credential(value: String) -> Result<String, String> {
    if value.len() < 16 || value.len() > 256 || !value.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("invalid credential format".into());
    }
    Ok(value)
}

/// Connection settings shared by the CLI and MCP host. A private file avoids
/// putting the credential itself in host configuration or command arguments.
pub fn connection_from_env() -> Result<(String, String), String> {
    let endpoint = std::env::var("FABRIC_ENDPOINT").map_err(|_| "FABRIC_ENDPOINT required")?;
    let credential = match (
        std::env::var_os("FABRIC_CREDENTIAL_FILE"),
        std::env::var_os("FABRIC_CREDENTIAL"),
    ) {
        (Some(path), None) => read_credential_file(std::path::Path::new(&path))?,
        (None, Some(value)) => validate_credential(
            value
                .into_string()
                .map_err(|_| "invalid credential encoding")?,
        )?,
        (Some(_), Some(_)) => {
            return Err("choose FABRIC_CREDENTIAL_FILE or FABRIC_CREDENTIAL".into());
        }
        (None, None) => return Err("FABRIC_CREDENTIAL_FILE required".into()),
    };
    Ok((endpoint, credential))
}
impl Client {
    pub fn new(endpoint: impl Into<String>, credential: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            credential: credential.into(),
        }
    }
    pub fn exchange(
        &self,
        request: &crate::canonical::Request,
        deadline: std::time::Instant,
    ) -> Result<crate::canonical::Response, String> {
        crate::canonical::exchange(&self.endpoint, &self.credential, request, deadline)
    }
    pub fn query(
        &self,
        request: QueryRequest,
        deadline: std::time::Instant,
    ) -> Result<QueryResponse, String> {
        request.validate()?;
        match self.exchange(&crate::canonical::Request::Query { request }, deadline)? {
            crate::canonical::Response::Query { response } => Ok(response),
            crate::canonical::Response::Error { error } => Err(error),
            _ => Err("unexpected query response".into()),
        }
    }
    pub fn plan(
        &self,
        request: QueryRequest,
        deadline: std::time::Instant,
    ) -> Result<QueryPlan, String> {
        request.validate()?;
        match self.exchange(&crate::canonical::Request::Plan { request }, deadline)? {
            crate::canonical::Response::Plan { plan } => Ok(plan),
            crate::canonical::Response::Error { error } => Err(error),
            _ => Err("unexpected plan response".into()),
        }
    }
}

/// Exact, typed interpretation of aggregate rows. `None` means there were no
/// duration-bearing observations; it does not invent a zero-duration sample.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Statistics {
    #[serde(with = "decimal")]
    pub count: u64,
    #[serde(with = "decimal")]
    pub errors: u64,
    #[serde(with = "decimal")]
    pub cancelled: u64,
    #[serde(with = "decimal")]
    pub duration_count: u64,
    #[serde(with = "optional_decimal")]
    pub min_ns: Option<u64>,
    #[serde(with = "optional_decimal")]
    pub max_ns: Option<u64>,
    #[serde(with = "optional_u128_decimal")]
    pub sum_ns: Option<u128>,
    pub mean_ns: Option<ExactMean>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExactMean {
    #[serde(with = "u128_decimal")]
    pub numerator: u128,
    #[serde(with = "decimal")]
    pub denominator: u64,
}
pub mod u128_decimal {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &u128, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
        let s = String::deserialize(d)?;
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(serde::de::Error::custom("expected unsigned decimal string"));
        }
        s.parse().map_err(serde::de::Error::custom)
    }
}
pub mod optional_u128_decimal {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &Option<u128>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => s.serialize_some(&v.to_string()),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u128>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|s| {
                if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(serde::de::Error::custom("expected unsigned decimal string"));
                }
                s.parse().map_err(serde::de::Error::custom)
            })
            .transpose()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunSummary {
    pub run: RunKey,
    #[serde(flatten)]
    pub statistics: Statistics,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolSummary {
    pub tool: String,
    #[serde(flatten)]
    pub statistics: Statistics,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceSummary {
    pub service: String,
    #[serde(flatten)]
    pub statistics: Statistics,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivityBucket {
    #[serde(with = "decimal")]
    pub bucket_start_ns: u64,
    #[serde(flatten)]
    pub statistics: Statistics,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    #[serde(with = "decimal")]
    pub canonical_id: u64,
    pub origin: String,
    #[serde(with = "decimal")]
    pub source_id: u64,
    #[serde(with = "decimal")]
    pub timestamp_ns: u64,
    pub run: Option<RunKey>,
    pub resource: String,
    pub raw: String,
}
impl QueryResponse {
    /// Decode one of the documented row types without exposing Arrow to clients.
    pub fn decode_rows<T: serde::de::DeserializeOwned>(&self) -> Result<Vec<T>, String> {
        self.rows
            .iter()
            .cloned()
            .map(|row| serde_json::from_value(row).map_err(|e| e.to_string()))
            .collect()
    }
}

/// Optional registry integration. It does not add fields to the v1 wire DTOs.
pub mod contracts {
    use super::{API_VERSION, Operation, QueryRequest, QueryResponse};
    use fabric_ops::Shape;
    use fabric_ops::registry::{
        Descriptor, Operation as OperationContract, Registry, RegistryError,
    };

    const fn descriptor(operation: &'static str) -> Descriptor {
        Descriptor {
            protocol: "fabric.query",
            protocol_version: API_VERSION,
            operation,
            operation_version: 1,
            shape: Shape::Unary,
        }
    }

    /// Local operation-contract version 1; this is not a field in QueryRequest.
    pub const QUERY_V1: [Descriptor; 7] = [
        descriptor("capabilities"),
        descriptor("list_runs"),
        descriptor("run_summary"),
        descriptor("tool_summary"),
        descriptor("service_summary"),
        descriptor("activity"),
        descriptor("evidence"),
    ];

    macro_rules! contract {
        ($name:ident, $index:literal) => {
            struct $name;
            impl OperationContract for $name {
                type Request = QueryRequest;
                type Response = QueryResponse;
                const DESCRIPTOR: Descriptor = QUERY_V1[$index];
            }
        };
    }
    contract!(Capabilities, 0);
    contract!(ListRuns, 1);
    contract!(RunSummary, 2);
    contract!(ToolSummary, 3);
    contract!(ServiceSummary, 4);
    contract!(Activity, 5);
    contract!(Evidence, 6);

    /// The request is held after its actual variant and versions are checked,
    /// so a different operation cannot be substituted before the handler.
    pub struct NegotiatedQuery {
        request: QueryRequest,
    }
    impl NegotiatedQuery {
        pub fn call(self, handler: impl FnOnce(QueryRequest) -> QueryResponse) -> QueryResponse {
            handler(self.request)
        }
    }

    /// This local gate does not authenticate, deserialize, or dispatch a wire call.
    pub fn negotiate(
        registry: &Registry,
        request: QueryRequest,
        operation_version: u32,
    ) -> Result<NegotiatedQuery, RegistryError> {
        macro_rules! check {
            ($binding:ty, $index:literal) => {
                registry.negotiate::<$binding>(
                    "fabric.query",
                    request.version,
                    QUERY_V1[$index].operation,
                    operation_version,
                )?
            };
        }
        match &request.operation {
            Operation::Capabilities => {
                check!(Capabilities, 0);
            }
            Operation::ListRuns => {
                check!(ListRuns, 1);
            }
            Operation::RunSummary => {
                check!(RunSummary, 2);
            }
            Operation::ToolSummary => {
                check!(ToolSummary, 3);
            }
            Operation::ServiceSummary => {
                check!(ServiceSummary, 4);
            }
            Operation::Activity { .. } => {
                check!(Activity, 5);
            }
            Operation::Evidence => {
                check!(Evidence, 6);
            }
        }
        Ok(NegotiatedQuery { request })
    }
}
