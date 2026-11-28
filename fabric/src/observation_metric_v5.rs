//! Explicit metric-only v5 decoding. This module validates v5 before admission.
use super::{Data, Identity, MAX_FRAME, MetricType, check_json, valid_identity_resource};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::collections::BTreeMap;

pub const VERSION_V5: u32 = 5;
pub const REGISTRY_V5: &str = "fabrico11y/5";
const RESERVED: &[&str] = &[
    "producer",
    "generation",
    "sequence",
    "metric_unit",
    "metric_type",
    "reset",
    "context_run_id",
    "version",
    "labels",
];

/// An explicit metric envelope validated by the versioned wire decoder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationV5 {
    pub version: u32,
    pub registry: String,
    pub context: crate::run::RunContext,
    pub identity: Identity,
    pub timestamp_ns: u64,
    pub resource: String,
    pub data: MetricV5,
}

/// Metric dimensions are nested separately from collector provenance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricV5 {
    pub kind: MetricKind,
    pub name: String,
    pub unit: String,
    pub value: f64,
    pub metric_type: MetricType,
    pub reset: bool,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "deserialize_labels"
    )]
    pub labels: BTreeMap<String, String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    Metric,
}

fn valid_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    key.len() <= 64
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        && !RESERVED.contains(&key)
}
fn deserialize_labels<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    use serde::de::{Error, MapAccess, Visitor};
    struct Labels;
    impl<'de> Visitor<'de> for Labels {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a bounded label object")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut labels = BTreeMap::new();
            while let Some(key) = map.next_key::<String>()? {
                if labels.len() >= 8 || !valid_key(&key) || labels.contains_key(&key) {
                    return Err(M::Error::custom("invalid or duplicate label key"));
                }
                let value: String = map.next_value()?;
                if !valid_value(&value) {
                    return Err(M::Error::custom("invalid label value"));
                }
                labels.insert(key, value);
            }
            Ok(labels)
        }
    }
    deserializer.deserialize_map(Labels)
}
fn valid_value(value: &str) -> bool {
    value.len() <= 128 && !value.chars().any(char::is_control)
}

impl ObservationV5 {
    /// Exact, versioned logical series identity; authority and lease ownership are external.
    /// Timestamp, value, reset, generation, sequence, and run context are not dimensions.
    pub fn series_key(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        fn field(out: &mut Vec<u8>, value: &str) -> Result<(), String> {
            let len = u32::try_from(value.len()).map_err(|_| "series field too large")?;
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(value.as_bytes());
            Ok(())
        }
        let mut key = b"fabrico11y.metric-series.v1\0".to_vec();
        for value in [
            &self.identity.producer,
            &self.resource,
            &self.data.name,
            &self.data.unit,
        ] {
            field(&mut key, value)?;
        }
        field(
            &mut key,
            match self.data.metric_type {
                MetricType::Gauge => "gauge",
                MetricType::Counter => "counter",
            },
        )?;
        let count = u32::try_from(self.data.labels.len()).map_err(|_| "too many labels")?;
        key.extend_from_slice(&count.to_be_bytes());
        for (name, value) in &self.data.labels {
            field(&mut key, name)?;
            field(&mut key, value)?;
        }
        if key.len() > 4096 {
            return Err("series key too large".into());
        }
        Ok(key)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != VERSION_V5 || self.registry != REGISTRY_V5 {
            return Err("invalid v5 envelope".into());
        }
        self.context.validate()?;
        if !valid_identity_resource(&self.identity, &self.resource) {
            return Err("invalid v5 identity or resource".into());
        }
        if self.data.labels.len() > 8
            || !self
                .data
                .labels
                .iter()
                .all(|(k, v)| valid_key(k) && valid_value(v))
        {
            return Err("invalid metric labels".into());
        }
        let metric = Data::Metric {
            name: self.data.name.clone(),
            unit: self.data.unit.clone(),
            value: self.data.value,
            metric_type: self.data.metric_type,
            reset: self.data.reset,
        };
        if !metric.validate() {
            return Err("invalid metric fields".into());
        }
        Ok(())
    }
}

// Read only JSON string tokens, limiting their encoded length before allocating.
// Six encoded bytes per decoded byte covers JSON escapes, including surrogate pairs.
fn token<'a>(input: &'a str, pos: &mut usize, max: usize) -> Result<&'a str, String> {
    let bytes = input.as_bytes();
    if bytes.get(*pos) != Some(&b'"') {
        return Err("label must be a string".into());
    }
    let start = *pos;
    *pos += 1;
    let mut escaped = false;
    while let Some(&b) = bytes.get(*pos) {
        *pos += 1;
        if *pos - start > max * 6 + 2 {
            return Err("label token too large".into());
        }
        if escaped {
            escaped = false;
        } else if b == b'\\' {
            escaped = true;
        } else if b == b'"' {
            return Ok(&input[start..*pos]);
        }
    }
    Err("unterminated label string".into())
}
fn whitespace(bytes: &[u8], pos: &mut usize) {
    while bytes.get(*pos).is_some_and(u8::is_ascii_whitespace) {
        *pos += 1;
    }
}
fn validate_labels_raw(raw: &RawValue) -> Result<(), String> {
    let s = raw.get();
    let b = s.as_bytes();
    let mut p = 0;
    let mut seen = BTreeMap::new();
    whitespace(b, &mut p);
    if b.get(p) != Some(&b'{') {
        return Err("labels must be an object".into());
    }
    p += 1;
    loop {
        whitespace(b, &mut p);
        if b.get(p) == Some(&b'}') {
            p += 1;
            break;
        }
        if seen.len() == 8 {
            return Err("too many labels".into());
        }
        let key: String = serde_json::from_str(token(s, &mut p, 64)?).map_err(|e| e.to_string())?;
        if !valid_key(&key) || seen.contains_key(&key) {
            return Err("invalid or duplicate label key".into());
        }
        whitespace(b, &mut p);
        if b.get(p) != Some(&b':') {
            return Err("missing label colon".into());
        }
        p += 1;
        whitespace(b, &mut p);
        let value: String =
            serde_json::from_str(token(s, &mut p, 128)?).map_err(|e| e.to_string())?;
        if !valid_value(&value) {
            return Err("invalid label value".into());
        }
        seen.insert(key, ());
        whitespace(b, &mut p);
        match b.get(p) {
            Some(b',') => {
                p += 1;
                whitespace(b, &mut p);
                if b.get(p) == Some(&b'}') {
                    return Err("trailing label comma".into());
                }
            }
            Some(b'}') => {
                p += 1;
                break;
            }
            _ => return Err("invalid label object".into()),
        }
    }
    whitespace(b, &mut p);
    if p != b.len() {
        return Err("invalid label object".into());
    }
    Ok(())
}

/// Decode one bounded v5 metric without modifying its input bytes or changing its raw spelling.
pub fn decode_metric_v5(bytes: &[u8]) -> Result<ObservationV5, String> {
    check_json(bytes, MAX_FRAME)?;
    #[derive(Deserialize)]
    struct RawEnvelope<'a> {
        #[serde(borrow)]
        data: RawData<'a>,
    }
    #[derive(Deserialize)]
    struct RawData<'a> {
        #[serde(default, borrow)]
        labels: Option<&'a RawValue>,
    }
    let raw: RawEnvelope<'_> = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if let Some(labels) = raw.data.labels {
        validate_labels_raw(labels)?;
    }
    let value: ObservationV5 = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    value.validate()?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(labels: &str) -> Vec<u8> {
        format!(r#"{{"version":5,"registry":"fabrico11y/5","context":{{"run_id":"00000000-0000-4000-8000-000000000001"}},"identity":{{"producer":"p","generation":"g","sequence":1}},"timestamp_ns":1,"resource":"r","data":{{"kind":"metric","name":"m","unit":"s","value":1.0,"metric_type":"gauge","reset":false{labels}}}}}"#).into_bytes()
    }
    #[test]
    fn pinned_registry_fixtures_use_exact_wire_decoders() {
        #[derive(serde::Deserialize)]
        struct Fixture<'a> {
            name: String,
            valid: bool,
            #[serde(default, borrow)]
            wire: Option<&'a RawValue>,
            #[serde(default)]
            wire_json: Option<String>,
        }
        let fixtures: Vec<Fixture<'_>> =
            serde_json::from_str(include_str!("../../registry/v5/fixtures.json")).unwrap();
        assert_eq!(fixtures.len(), 20);
        let mut names = std::collections::BTreeSet::new();
        for fixture in fixtures {
            assert!(
                names.insert(fixture.name.clone()),
                "duplicate fixture {}",
                fixture.name
            );
            let bytes = match (&fixture.wire, &fixture.wire_json) {
                (Some(raw), None) => raw.get().as_bytes(),
                (None, Some(raw)) => raw.as_bytes(),
                _ => panic!("{}: expected exactly one wire representation", fixture.name),
            };
            let actual = if fixture.name.starts_with("v3-") {
                super::super::decode_versioned(bytes).map(|_| ())
            } else {
                let active = super::super::decode_versioned(bytes);
                assert_eq!(
                    active.is_ok(),
                    fixture.valid,
                    "active v5 fixture {}",
                    fixture.name
                );
                decode_metric_v5(bytes).map(|_| ())
            };
            assert_eq!(
                actual.is_ok(),
                fixture.valid,
                "fixture {}: {:?}",
                fixture.name,
                actual
            );
        }
    }
    #[test]
    fn bounded_labels_and_legacy_decoder_separation() {
        for labels in [
            "",
            ",\"labels\":{}",
            ",\"labels\":{\"stage\":\"α\",\"_empty\":\"\"}",
        ] {
            let bytes = frame(labels);
            let original = bytes.clone();
            let parsed = decode_metric_v5(&bytes).unwrap();
            assert_eq!(parsed.version, 5);
            assert_eq!(bytes, original);
            assert!(super::super::decode_versioned(&bytes).is_ok());
        }
        let key = format!("a{}", "x".repeat(63));
        let labels = format!(",\"labels\":{{\"{key}\":\"{}\"}}", "é".repeat(64));
        assert!(decode_metric_v5(&frame(&labels)).is_ok());
        let eight = (0..8)
            .map(|i| format!("\"k{i}\":\"v\""))
            .collect::<Vec<_>>()
            .join(",");
        assert!(decode_metric_v5(&frame(&format!(",\"labels\":{{{eight}}}"))).is_ok());
    }
    #[test]
    fn legacy_metric_wire_remains_strict_and_unchanged() {
        let wire = br#"{"version":2,"registry":"fabrico11y/2","identity":{"producer":"p","generation":"g","sequence":1},"timestamp_ns":1,"resource":"r","data":{"kind":"metric","name":"m","unit":"s","value":1.0,"metric_type":"gauge","reset":false}}"#;
        assert!(super::super::decode(wire).is_ok());
        let extra = String::from_utf8(wire.to_vec())
            .unwrap()
            .replace("\"reset\":false", "\"reset\":false,\"labels\":{}");
        assert!(super::super::decode(extra.as_bytes()).is_err());
        let nonmetric = String::from_utf8(frame(""))
            .unwrap()
            .replace("\"kind\":\"metric\"", "\"kind\":\"event\"");
        assert!(decode_metric_v5(nonmetric.as_bytes()).is_err());
    }
    #[test]
    fn logical_series_key_is_stable_and_dimension_sensitive() {
        let a = decode_metric_v5(&frame(r#","labels":{"x":"one","y":"two"}"#)).unwrap();
        let mut b = decode_metric_v5(&frame(r#","labels":{"y":"two","x":"one"}"#)).unwrap();
        b.timestamp_ns = 99;
        b.identity.sequence = 44;
        b.identity.generation = "other".into();
        b.context.run_id = "00000000-0000-4000-8000-000000000002".into();
        b.data.value = 4.0;
        b.data.reset = true;
        let key = a.series_key().unwrap();
        assert_eq!(key, b.series_key().unwrap());
        let mut variants = Vec::new();
        let mut v = a.clone();
        v.identity.producer = "q".into();
        variants.push(v);
        let mut v = a.clone();
        v.resource = "other".into();
        variants.push(v);
        let mut v = a.clone();
        v.data.name = "other".into();
        variants.push(v);
        let mut v = a.clone();
        v.data.unit = "other".into();
        variants.push(v);
        let mut v = a.clone();
        v.data.metric_type = MetricType::Counter;
        variants.push(v);
        let mut v = a.clone();
        v.data.labels.insert("x".into(), "other".into());
        variants.push(v);
        let mut v = a.clone();
        v.data.labels.remove("x");
        v.data.labels.insert("z".into(), "one".into());
        variants.push(v);
        for v in variants {
            assert_ne!(key, v.series_key().unwrap());
        }
        let mut left = a.clone();
        left.identity.producer = "ab".into();
        left.resource = "c".into();
        let mut right = a.clone();
        right.identity.producer = "a".into();
        right.resource = "bc".into();
        assert_ne!(left.series_key().unwrap(), right.series_key().unwrap());
        let mut invalid = a;
        invalid.data.labels.insert("producer".into(), "x".into());
        assert!(invalid.series_key().is_err());
    }
    #[test]
    fn invalid_label_shapes_limits_and_duplicates_fail() {
        let nine = (0..9)
            .map(|i| format!("\"k{i}\":\"v\""))
            .collect::<Vec<_>>()
            .join(",");
        let bad = vec![
            format!(",\"labels\":{{{nine}}}"),
            format!(",\"labels\":{{\"{}\":\"v\"}}", "a".repeat(65)),
            format!(",\"labels\":{{\"k\":\"{}\"}}", "x".repeat(129)),
            format!(",\"labels\":{{\"k\":\"{}\"}}", "\\u0061".repeat(129)),
            ",\"labels\":{\"a\":\"1\",\"\\u0061\":\"2\"}".into(),
            ",\"labels\":{\"producer\":\"bad\"}".into(),
            ",\"labels\":{\"a/b\":\"bad\"}".into(),
            ",\"labels\":{\"k\":\"\\u0000\"}".into(),
            ",\"labels\":{\"k\":\"\\u0085\"}".into(),
            ",\"labels\":{\"k\":1}".into(),
            ",\"labels\":null".into(),
            ",\"labels\":{\"k\":\"v\",}".into(),
        ];
        for labels in bad {
            assert!(decode_metric_v5(&frame(&labels)).is_err(), "{labels}");
        }
        for key in RESERVED {
            assert!(decode_metric_v5(&frame(&format!(",\"labels\":{{\"{key}\":\"x\"}}"))).is_err());
        }
        assert!(decode_metric_v5(&frame(",\"labels\":{},\"labels\":{}")).is_err());
    }
}
