//! Experimental codecs, intentionally separate from the production wire protocol.
pub mod custody;
pub mod fixtures;
use fabric::observation::Observation;
use serde_json::{Map, Number, Value};
use std::collections::BTreeMap;
pub const MAX_INPUT: usize = 256 * 1024;
pub const MAX_FRAME: usize = 1024 * 1024;
const MAGIC: &[u8] = b"F11Y-EXPERIMENT-1/fabrico11y-2\0";
// Explicit stable codes within this experimental schema, never enum discriminants.
const KEYS: &[&str] = &[
    "version",
    "registry",
    "identity",
    "producer",
    "generation",
    "sequence",
    "timestamp_ns",
    "resource",
    "data",
    "kind",
    "service",
    "operation",
    "duration_ns",
    "status",
    "trace_id",
    "agent",
    "run_id",
    "tool",
    "level",
    "message",
    "fields",
    "name",
    "unit",
    "value",
    "metric_type",
    "reset",
    "source",
    "reason",
];
const ENUMS: &[&str] = &[
    "service_request",
    "agent_run",
    "tool_execution",
    "log",
    "json_log",
    "event",
    "metric",
    "gap",
    "trace",
    "debug",
    "info",
    "warn",
    "error",
    "ok",
    "cancelled",
    "counter",
    "gauge",
];
const INTERN: &[&str] = &[
    "registry",
    "producer",
    "generation",
    "resource",
    "kind",
    "service",
    "operation",
    "status",
    "agent",
    "tool",
    "level",
    "name",
    "unit",
    "metric_type",
];
const COLS: &[&str] = &[
    "version",
    "registry",
    "identity",
    "timestamp_ns",
    "resource",
    "data",
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Json,
    Rows,
    Columns,
}
impl Codec {
    pub const ALL: [Self; 3] = [Self::Json, Self::Rows, Self::Columns];
    pub fn name(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Rows => "rows",
            Self::Columns => "columns",
        }
    }
}
fn put32(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&(n as u32).to_le_bytes());
}
fn string(out: &mut Vec<u8>, s: &str) {
    put32(out, s.len());
    out.extend_from_slice(s.as_bytes());
}
fn gather(v: &Value, key: &str, dict: &mut BTreeMap<String, u16>, bytes: &mut usize) {
    match v {
        Value::String(s)
            if INTERN.contains(&key)
                && !["kind", "level", "status", "metric_type"].contains(&key)
                && !dict.contains_key(s)
                && dict.len() < 1024
                && *bytes + s.len() <= 65536 =>
        {
            let id = dict.len() as u16;
            *bytes += s.len();
            dict.insert(s.clone(), id);
        }
        Value::Object(m) => {
            for (k, v) in m {
                gather(v, k, dict, bytes)
            }
        }
        Value::Array(a) => {
            for v in a {
                gather(v, key, dict, bytes)
            }
        }
        _ => {}
    }
}
fn value(out: &mut Vec<u8>, v: &Value, key: &str, dict: &BTreeMap<String, u16>) {
    match v {
        Value::Null => out.push(0),
        Value::Bool(b) => out.push(if *b { 2 } else { 1 }),
        Value::Number(n) => {
            if let Some(x) = n.as_u64() {
                if let Ok(small) = u8::try_from(x) {
                    out.extend_from_slice(&[11, small]);
                } else {
                    out.push(3);
                    out.extend_from_slice(&x.to_le_bytes());
                }
            } else if let Some(x) = n.as_i64() {
                out.push(4);
                out.extend_from_slice(&x.to_le_bytes());
            } else {
                out.push(5);
                out.extend_from_slice(&n.as_f64().unwrap().to_bits().to_le_bytes());
            }
        }
        Value::String(s) => {
            if ["kind", "level", "status", "metric_type"].contains(&key)
                && let Some(code) = ENUMS.iter().position(|e| *e == s)
            {
                out.push(10);
                out.push(code as u8);
                return;
            }
            if INTERN.contains(&key)
                && let Some(id) = dict.get(s)
            {
                out.push(7);
                out.extend_from_slice(&id.to_le_bytes());
            } else {
                out.push(6);
                string(out, s);
            }
        }
        Value::Array(a) => {
            out.push(8);
            put32(out, a.len());
            for v in a {
                value(out, v, key, dict);
            }
        }
        Value::Object(m) => {
            out.push(9);
            put32(out, m.len());
            for (k, v) in m {
                if let Some(i) = KEYS.iter().position(|x| x == k) {
                    out.push(i as u8 + 1);
                } else {
                    out.push(0);
                    string(out, k);
                }
                value(out, v, k, dict);
            }
        }
    }
}
pub fn canonical(records: &[Observation]) -> Result<Vec<u8>, String> {
    if records.is_empty() || records.len() > 32 {
        return Err("record count".into());
    }
    for r in records {
        r.validate()?;
        let bytes = serde_json::to_vec(r).map_err(|e| e.to_string())?;
        fabric::observation::decode(&bytes)?;
    }
    let bytes = serde_json::to_vec(records).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_INPUT {
        return Err("expanded batch limit".into());
    }
    Ok(bytes)
}
pub fn encode(codec: Codec, records: &[Observation]) -> Result<Vec<u8>, String> {
    let raw = canonical(records)?;
    let mut out = MAGIC.to_vec();
    out.push(match codec {
        Codec::Json => 0,
        Codec::Rows => 1,
        Codec::Columns => 2,
    });
    if codec == Codec::Json {
        out.extend_from_slice(&raw);
        return Ok(out);
    }
    let values: Vec<Value> = records
        .iter()
        .map(|r| serde_json::to_value(r).unwrap())
        .collect();
    let mut dict = BTreeMap::new();
    let mut bytes = 0;
    if codec == Codec::Columns {
        for v in &values {
            gather(v, "", &mut dict, &mut bytes);
        }
    }
    put32(&mut out, dict.len());
    let mut ordered: Vec<_> = dict.iter().collect();
    ordered.sort_by_key(|(_, id)| **id);
    for (s, _) in ordered {
        string(&mut out, s);
    }
    put32(&mut out, records.len());
    if codec == Codec::Rows {
        for v in &values {
            value(&mut out, v, "", &dict);
        }
    } else {
        for key in COLS {
            for v in &values {
                value(&mut out, &v[*key], key, &dict);
            }
        }
    }
    if out.len() > MAX_FRAME {
        return Err("frame limit".into());
    }
    Ok(out)
}
struct Reader<'a> {
    b: &'a [u8],
    at: usize,
    expanded: usize,
    nodes: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).ok_or("overflow")?;
        let s = self.b.get(self.at..end).ok_or("truncated")?;
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<usize, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()) as usize)
    }
    fn charge(&mut self, n: usize) -> Result<(), String> {
        self.expanded = self.expanded.checked_add(n).ok_or("expanded overflow")?;
        if self.expanded > MAX_INPUT {
            Err("expanded budget".into())
        } else {
            Ok(())
        }
    }
    fn string(&mut self) -> Result<String, String> {
        self.string_limited(MAX_INPUT)
    }
    fn string_limited(&mut self, remaining: usize) -> Result<String, String> {
        let n = self.u32()?;
        if n > remaining {
            return Err("string budget".into());
        }
        self.charge(n)?;
        Ok(std::str::from_utf8(self.take(n)?)
            .map_err(|_| "utf8")?
            .to_owned())
    }
    fn value(&mut self, dict: &[String], depth: usize) -> Result<Value, String> {
        if depth > 24 || self.nodes == 0 {
            return Err("structure budget".into());
        }
        self.nodes -= 1;
        Ok(match self.u8()? {
            0 => Value::Null,
            1 => Value::Bool(false),
            2 => Value::Bool(true),
            11 => Value::Number(self.u8()?.into()),
            3 => Value::Number(u64::from_le_bytes(self.take(8)?.try_into().unwrap()).into()),
            4 => Value::Number(i64::from_le_bytes(self.take(8)?.try_into().unwrap()).into()),
            5 => Value::Number(
                Number::from_f64(f64::from_bits(u64::from_le_bytes(
                    self.take(8)?.try_into().unwrap(),
                )))
                .ok_or("nonfinite")?,
            ),
            10 => {
                let id = self.u8()?;
                let text = ENUMS.get(id as usize).ok_or("enum code")?;
                self.charge(text.len())?;
                Value::String(text.to_string())
            }
            6 => Value::String(self.string()?),
            7 => {
                let id = u16::from_le_bytes(self.take(2)?.try_into().unwrap()) as usize;
                let s = dict.get(id).ok_or("dictionary reference")?;
                self.charge(s.len())?;
                Value::String(s.clone())
            }
            8 => {
                let n = self.u32()?;
                if n > 4096 || n > self.nodes {
                    return Err("array count".into());
                }
                let mut a = Vec::new();
                for _ in 0..n {
                    a.push(self.value(dict, depth + 1)?);
                }
                Value::Array(a)
            }
            9 => {
                let n = self.u32()?;
                if n > 64 || n > self.nodes {
                    return Err("object count".into());
                }
                let mut m = Map::new();
                for _ in 0..n {
                    let id = self.u8()?;
                    let k = if id == 0 {
                        self.string()?
                    } else {
                        let k = KEYS.get(id as usize - 1).ok_or("field code")?;
                        self.charge(k.len())?;
                        k.to_string()
                    };
                    let v = self.value(dict, depth + 1)?;
                    if m.insert(k, v).is_some() {
                        return Err("duplicate field".into());
                    }
                }
                Value::Object(m)
            }
            _ => return Err("value code".into()),
        })
    }
}
fn decode_json_batch(bytes: &[u8]) -> Result<Vec<Observation>, String> {
    struct Batch;
    impl<'de> serde::de::Visitor<'de> for Batch {
        type Value = Vec<Observation>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("at most 32 observations")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut records = Vec::new();
            while records.len() < 32 {
                match seq.next_element()? {
                    Some(record) => records.push(record),
                    None => return Ok(records),
                }
            }
            // Skip an excess value without materializing another Observation.
            if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom("record count"));
            }
            Ok(records)
        }
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let records =
        serde::Deserializer::deserialize_seq(&mut decoder, Batch).map_err(|e| e.to_string())?;
    decoder.end().map_err(|e| e.to_string())?;
    Ok(records)
}
pub fn decode(bytes: &[u8]) -> Result<Vec<Observation>, String> {
    if bytes.len() > MAX_FRAME || !bytes.starts_with(MAGIC) {
        return Err("frame or schema".into());
    }
    let mut r = Reader {
        b: bytes,
        at: MAGIC.len(),
        expanded: 0,
        // Each value consumes at least one canonical JSON byte. This preserves
        // the entire valid batch domain while bounding malformed binary input.
        nodes: MAX_INPUT,
    };
    let codec = r.u8()?;
    let records = if codec == 0 {
        if bytes.len() - r.at > MAX_INPUT {
            return Err("json budget".into());
        }
        decode_json_batch(&bytes[r.at..])?
    } else {
        if codec != 1 && codec != 2 {
            return Err("codec".into());
        }
        let n = r.u32()?;
        if n > 1024 {
            return Err("dictionary count".into());
        }
        let mut dict = Vec::new();
        let start = r.expanded;
        for _ in 0..n {
            dict.push(r.string_limited(65536 - (r.expanded - start))?);
        }
        r.expanded = 0;
        let count = r.u32()?;
        if count == 0 || count > 32 {
            return Err("record count".into());
        }
        let mut values = Vec::new();
        if codec == 1 {
            for _ in 0..count {
                values.push(r.value(&dict, 0)?);
            }
        } else {
            values.resize_with(count, || Value::Object(Map::new()));
            for key in COLS {
                for v in &mut values {
                    v.as_object_mut()
                        .unwrap()
                        .insert(key.to_string(), r.value(&dict, 0)?);
                }
            }
        }
        if r.at != bytes.len() {
            return Err("trailing data".into());
        }
        values
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<Vec<Observation>, _>>()
            .map_err(|e| e.to_string())?
    };
    canonical(&records)?;
    Ok(records)
}
pub mod allocation;
/// Envelope metadata only; called after qualification, outside ingest timing.
pub fn dictionary_usage(frame: &[u8]) -> (usize, usize) {
    if frame[MAGIC.len()] != 2 {
        return (0, 0);
    }
    let mut at = MAGIC.len() + 1;
    let n = u32::from_le_bytes(frame[at..at + 4].try_into().unwrap()) as usize;
    at += 4;
    let mut bytes = 0;
    for _ in 0..n {
        let len = u32::from_le_bytes(frame[at..at + 4].try_into().unwrap()) as usize;
        at += 4 + len;
        bytes += len;
    }
    (n, bytes)
}

/// Validation token binds decoded values to the exact checked input frame.
pub struct ValidatedBatch<'a> {
    bytes: &'a [u8],
    records: Vec<Observation>,
}
impl ValidatedBatch<'_> {
    pub fn records(&self) -> &[Observation] {
        &self.records
    }
}
pub fn validate_frame(bytes: &[u8]) -> Result<ValidatedBatch<'_>, String> {
    Ok(ValidatedBatch {
        bytes,
        records: decode(bytes)?,
    })
}
