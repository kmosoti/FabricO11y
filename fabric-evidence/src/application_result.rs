//! Bounded application-result contract, independent of custody and transport.
//!
//! `fabric.application-json.v1` is compact UTF-8 JSON with lexicographically
//! sorted UTF-8 object keys, decimal integers, and serde_json string escaping.
//! It does not normalize Unicode and is not RFC 8785/JCS. Floats, exponents,
//! negative zero and integers outside i64/u64 are rejected, as are duplicate
//! decoded keys. The source bytes and application payload have distinct digests;
//! neither digest names the enclosing v3 custody frame.
use crate::evidence_manifest::{
    DigestAlgorithm, DigestClaim, DigestRepresentation, OriginalArtifactDigest, ProvenancedTime,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const APPLICATION_RESULT_VERSION: u16 = 1;
pub const APPLICATION_JSON_PROFILE: &str = "fabric.application-json.v1";
pub const MAX_APPLICATION_BYTES: usize = 4096;
pub const MAX_RESULT_WIRE_BYTES: usize = 16 * 1024;
pub const MAX_APPLICATION_DEPTH: usize = 8;
pub const MAX_APPLICATION_VALUES: usize = 1024;
const MAX_CONTAINER: usize = 64;
const MAX_KEY_BYTES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplicationResultError {
    Size,
    InvalidJson,
    DuplicateKey,
    Bounds,
    InvalidId,
    InvalidSchema,
    InvalidTime,
    InvalidDigest,
    UnsupportedVersion,
}

/// Scope is an identifier, not authentication or authorization evidence.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReportScope {
    origin: String,
    producer: String,
}
impl ReportScope {
    pub fn new(origin: String, producer: String) -> Result<Self, ApplicationResultError> {
        let scope = Self { origin, producer };
        scope.validate()?;
        Ok(scope)
    }
    fn validate(&self) -> Result<(), ApplicationResultError> {
        if !bounded_text(&self.origin, 128) || !bounded_text(&self.producer, 128) {
            return Err(ApplicationResultError::InvalidId);
        }
        Ok(())
    }
    pub fn origin(&self) -> &str {
        &self.origin
    }
    pub fn producer(&self) -> &str {
        &self.producer
    }
}

/// SHA-256 of `fabric.application-json.v1` payload bytes, not the custody frame.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct ApplicationPayloadDigest(pub DigestClaim);
impl ApplicationPayloadDigest {
    pub fn verify_exact_bytes(&self, bytes: &[u8]) -> Result<(), ApplicationResultError> {
        if bytes.len() > MAX_APPLICATION_BYTES {
            return Err(ApplicationResultError::Size);
        }
        verify_claim(&self.0, bytes)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub(crate) enum Json {
    Null(()),
    Bool(bool),
    I64(i64),
    U64(u64),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

/// Immutable validated result. Parent UUID is interpreted only within `scope`.
#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationResult {
    version: u16,
    scope: ReportScope,
    report_id: String,
    parent_report_id: Option<String>,
    schema: String,
    producer_completion: ProvenancedTime,
    payload_profile: String,
    payload: Json,
    original_artifact_digest: OriginalArtifactDigest,
    application_payload_digest: ApplicationPayloadDigest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    version: u16,
    scope: ReportScope,
    report_id: String,
    parent_report_id: Option<String>,
    schema: String,
    producer_completion: ProvenancedTime,
    payload_profile: String,
    payload: serde_json::Value,
    original_artifact_digest: OriginalArtifactDigest,
    application_payload_digest: ApplicationPayloadDigest,
}

impl ApplicationResult {
    /// `artifact` is the original JSON source, including all whitespace/newlines.
    pub fn new(
        scope: ReportScope,
        report_id: String,
        parent_report_id: Option<String>,
        schema: String,
        producer_completion: ProvenancedTime,
        artifact: &[u8],
    ) -> Result<Self, ApplicationResultError> {
        let payload = parse(
            artifact,
            MAX_APPLICATION_BYTES,
            MAX_APPLICATION_DEPTH,
            MAX_APPLICATION_VALUES,
        )?;
        let canonical = encode_payload(&payload)?;
        let result = Self {
            version: APPLICATION_RESULT_VERSION,
            scope,
            report_id,
            parent_report_id,
            schema,
            producer_completion,
            payload_profile: APPLICATION_JSON_PROFILE.into(),
            payload,
            original_artifact_digest: OriginalArtifactDigest(claim(artifact)),
            application_payload_digest: ApplicationPayloadDigest(claim(&canonical)),
        };
        result.validate()?;
        result.encode_bounded()?;
        Ok(result)
    }
    /// Decode only after a bounded strict JSON scan; source digest remains a claim
    /// until `verify_original_artifact` receives the original bytes.
    pub fn decode_bounded(bytes: &[u8]) -> Result<Self, ApplicationResultError> {
        parse(
            bytes,
            MAX_RESULT_WIRE_BYTES,
            MAX_APPLICATION_DEPTH + 4,
            MAX_APPLICATION_VALUES + 64,
        )?;
        let wire: Wire =
            serde_json::from_slice(bytes).map_err(|_| ApplicationResultError::InvalidJson)?;
        let payload_bytes =
            serde_json::to_vec(&wire.payload).map_err(|_| ApplicationResultError::InvalidJson)?;
        let payload = parse(
            &payload_bytes,
            MAX_APPLICATION_BYTES,
            MAX_APPLICATION_DEPTH,
            MAX_APPLICATION_VALUES,
        )?;
        let result = Self {
            version: wire.version,
            scope: wire.scope,
            report_id: wire.report_id,
            parent_report_id: wire.parent_report_id,
            schema: wire.schema,
            producer_completion: wire.producer_completion,
            payload_profile: wire.payload_profile,
            payload,
            original_artifact_digest: wire.original_artifact_digest,
            application_payload_digest: wire.application_payload_digest,
        };
        result.validate()?;
        result.encode_bounded()?;
        Ok(result)
    }
    pub fn encode_bounded(&self) -> Result<Vec<u8>, ApplicationResultError> {
        let bytes = serde_json::to_vec(self).map_err(|_| ApplicationResultError::InvalidJson)?;
        if bytes.len() > MAX_RESULT_WIRE_BYTES {
            return Err(ApplicationResultError::Size);
        }
        Ok(bytes)
    }
    pub fn canonical_payload(&self) -> Result<Vec<u8>, ApplicationResultError> {
        encode_payload(&self.payload)
    }
    pub fn original_artifact_digest(&self) -> &OriginalArtifactDigest {
        &self.original_artifact_digest
    }
    pub fn application_payload_digest(&self) -> &ApplicationPayloadDigest {
        &self.application_payload_digest
    }
    pub fn scope(&self) -> &ReportScope {
        &self.scope
    }
    pub fn report_id(&self) -> &str {
        &self.report_id
    }
    pub fn parent_report_id(&self) -> Option<&str> {
        self.parent_report_id.as_deref()
    }
    pub fn schema(&self) -> &str {
        &self.schema
    }
    pub fn producer_completion(&self) -> &ProvenancedTime {
        &self.producer_completion
    }
    pub fn verify_original_artifact(&self, bytes: &[u8]) -> Result<(), ApplicationResultError> {
        if bytes.len() > MAX_APPLICATION_BYTES {
            return Err(ApplicationResultError::Size);
        }
        self.original_artifact_digest
            .verify_exact_bytes(bytes)
            .map_err(|_| ApplicationResultError::InvalidDigest)?;
        let source = parse(
            bytes,
            MAX_APPLICATION_BYTES,
            MAX_APPLICATION_DEPTH,
            MAX_APPLICATION_VALUES,
        )?;
        if encode_payload(&source)? != self.canonical_payload()? {
            return Err(ApplicationResultError::InvalidDigest);
        }
        Ok(())
    }
    fn validate(&self) -> Result<(), ApplicationResultError> {
        if self.version != APPLICATION_RESULT_VERSION
            || self.payload_profile != APPLICATION_JSON_PROFILE
        {
            return Err(ApplicationResultError::UnsupportedVersion);
        }
        self.scope.validate()?;
        if !uuid(&self.report_id)
            || self
                .parent_report_id
                .as_ref()
                .is_some_and(|p| !uuid(p) || p == &self.report_id)
        {
            return Err(ApplicationResultError::InvalidId);
        }
        validate_schema(&self.schema, &self.payload)?;
        if !bounded_text(&self.producer_completion.clock.clock_id, 128)
            || !bounded_text(&self.producer_completion.clock.source, 256)
        {
            return Err(ApplicationResultError::InvalidTime);
        }
        let source = &self.original_artifact_digest.0;
        if source.validate().is_err() || source.representation != DigestRepresentation::ExactBytes {
            return Err(ApplicationResultError::InvalidDigest);
        }
        self.application_payload_digest
            .verify_exact_bytes(&encode_payload(&self.payload)?)?;
        Ok(())
    }
}
fn bounded_text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.contains('\0')
}
pub(crate) fn uuid(s: &str) -> bool {
    s.len() == 36
        && s != "00000000-0000-0000-0000-000000000000"
        && s.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
}
fn validate_schema(schema: &str, payload: &Json) -> Result<(), ApplicationResultError> {
    let Json::Object(map) = payload else {
        return Err(ApplicationResultError::InvalidSchema);
    };
    let Some((prefix, version)) = schema.rsplit_once(".v") else {
        return Err(ApplicationResultError::InvalidSchema);
    };
    if schema.len() > 128
        || prefix.is_empty()
        || !prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        || version.is_empty()
        || version.starts_with('0')
        || !version.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(ApplicationResultError::InvalidSchema);
    }
    if let Some(value) = map.get("schema")
        && !matches!(value, Json::String(s) if s == schema)
    {
        return Err(ApplicationResultError::InvalidSchema);
    }
    Ok(())
}

fn encode_payload(v: &Json) -> Result<Vec<u8>, ApplicationResultError> {
    let bytes = serde_json::to_vec(v).map_err(|_| ApplicationResultError::InvalidJson)?;
    if bytes.len() > MAX_APPLICATION_BYTES {
        return Err(ApplicationResultError::Size);
    }
    Ok(bytes)
}
fn claim(bytes: &[u8]) -> DigestClaim {
    let mut hex = String::with_capacity(64);
    const DIGITS: &[u8] = b"0123456789abcdef";
    for b in Sha256::digest(bytes) {
        hex.push(DIGITS[(b >> 4) as usize] as char);
        hex.push(DIGITS[(b & 15) as usize] as char);
    }
    DigestClaim {
        algorithm: DigestAlgorithm::Sha256,
        representation: DigestRepresentation::ExactBytes,
        hex,
    }
}
fn verify_claim(c: &DigestClaim, bytes: &[u8]) -> Result<(), ApplicationResultError> {
    if c.validate().is_err() || c != &claim(bytes) {
        return Err(ApplicationResultError::InvalidDigest);
    }
    Ok(())
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    nodes: usize,
    max_depth: usize,
    max_nodes: usize,
}
pub(crate) fn parse(
    bytes: &[u8],
    max_bytes: usize,
    max_depth: usize,
    max_nodes: usize,
) -> Result<Json, ApplicationResultError> {
    if bytes.len() > max_bytes {
        return Err(ApplicationResultError::Size);
    }
    let mut p = Parser {
        bytes,
        pos: 0,
        nodes: 0,
        max_depth,
        max_nodes,
    };
    let value = p.value(0)?;
    p.ws();
    if p.pos != bytes.len() {
        return Err(ApplicationResultError::InvalidJson);
    }
    Ok(value)
}
impl Parser<'_> {
    fn ws(&mut self) {
        while self
            .bytes
            .get(self.pos)
            .is_some_and(|b| matches!(b, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.pos += 1;
        }
    }
    fn take(&mut self, b: u8) -> bool {
        self.ws();
        if self.bytes.get(self.pos) == Some(&b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn value(&mut self, depth: usize) -> Result<Json, ApplicationResultError> {
        self.ws();
        if depth > self.max_depth || self.nodes >= self.max_nodes {
            return Err(ApplicationResultError::Bounds);
        }
        self.nodes += 1;
        match self.bytes.get(self.pos).copied() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Json::String(self.string()?)),
            Some(b't') if self.literal(b"true") => Ok(Json::Bool(true)),
            Some(b'f') if self.literal(b"false") => Ok(Json::Bool(false)),
            Some(b'n') if self.literal(b"null") => Ok(Json::Null(())),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(ApplicationResultError::InvalidJson),
        }
    }
    fn literal(&mut self, s: &[u8]) -> bool {
        if self.bytes.get(self.pos..self.pos + s.len()) == Some(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }
    fn string(&mut self) -> Result<String, ApplicationResultError> {
        let start = self.pos;
        self.pos += 1;
        let mut escaped = false;
        while let Some(&b) = self.bytes.get(self.pos) {
            self.pos += 1;
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                return serde_json::from_slice(&self.bytes[start..self.pos])
                    .map_err(|_| ApplicationResultError::InvalidJson);
            }
        }
        Err(ApplicationResultError::InvalidJson)
    }
    fn number(&mut self) -> Result<Json, ApplicationResultError> {
        let start = self.pos;
        if self.bytes[self.pos] == b'-' {
            self.pos += 1;
        }
        let digits = self.pos;
        while self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
            self.pos += 1;
        }
        let raw = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| ApplicationResultError::InvalidJson)?;
        let digits_len = self.pos - digits;
        if digits_len == 0 || (digits_len > 1 && self.bytes[digits] == b'0') || raw == "-0" {
            return Err(ApplicationResultError::InvalidJson);
        }
        if raw.starts_with('-') {
            raw.parse::<i64>()
                .map(Json::I64)
                .map_err(|_| ApplicationResultError::InvalidJson)
        } else {
            raw.parse::<u64>()
                .map(Json::U64)
                .map_err(|_| ApplicationResultError::InvalidJson)
        }
    }
    fn array(&mut self, depth: usize) -> Result<Json, ApplicationResultError> {
        self.pos += 1;
        let mut values = Vec::new();
        if self.take(b']') {
            return Ok(Json::Array(values));
        }
        loop {
            if values.len() >= MAX_CONTAINER {
                return Err(ApplicationResultError::Bounds);
            }
            values.push(self.value(depth + 1)?);
            if self.take(b']') {
                break;
            }
            if !self.take(b',') {
                return Err(ApplicationResultError::InvalidJson);
            }
        }
        Ok(Json::Array(values))
    }
    fn object(&mut self, depth: usize) -> Result<Json, ApplicationResultError> {
        self.pos += 1;
        let mut values = BTreeMap::new();
        if self.take(b'}') {
            return Ok(Json::Object(values));
        }
        loop {
            self.ws();
            if self.bytes.get(self.pos) != Some(&b'"') {
                return Err(ApplicationResultError::InvalidJson);
            }
            let key = self.string()?;
            if key.len() > MAX_KEY_BYTES || values.len() >= MAX_CONTAINER {
                return Err(ApplicationResultError::Bounds);
            }
            if values.contains_key(&key) {
                return Err(ApplicationResultError::DuplicateKey);
            }
            if !self.take(b':') {
                return Err(ApplicationResultError::InvalidJson);
            }
            values.insert(key, self.value(depth + 1)?);
            if self.take(b'}') {
                break;
            }
            if !self.take(b',') {
                return Err(ApplicationResultError::InvalidJson);
            }
        }
        Ok(Json::Object(values))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence_manifest::ClockProvenance;

    const FIRST: &str = "aaaaaaaa-0000-4000-8000-000000000001";
    const SECOND: &str = "aaaaaaaa-0000-4000-8000-000000000002";
    fn time() -> ProvenancedTime {
        ProvenancedTime {
            unix_ms: 42,
            clock: ClockProvenance {
                clock_id: "producer-clock".into(),
                source: "worker".into(),
                uncertainty_ms: Some(3),
            },
        }
    }
    fn report(
        id: &str,
        parent: Option<&str>,
        source: &[u8],
    ) -> Result<ApplicationResult, ApplicationResultError> {
        ApplicationResult::new(
            ReportScope::new("origin".into(), "producer".into()).unwrap(),
            id.into(),
            parent.map(str::to_owned),
            "example.result.v1".into(),
            time(),
            source,
        )
    }

    #[test]
    fn source_and_canonical_application_payload_have_independent_known_digests() {
        let source = b"{\"label\":\"caf\\u00e9\"}\n";
        let result = report(FIRST, None, source).unwrap();
        let canonical = "{\"label\":\"café\"}".as_bytes();
        assert_eq!(result.canonical_payload().unwrap(), canonical);
        assert_eq!(
            result.original_artifact_digest().0.hex,
            "426d8f9948cd8aa9aede8235c65e8f5fd79eb048055e962afbea5c2b5ca1b64d"
        );
        assert_eq!(
            result.application_payload_digest().0.hex,
            "2d7fba14f0a7cffed454bc268d791c0f32d11f62946998a323087ccf5df29075"
        );
        result.verify_original_artifact(source).unwrap();
        assert_eq!(
            result.verify_original_artifact(canonical),
            Err(ApplicationResultError::InvalidDigest)
        );
        result
            .application_payload_digest()
            .verify_exact_bytes(canonical)
            .unwrap();
        assert_eq!(
            result
                .application_payload_digest()
                .verify_exact_bytes(source),
            Err(ApplicationResultError::InvalidDigest)
        );
        let wire = result.encode_bounded().unwrap();
        let decoded = ApplicationResult::decode_bounded(&wire).unwrap();
        assert_eq!(decoded.canonical_payload().unwrap(), canonical);
        assert_eq!(
            decoded.original_artifact_digest(),
            result.original_artifact_digest()
        );
        assert_eq!(decoded.producer_completion().clock.source, "worker");
        let other = br#"{"label":"other"}"#;
        let mut forged: serde_json::Value = serde_json::from_slice(&wire).unwrap();
        forged["original_artifact_digest"]["hex"] = claim(other).hex.into();
        let forged =
            ApplicationResult::decode_bounded(&serde_json::to_vec(&forged).unwrap()).unwrap();
        assert_eq!(
            forged.verify_original_artifact(other),
            Err(ApplicationResultError::InvalidDigest)
        );
        let mut tampered: serde_json::Value = serde_json::from_slice(&wire).unwrap();
        tampered["payload"]["label"] = "changed".into();
        assert!(matches!(
            ApplicationResult::decode_bounded(&serde_json::to_vec(&tampered).unwrap()),
            Err(ApplicationResultError::InvalidDigest)
        ));
    }

    #[test]
    fn strict_integer_and_decoded_key_profile() {
        for bad in [
            r#"{"x":1,"\u0078":2}"#,
            r#"{"x":1.0}"#,
            r#"{"x":1e0}"#,
            r#"{"x":-0}"#,
            r#"{"x":18446744073709551616}"#,
            r#"{"x":-9223372036854775809}"#,
            r#"{"x":01}"#,
            r#"{"x":true,}"#,
        ] {
            assert!(
                report(FIRST, None, bad.as_bytes()).is_err(),
                "accepted {bad}"
            );
        }
        assert!(matches!(
            report(FIRST, None, br#"{"x":1,"\u0078":2}"#),
            Err(ApplicationResultError::DuplicateKey)
        ));
        let valid = br#"{"z":18446744073709551615,"a":-9223372036854775808}"#;
        assert_eq!(
            report(FIRST, None, valid)
                .unwrap()
                .canonical_payload()
                .unwrap(),
            br#"{"a":-9223372036854775808,"z":18446744073709551615}"#
        );
    }

    #[test]
    fn scoped_primary_and_supplement_preserve_fixture_content() {
        let first = br#"{"schema":"example.result.v1","record_id":"fixture.primary","enrolled":8,"valid":4,"exact":3,"unavailable":4}"#;
        let second = br#"{"schema":"example.result.v1","record_id":"fixture.supplementary","parent_report_id":"aaaaaaaa-0000-4000-8000-000000000001","relation":"supplements","enrolled":8,"valid":7,"exact":5,"unavailable":1}"#;
        let primary = report(FIRST, None, first).unwrap();
        let supplement = report(SECOND, Some(FIRST), second).unwrap();
        assert_eq!(supplement.parent_report_id(), Some(primary.report_id()));
        assert_eq!(supplement.scope(), primary.scope());
        let supplementary: serde_json::Value =
            serde_json::from_slice(&supplement.canonical_payload().unwrap()).unwrap();
        assert_eq!(supplementary["parent_report_id"], primary.report_id());
        for (r, valid, exact, unavailable) in [(&primary, 4, 3, 4), (&supplement, 7, 5, 1)] {
            let p: serde_json::Value =
                serde_json::from_slice(&r.canonical_payload().unwrap()).unwrap();
            assert_eq!(p["enrolled"], 8);
            assert_eq!(p["valid"], valid);
            assert_eq!(p["exact"], exact);
            assert_eq!(p["unavailable"], unavailable);
            let result = ApplicationResult::decode_bounded(&r.encode_bounded().unwrap()).unwrap();
            assert_eq!(
                result.application_payload_digest(),
                r.application_payload_digest()
            );
        }
        assert!(matches!(
            report(FIRST, Some(FIRST), first),
            Err(ApplicationResultError::InvalidId)
        ));
        assert!(matches!(
            report("00000000-0000-0000-0000-000000000000", None, first),
            Err(ApplicationResultError::InvalidId)
        ));
        assert!(matches!(
            report(FIRST, None, br#"{"schema":"other.v1"}"#),
            Err(ApplicationResultError::InvalidSchema)
        ));
    }

    #[test]
    fn bounds_are_enforced_before_acceptance() {
        assert!(matches!(
            report(FIRST, None, &vec![b' '; MAX_APPLICATION_BYTES + 1]),
            Err(ApplicationResultError::Size)
        ));
        let wide = format!("{{\"x\":[{}]}}", vec!["0"; 65].join(","));
        assert!(matches!(
            report(FIRST, None, wide.as_bytes()),
            Err(ApplicationResultError::Bounds)
        ));
        let deep = format!(
            "{{\"x\":{}0{}}}",
            "[".repeat(MAX_APPLICATION_DEPTH),
            "]".repeat(MAX_APPLICATION_DEPTH)
        );
        assert!(matches!(
            report(FIRST, None, deep.as_bytes()),
            Err(ApplicationResultError::Bounds)
        ));
        let long_key = format!("{{\"{}\":0}}", "x".repeat(MAX_KEY_BYTES + 1));
        assert!(matches!(
            report(FIRST, None, long_key.as_bytes()),
            Err(ApplicationResultError::Bounds)
        ));
        assert!(matches!(
            ApplicationResult::decode_bounded(&vec![b' '; MAX_RESULT_WIRE_BYTES + 1]),
            Err(ApplicationResultError::Size)
        ));
        let result = report(FIRST, None, br#"{}"#).unwrap();
        assert_eq!(
            result
                .application_payload_digest()
                .verify_exact_bytes(&vec![b' '; MAX_APPLICATION_BYTES + 1]),
            Err(ApplicationResultError::Size)
        );
        let bad_scope: ReportScope = serde_json::from_str(&format!(
            r#"{{"origin":"{}","producer":"p"}}"#,
            "a".repeat(129)
        ))
        .unwrap();
        assert!(matches!(
            ApplicationResult::new(
                bad_scope,
                FIRST.into(),
                None,
                "example.result.v1".into(),
                time(),
                br#"{}"#
            ),
            Err(ApplicationResultError::InvalidId)
        ));
    }
}
