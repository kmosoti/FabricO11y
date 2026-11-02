//! Versioned local observation wire contract. New producers write v2 only.
use serde::{Deserialize, Serialize};

#[path = "../../registry/vocabulary.rs"]
pub mod vocabulary;

pub const VERSION: u32 = 2;
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
impl Observation {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != VERSION
            || self.registry != REGISTRY
            || self.identity.sequence == 0
            || !text(&self.identity.producer, 128)
            || !text(&self.identity.generation, 128)
            || !text(&self.resource, 256)
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
                    if depth > MAX_DEPTH {
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
