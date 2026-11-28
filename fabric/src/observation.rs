//! Versioned local observation wire contract. Supports immutable v2 and context-bearing v3.
use serde::{Deserialize, Serialize};

#[path = "observation_framing.rs"]
mod framing;
pub use framing::{
    frame_untrusted_telemetry, sanitize_untrusted_json_value, sanitize_untrusted_telemetry_str,
};

#[path = "../../registry/vocabulary.rs"]
pub mod vocabulary;

pub const VERSION: u32 = 2;
pub const VERSION_V3: u32 = 3;
pub const VERSION_V4: u32 = 4;
pub const DESCRIPTOR_V5: &str = include_str!("../../registry/v5/descriptor.json");
pub const REGISTRY_V4: &str = "fabrico11y/4";
pub const DESCRIPTOR_V4: &str = include_str!("../../registry/v4/descriptor.json");
pub const REGISTRY_V3: &str = "fabrico11y/3";
pub const DESCRIPTOR_V3: &str = include_str!("../../registry/v3/descriptor.json");
pub const MAX_FRAME: usize = 65536;
pub const MAX_DEPTH: usize = 16;
pub const REGISTRY: &str = "fabrico11y/2";
pub const DESCRIPTOR: &str = include_str!("../../registry/descriptor.json");

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub producer: String,
    pub generation: String,
    pub sequence: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub version: u32,
    pub registry: String,
    pub identity: Identity,
    pub timestamp_ns: u64,
    pub resource: String,
    pub data: Data,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Data {
    ServiceRequest {
        service: String,
        operation: String,
        duration_ns: u64,
        status: Outcome,
        trace_id: Option<String>,
    },
    AgentRun {
        agent: String,
        run_id: String,
        duration_ns: u64,
        status: Outcome,
    },
    ToolExecution {
        tool: String,
        run_id: String,
        duration_ns: u64,
        status: Outcome,
    },
    Log {
        level: Level,
        message: String,
    },
    JsonLog {
        fields: serde_json::Map<String, serde_json::Value>,
    },
    Event {
        name: String,
        message: String,
    },
    Metric {
        name: String,
        unit: String,
        value: f64,
        metric_type: MetricType,
        reset: bool,
    },
    Gap {
        source: String,
        reason: String,
    },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    Error,
    Cancelled,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MetricType {
    Counter,
    Gauge,
}
fn text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.contains('\0')
}
fn valid_identity_resource(identity: &Identity, resource: &str) -> bool {
    identity.sequence != 0
        && text(&identity.producer, 128)
        && text(&identity.generation, 128)
        && text(resource, 256)
}
impl Observation {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != VERSION
            || self.registry != REGISTRY
            || !valid_identity_resource(&self.identity, &self.resource)
        {
            return Err("invalid envelope".into());
        }
        if self.data.validate() {
            Ok(())
        } else {
            Err("invalid observation fields".into())
        }
    }
}
impl Data {
    pub fn validate_in_context(&self, context: &crate::run::RunContext) -> bool {
        self.validate()
            && match self {
                Self::AgentRun { run_id, .. } | Self::ToolExecution { run_id, .. } => {
                    run_id == &context.run_id
                }
                _ => true,
            }
    }
    pub fn validate(&self) -> bool {
        match self {
            Data::ServiceRequest {
                service,
                operation,
                trace_id,
                ..
            } => {
                text(service, 256)
                    && text(operation, 256)
                    && trace_id.as_ref().is_none_or(|s| text(s, 128))
            }
            Data::AgentRun { agent, run_id, .. } => text(agent, 256) && text(run_id, 128),
            Data::ToolExecution { tool, run_id, .. } => text(tool, 256) && text(run_id, 128),
            Data::Log { message, .. } => text(message, 32768),
            Data::JsonLog { fields } => {
                fields.len() <= 64 && !fields.is_empty() && {
                    let mut budget = 4096usize;
                    let mut bytes = 0usize;
                    fields
                        .iter()
                        .all(|(k, v)| text(k, 256) && valid_json(v, 0, &mut budget, &mut bytes))
                }
            }
            Data::Event { name, message } => text(name, 256) && text(message, 32768),
            Data::Metric {
                name,
                unit,
                value,
                metric_type,
                ..
            } => {
                text(name, 256)
                    && text(unit, 64)
                    && value.is_finite()
                    && (*metric_type != MetricType::Counter || *value >= 0.0)
            }
            Data::Gap { source, reason } => text(source, 256) && text(reason, 4096),
        }
    }
}
/// Checks wire size and syntactic nesting before any owned decoding allocations.
pub fn decode(bytes: &[u8]) -> Result<Observation, String> {
    check_json(bytes, MAX_FRAME)?;
    let value: Observation = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    value.validate()?;
    Ok(value)
}
pub fn check_json(bytes: &[u8], limit: usize) -> Result<(), String> {
    check_json_depth(bytes, limit, MAX_DEPTH)
}
pub fn check_json_depth(bytes: &[u8], limit: usize, max_depth: usize) -> Result<(), String> {
    if bytes.len() > limit {
        return Err("frame too large".into());
    }
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for &b in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
            }
        } else {
            match b {
                b'"' => quoted = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > max_depth {
                        return Err("nesting limit".into());
                    }
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    Ok(())
}
/// Legacy reads are deliberate and keep their original, unitless prototype data.
pub enum ReadObservation {
    V1(crate::collection::Observation),
    V2(Observation),
}
pub fn read_legacy(bytes: &[u8]) -> Result<ReadObservation, String> {
    check_json(bytes, MAX_FRAME)?;
    let o: crate::collection::Observation =
        serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    o.validate().map_err(str::to_owned)?;
    Ok(ReadObservation::V1(o))
}
pub fn reserved_producer(producer: &str) -> bool {
    producer == "host" || producer.starts_with("file:") || producer.starts_with("journal:")
}
pub fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u64::MAX as u128) as u64
}

fn valid_json(v: &serde_json::Value, depth: usize, budget: &mut usize, bytes: &mut usize) -> bool {
    if depth > MAX_DEPTH || *budget == 0 {
        return false;
    }
    *budget -= 1;
    match v {
        serde_json::Value::String(s) => {
            *bytes = bytes.saturating_add(s.len());
            s.len() <= 32768 && *bytes <= MAX_FRAME
        }
        serde_json::Value::Array(a) => {
            a.len() <= 64 && a.iter().all(|v| valid_json(v, depth + 1, budget, bytes))
        }
        serde_json::Value::Object(m) => {
            m.len() <= 64
                && m.iter().all(|(k, v)| {
                    *bytes = bytes.saturating_add(k.len());
                    k.len() <= 256 && *bytes <= MAX_FRAME && valid_json(v, depth + 1, budget, bytes)
                })
        }
        _ => true,
    }
}

/// Context-bearing revision; v2's Rust and wire interface remains unchanged.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ObservationV3 {
    pub version: u32,
    pub registry: String,
    pub context: crate::run::RunContext,
    pub identity: Identity,
    pub timestamp_ns: u64,
    pub resource: String,
    pub data: Data,
}
impl ObservationV3 {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != VERSION_V3 || self.registry != REGISTRY_V3 {
            return Err("invalid v3 envelope".into());
        }
        self.context.validate()?;
        if !valid_identity_resource(&self.identity, &self.resource)
            || !self.data.validate_in_context(&self.context)
        {
            return Err("invalid v3 observation fields".into());
        }
        Ok(())
    }
}

/// Caller-supplied trace context. Parent identity is scoped to the trace; it
/// does not prove parent existence, uniqueness, or authentication.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SpanContext {
    pub trace_id: String,
    pub span_id: String,
    #[serde(default)]
    pub parent_span_id: Option<String>,
}
fn valid_span_id(value: &str, width: usize) -> bool {
    value.len() == width
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && value.bytes().any(|b| b != b'0')
}
impl SpanContext {
    fn validate(&self) -> bool {
        valid_span_id(&self.trace_id, 32)
            && valid_span_id(&self.span_id, 16)
            && self
                .parent_span_id
                .as_ref()
                .is_none_or(|id| valid_span_id(id, 16) && id != &self.span_id)
    }
}
/// Explicit opt-in trace-bearing observation. Other observation kinds retain
/// their existing v2/v3 wire contracts.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ObservationV4 {
    pub version: u32,
    pub registry: String,
    pub context: crate::run::RunContext,
    pub identity: Identity,
    pub timestamp_ns: u64,
    pub resource: String,
    pub data: Data,
    pub span: SpanContext,
}
impl ObservationV4 {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != VERSION_V4 || self.registry != REGISTRY_V4 || !self.span.validate() {
            return Err("invalid v4 envelope".into());
        }
        self.context.validate()?;
        if !valid_identity_resource(&self.identity, &self.resource)
            || !self.data.validate_in_context(&self.context)
        {
            return Err("invalid v4 observation fields".into());
        }
        match &self.data {
            Data::ServiceRequest { trace_id, .. }
                if trace_id.as_ref().is_none_or(|id| id == &self.span.trace_id) =>
            {
                Ok(())
            }
            Data::AgentRun { .. } | Data::ToolExecution { .. } => Ok(()),
            _ => Err("invalid v4 trace data".into()),
        }
    }
}
/// Serialize the explicit v4 envelope; existing SDK encoders remain unchanged.
pub fn encode_v4(observation: &ObservationV4) -> Result<Vec<u8>, String> {
    observation.validate()?;
    let bytes = serde_json::to_vec(observation).map_err(|e| e.to_string())?;
    check_json(&bytes, MAX_FRAME)?;
    Ok(bytes)
}

// RawValue borrows the original token without decoding escape sequences or
// allocating an owned ID. Only exact quoted lowercase hex is accepted.
fn raw_span_id(raw: &serde_json::value::RawValue, width: usize) -> Result<&str, String> {
    let token = raw.get();
    let bytes = token.as_bytes();
    if bytes.len() != width + 2 || bytes.first() != Some(&b'"') || bytes.last() != Some(&b'"') {
        return Err("invalid raw span identity".into());
    }
    let id = &token[1..token.len() - 1];
    if !valid_span_id(id, width) {
        return Err("invalid raw span identity".into());
    }
    Ok(id)
}
pub(crate) fn validate_v4_raw(bytes: &[u8]) -> Result<(), String> {
    use serde_json::value::RawValue;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RawSpan<'a> {
        #[serde(borrow)]
        trace_id: &'a RawValue,
        #[serde(borrow)]
        span_id: &'a RawValue,
        #[serde(default, borrow)]
        parent_span_id: Option<&'a RawValue>,
    }
    #[derive(Deserialize)]
    struct RawData<'a> {
        #[serde(default, borrow)]
        trace_id: Option<&'a RawValue>,
    }
    #[derive(Deserialize)]
    struct RawEnvelope<'a> {
        #[serde(borrow)]
        span: RawSpan<'a>,
        #[serde(borrow)]
        data: RawData<'a>,
    }
    let raw: RawEnvelope<'_> = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let trace = raw_span_id(raw.span.trace_id, 32)?;
    let span = raw_span_id(raw.span.span_id, 16)?;
    if let Some(parent) = raw.span.parent_span_id
        && raw_span_id(parent, 16)? == span
    {
        return Err("span cannot parent itself".into());
    }
    if let Some(data_trace) = raw.data.trace_id
        && raw_span_id(data_trace, 32)? != trace
    {
        return Err("trace identity mismatch".into());
    }
    Ok(())
}

/// Sealed v5 record with a derived legacy metric view. Raw custody bytes are stored separately.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredMetricV5 {
    observation: ObservationV5,
    legacy: Data,
}
impl StoredMetricV5 {
    fn new(observation: ObservationV5) -> Self {
        let metric = &observation.data;
        let legacy = Data::Metric {
            name: metric.name.clone(),
            unit: metric.unit.clone(),
            value: metric.value,
            metric_type: metric.metric_type,
            reset: metric.reset,
        };
        Self {
            observation,
            legacy,
        }
    }
}
impl Serialize for StoredMetricV5 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.observation.serialize(serializer)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum StoredObservation {
    V2(Observation),
    V3(ObservationV3),
    V4(ObservationV4),
    V5(StoredMetricV5),
}
impl StoredObservation {
    pub fn registry(&self) -> &str {
        match self {
            Self::V2(o) => &o.registry,
            Self::V3(o) => &o.registry,
            Self::V4(o) => &o.registry,
            Self::V5(o) => &o.observation.registry,
        }
    }
    pub fn identity(&self) -> &Identity {
        match self {
            Self::V2(o) => &o.identity,
            Self::V3(o) => &o.identity,
            Self::V4(o) => &o.identity,
            Self::V5(o) => &o.observation.identity,
        }
    }
    pub fn context(&self) -> Option<&crate::run::RunContext> {
        match self {
            Self::V2(_) => None,
            Self::V3(o) => Some(&o.context),
            Self::V4(o) => Some(&o.context),
            Self::V5(o) => Some(&o.observation.context),
        }
    }
    pub fn timestamp_ns(&self) -> u64 {
        match self {
            Self::V2(o) => o.timestamp_ns,
            Self::V3(o) => o.timestamp_ns,
            Self::V4(o) => o.timestamp_ns,
            Self::V5(o) => o.observation.timestamp_ns,
        }
    }
    pub fn data(&self) -> &Data {
        match self {
            Self::V2(o) => &o.data,
            Self::V3(o) => &o.data,
            Self::V4(o) => &o.data,
            Self::V5(o) => &o.legacy,
        }
    }
    pub fn resource(&self) -> &str {
        match self {
            Self::V2(o) => &o.resource,
            Self::V3(o) => &o.resource,
            Self::V4(o) => &o.resource,
            Self::V5(o) => &o.observation.resource,
        }
    }
    pub fn metric_v5(&self) -> Option<&ObservationV5> {
        match self {
            Self::V5(o) => Some(&o.observation),
            _ => None,
        }
    }
    pub fn span(&self) -> Option<&SpanContext> {
        match self {
            Self::V4(o) => Some(&o.span),
            _ => None,
        }
    }
    pub fn version(&self) -> u32 {
        match self {
            Self::V2(o) => o.version,
            Self::V3(o) => o.version,
            Self::V4(o) => o.version,
            Self::V5(o) => o.observation.version,
        }
    }
}
impl<'de> Deserialize<'de> for StoredObservation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Compatible {
            V2(Observation),
            V3(ObservationV3),
            V4(ObservationV4),
            V5(ObservationV5),
        }
        let v5 = match Compatible::deserialize(deserializer)? {
            Compatible::V2(o) => {
                o.validate().map_err(serde::de::Error::custom)?;
                return Ok(Self::V2(o));
            }
            Compatible::V3(o) if o.version == VERSION_V3 => {
                o.validate().map_err(serde::de::Error::custom)?;
                return Ok(Self::V3(o));
            }
            Compatible::V3(o) if o.version == VERSION_V5 => {
                let Data::Metric {
                    name,
                    unit,
                    value,
                    metric_type,
                    reset,
                } = o.data
                else {
                    return Err(serde::de::Error::custom("v5 requires metric data"));
                };
                ObservationV5 {
                    version: o.version,
                    registry: o.registry,
                    context: o.context,
                    identity: o.identity,
                    timestamp_ns: o.timestamp_ns,
                    resource: o.resource,
                    data: MetricV5 {
                        kind: MetricKind::Metric,
                        name,
                        unit,
                        value,
                        metric_type,
                        reset,
                        labels: Default::default(),
                    },
                }
            }
            Compatible::V3(_) => {
                return Err(serde::de::Error::custom("unsupported observation version"));
            }
            Compatible::V4(o) => {
                o.validate().map_err(serde::de::Error::custom)?;
                return Ok(Self::V4(o));
            }
            Compatible::V5(o) => o,
        };
        v5.validate().map_err(serde::de::Error::custom)?;
        Ok(Self::V5(StoredMetricV5::new(v5)))
    }
}
pub fn decode_versioned(bytes: &[u8]) -> Result<StoredObservation, String> {
    check_json(bytes, MAX_FRAME)?;
    #[derive(Deserialize)]
    struct Header {
        version: u32,
    }
    match serde_json::from_slice::<Header>(bytes)
        .map_err(|e| e.to_string())?
        .version
    {
        VERSION => decode(bytes).map(StoredObservation::V2),
        VERSION_V3 => {
            let observation: ObservationV3 =
                serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
            observation.validate()?;
            Ok(StoredObservation::V3(observation))
        }
        VERSION_V4 => {
            validate_v4_raw(bytes)?;
            let observation: ObservationV4 =
                serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
            observation.validate()?;
            Ok(StoredObservation::V4(observation))
        }
        VERSION_V5 => {
            decode_metric_v5(bytes).map(|o| StoredObservation::V5(StoredMetricV5::new(o)))
        }
        _ => Err("unsupported observation version".into()),
    }
}

#[cfg(test)]
mod v4_tests {
    use super::*;
    const RUN: &str = "00000000-0000-4000-8000-000000000001";
    const TRACE: &str = "11111111111111111111111111111111";
    const SPAN: &str = "2222222222222222";
    fn envelope(data: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"version":4,"registry":"fabrico11y/4","context":{"run_id":RUN},"identity":{"producer":"producer","generation":"generation","sequence":1},"timestamp_ns":1,"resource":"resource","data":data,"span":{"trace_id":TRACE,"span_id":SPAN,"parent_span_id":"3333333333333333"}})
    }
    fn encode(value: &serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(value).unwrap()
    }
    #[test]
    fn retained_v4_registry_fixtures_match_wire_validation() {
        let cases: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../registry/v4/fixtures.json")).unwrap();
        assert_eq!(cases.len(), 13);
        for case in cases {
            let bytes = serde_json::to_vec(&case["wire"]).unwrap();
            let observed = decode_versioned(&bytes);
            assert_eq!(
                observed.is_ok(),
                case["valid"].as_bool().unwrap(),
                "{}: {observed:?}",
                case["name"],
            );
            if let Ok(record) = observed {
                assert_eq!(record.version(), VERSION_V4);
            }
        }
    }

    #[test]
    fn v4_explicit_roundtrip_and_three_trace_kinds() {
        for data in [
            serde_json::json!({"kind":"service_request","service":"api","operation":"get","duration_ns":4,"status":"ok","trace_id":TRACE}),
            serde_json::json!({"kind":"agent_run","agent":"agent","run_id":RUN,"duration_ns":4,"status":"ok"}),
            serde_json::json!({"kind":"tool_execution","tool":"tool","run_id":RUN,"duration_ns":4,"status":"ok"}),
        ] {
            let decoded = decode_versioned(&encode(&envelope(data))).unwrap();
            let StoredObservation::V4(observation) = decoded else {
                panic!("expected v4");
            };
            let bytes = encode_v4(&observation).unwrap();
            assert_eq!(
                decode_versioned(&bytes).unwrap(),
                StoredObservation::V4(observation)
            );
        }
        let mut no_parent = envelope(
            serde_json::json!({"kind":"agent_run","agent":"agent","run_id":RUN,"duration_ns":1,"status":"ok"}),
        );
        no_parent["span"]
            .as_object_mut()
            .unwrap()
            .remove("parent_span_id");
        assert!(decode_versioned(&encode(&no_parent)).is_ok());
        no_parent["span"]["parent_span_id"] = serde_json::Value::Null;
        assert!(decode_versioned(&encode(&no_parent)).is_ok());
    }
    #[test]
    fn v4_rejects_invalid_raw_ids_scope_and_shape() {
        let base = envelope(
            serde_json::json!({"kind":"service_request","service":"api","operation":"get","duration_ns":4,"status":"ok","trace_id":TRACE}),
        );
        for (field, value) in [
            ("trace_id", "00000000000000000000000000000000"),
            ("trace_id", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
            ("span_id", "0000000000000000"),
            ("span_id", "123"),
            ("parent_span_id", SPAN),
        ] {
            let mut bad = base.clone();
            bad["span"][field] = value.into();
            assert!(decode_versioned(&encode(&bad)).is_err());
        }
        let mut bad = base.clone();
        bad["span"]["trace_id"] = "1".repeat(10_000).into();
        assert!(decode_versioned(&encode(&bad)).is_err());
        bad = base.clone();
        bad["span"]["parent_span_id"] = "0000000000000000".into();
        assert!(decode_versioned(&encode(&bad)).is_err());
        bad = base.clone();
        bad["data"]["trace_id"] = "44444444444444444444444444444444".into();
        assert!(decode_versioned(&encode(&bad)).is_err());
        bad = base.clone();
        bad["data"] = serde_json::json!({"kind":"event","name":"event","message":"text"});
        assert!(decode_versioned(&encode(&bad)).is_err());
        bad = base.clone();
        bad["span"]["unexpected"] = true.into();
        assert!(decode_versioned(&encode(&bad)).is_err());
        bad = base.clone();
        bad.as_object_mut().unwrap().remove("span");
        assert!(decode_versioned(&encode(&bad)).is_err());
        let raw = String::from_utf8(encode(&base)).unwrap();
        let escaped = raw.replacen(TRACE, "\\u00311111111111111111111111111111111", 1);
        assert_ne!(escaped, raw);
        assert!(decode_versioned(escaped.as_bytes()).is_err());
        let escaped_span = raw.replacen(SPAN, "\\u0032222222222222222", 1);
        assert_ne!(escaped_span, raw);
        assert!(decode_versioned(escaped_span.as_bytes()).is_err());
        let field = format!("\"span_id\":\"{SPAN}\"");
        let duplicate = raw.replacen(&field, &format!("{field},{field}"), 1);
        assert_ne!(duplicate, raw);
        assert!(decode_versioned(duplicate.as_bytes()).is_err());
        assert!(decode_versioned(&vec![b' '; MAX_FRAME + 1]).is_err());
    }
}

#[path = "observation_metric_v5.rs"]
mod metric_v5;
pub use metric_v5::{
    MetricKind, MetricV5, ObservationV5, REGISTRY_V5, VERSION_V5, decode_metric_v5,
};
