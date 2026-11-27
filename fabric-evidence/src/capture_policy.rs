//! Bounded, default-deny transformation before any durable application capture.
//!
//! This pure utility does not configure or enforce any ingress by itself.
use crate::{
    application_result::{
        self, ApplicationResultError, Json, MAX_APPLICATION_BYTES, MAX_APPLICATION_DEPTH,
        MAX_APPLICATION_VALUES,
    },
    evidence_manifest::{DigestAlgorithm, DigestClaim, DigestRepresentation},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const CAPTURE_POLICY_VERSION: u16 = 1;
pub const CAPTURE_POLICY_PATH_VERSION: u16 = 2;
pub const MAX_CAPTURE_PATH_DEPTH: usize = 8;
pub const MAX_CAPTURE_PATH_BYTES: usize = 8192;
pub const MAX_CAPTURE_ARRAY_ITEMS: usize = 32;
pub const MAX_CAPTURE_PATH_WORK: usize = 8 * 1024 * 1024;
const MAX_POLICY_BYTES: usize = 16_384;
pub const MAX_CAPTURE_RULES: usize = 64;
pub const MAX_CAPTURE_NAME_BYTES: usize = 128;
pub const MAX_CAPTURE_TEXT_BYTES: usize = 512;
pub const MAX_CAPTURE_WORK: usize = 65_536;

/// Version 1 only captures explicitly permitted top-level scalars.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScalarType {
    Null,
    Boolean,
    Integer,
    Text,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldRule {
    pub name: String,
    pub scalar_type: ScalarType,
}

/// Each segment is an exact object key; no wildcard or subtree grant exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathRule {
    pub path: Vec<String>,
    pub action: CaptureAction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureAction {
    Scalar {
        scalar_type: ScalarType,
    },
    ScalarArray {
        scalar_type: ScalarType,
        max_items: u8,
    },
    Redact,
}

/// Output for a selected redaction; never derived from the input value.
pub const REDACTED_VALUE: &str = "[REDACTED]";

/// Configuration must be established by a trusted caller, not an input record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapturePolicy {
    version: u16,
    policy_id: String,
    max_input_bytes: usize,
    max_work: usize,
    rules: Vec<FieldRule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    paths: Option<Vec<PathRule>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureError {
    InvalidPolicy,
    InputSize,
    WorkLimit,
    InvalidInput,
    CapturedSize,
}

/// Fixed classes reveal neither omitted names, values, counts nor exact lengths.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Omission {
    PolicyExcluded,
    TypeNotPermitted,
    ValueLimit,
    Redacted,
}

/// SHA-256 of canonical policy bytes; this role is not an artifact digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapturePolicyDigest(pub DigestClaim);
/// SHA-256 of permitted captured UTF-8 bytes, never of the suppressed input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapturedArtifactDigest(pub DigestClaim);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CapturedArtifact {
    pub policy_id: String,
    pub policy_version: u16,
    pub policy_digest: CapturePolicyDigest,
    pub omissions: Vec<Omission>,
    /// Deterministic compact UTF-8 JSON for explicitly permitted values.
    pub bytes: Vec<u8>,
    pub digest: CapturedArtifactDigest,
}

impl CapturePolicy {
    pub fn new(
        policy_id: String,
        max_input_bytes: usize,
        max_work: usize,
        mut rules: Vec<FieldRule>,
    ) -> Result<Self, CaptureError> {
        let valid_id = !policy_id.is_empty()
            && policy_id.len() <= MAX_CAPTURE_NAME_BYTES
            && policy_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if !valid_id
            || max_input_bytes == 0
            || max_input_bytes > MAX_APPLICATION_BYTES
            || max_work == 0
            || max_work > MAX_CAPTURE_WORK
            || rules.len() > MAX_CAPTURE_RULES
        {
            return Err(CaptureError::InvalidPolicy);
        }
        if rules.iter().any(|r| {
            r.name.is_empty() || r.name.len() > MAX_CAPTURE_NAME_BYTES || r.name.contains('\0')
        }) {
            return Err(CaptureError::InvalidPolicy);
        }
        rules.sort_by(|a, b| a.name.cmp(&b.name));
        if rules.windows(2).any(|pair| pair[0].name == pair[1].name) {
            return Err(CaptureError::InvalidPolicy);
        }
        Ok(Self {
            version: CAPTURE_POLICY_VERSION,
            policy_id,
            max_input_bytes,
            max_work,
            rules,
            paths: None,
        })
    }

    /// Construct a v2 path policy. The complete rule list is operator-owned.
    pub fn new_paths(
        policy_id: String,
        max_input_bytes: usize,
        max_work: usize,
        mut paths: Vec<PathRule>,
    ) -> Result<Self, CaptureError> {
        if max_work == 0 || max_work > MAX_CAPTURE_PATH_WORK {
            return Err(CaptureError::InvalidPolicy);
        }
        let mut policy = Self::new(
            policy_id,
            max_input_bytes,
            max_work.min(MAX_CAPTURE_WORK),
            vec![],
        )?;
        policy.max_work = max_work;
        if paths.len() > MAX_CAPTURE_RULES {
            return Err(CaptureError::InvalidPolicy);
        }
        let mut total_path_bytes = 0usize;
        for rule in &paths {
            if rule.path.is_empty() || rule.path.len() > MAX_CAPTURE_PATH_DEPTH {
                return Err(CaptureError::InvalidPolicy);
            }
            for segment in &rule.path {
                total_path_bytes = total_path_bytes
                    .checked_add(segment.len())
                    .ok_or(CaptureError::InvalidPolicy)?;
                if total_path_bytes > MAX_CAPTURE_PATH_BYTES {
                    return Err(CaptureError::InvalidPolicy);
                }
            }
            if rule.path.iter().any(|segment| {
                segment.is_empty()
                    || segment.len() > MAX_CAPTURE_NAME_BYTES
                    || segment.contains('\0')
                    || segment.contains('*')
            }) || matches!(&rule.action, CaptureAction::ScalarArray { max_items, .. } if *max_items == 0 || usize::from(*max_items) > MAX_CAPTURE_ARRAY_ITEMS)
            {
                return Err(CaptureError::InvalidPolicy);
            }
        }
        paths.sort_by(|a, b| a.path.cmp(&b.path));
        if paths
            .windows(2)
            .any(|pair| pair[1].path.starts_with(&pair[0].path))
        {
            return Err(CaptureError::InvalidPolicy);
        }
        policy.version = CAPTURE_POLICY_PATH_VERSION;
        policy.paths = Some(paths);
        if policy.canonical_bytes().len() > MAX_POLICY_BYTES {
            return Err(CaptureError::InvalidPolicy);
        }
        Ok(policy)
    }

    /// Decode an owner-supplied bounded policy and reapply all constructor checks.
    pub fn decode(bytes: &[u8]) -> Result<Self, CaptureError> {
        #[derive(Default)]
        enum PathsField {
            #[default]
            Missing,
            Present(Vec<PathRule>),
        }
        impl<'de> Deserialize<'de> for PathsField {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Vec::<PathRule>::deserialize(d).map(Self::Present)
            }
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            version: u16,
            policy_id: String,
            max_input_bytes: usize,
            max_work: usize,
            rules: Vec<FieldRule>,
            #[serde(default)]
            paths: PathsField,
        }
        if bytes.len() > MAX_POLICY_BYTES {
            return Err(CaptureError::InvalidPolicy);
        }
        let wire: Wire = serde_json::from_slice(bytes).map_err(|_| CaptureError::InvalidPolicy)?;
        match (wire.version, wire.paths) {
            (CAPTURE_POLICY_VERSION, PathsField::Missing) => Self::new(
                wire.policy_id,
                wire.max_input_bytes,
                wire.max_work,
                wire.rules,
            ),
            (CAPTURE_POLICY_PATH_VERSION, PathsField::Present(paths)) if wire.rules.is_empty() => {
                Self::new_paths(wire.policy_id, wire.max_input_bytes, wire.max_work, paths)
            }
            _ => Err(CaptureError::InvalidPolicy),
        }
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        // Construction bounds this serializable value and serde_json's Vec writer is infallible.
        serde_json::to_vec(self).expect("bounded policy serialization")
    }
    pub fn digest(&self) -> CapturePolicyDigest {
        CapturePolicyDigest(claim(&self.canonical_bytes()))
    }

    /// Rejects bounds before parsing; diagnostics never contain input names or values.
    pub fn capture(&self, input: &[u8]) -> Result<CapturedArtifact, CaptureError> {
        if input.len() > self.max_input_bytes {
            return Err(CaptureError::InputSize);
        }
        // Charge each path component for bounded ordered-map lookups as well as parsing.
        let rule_work = match &self.paths {
            Some(paths) => paths
                .iter()
                .try_fold(0usize, |sum, rule| {
                    rule.path.iter().try_fold(sum, |total, segment| {
                        total.checked_add(segment.len().checked_mul(65)?)
                    })
                })
                .ok_or(CaptureError::WorkLimit)?,
            None => self.rules.len().saturating_mul(MAX_CAPTURE_NAME_BYTES),
        };
        // Conservative deterministic reservation for bounded parse, lookups and serialization.
        let work = input
            .len()
            .checked_mul(8)
            .and_then(|n| n.checked_add(rule_work))
            .and_then(|n| n.checked_add(32))
            .ok_or(CaptureError::WorkLimit)?;
        if work > self.max_work {
            return Err(CaptureError::WorkLimit);
        }
        let parsed = application_result::parse(
            input,
            self.max_input_bytes,
            MAX_APPLICATION_DEPTH,
            MAX_APPLICATION_VALUES,
        )
        .map_err(|_: ApplicationResultError| CaptureError::InvalidInput)?;
        let Json::Object(fields) = parsed else {
            return Err(CaptureError::InvalidInput);
        };
        let mut omissions = BTreeSet::new();
        let captured = if let Some(paths) = &self.paths {
            capture_object(fields, paths, 0, &mut omissions)
        } else {
            self.capture_v1(fields, &mut omissions)
        };
        let bytes = serde_json::to_vec(&captured).map_err(|_| CaptureError::InvalidInput)?;
        if bytes.len() > MAX_APPLICATION_BYTES {
            return Err(CaptureError::CapturedSize);
        }
        Ok(CapturedArtifact {
            policy_id: self.policy_id.clone(),
            policy_version: self.version,
            policy_digest: self.digest(),
            omissions: omissions.into_iter().collect(),
            digest: CapturedArtifactDigest(claim(&bytes)),
            bytes,
        })
    }

    fn capture_v1(
        &self,
        fields: BTreeMap<String, Json>,
        omissions: &mut BTreeSet<Omission>,
    ) -> BTreeMap<String, Json> {
        let mut captured = BTreeMap::new();
        for (name, value) in fields {
            let Ok(index) = self.rules.binary_search_by(|r| r.name.cmp(&name)) else {
                omissions.insert(Omission::PolicyExcluded);
                continue;
            };
            let fits = matches!(
                (&self.rules[index].scalar_type, &value),
                (ScalarType::Null, Json::Null(_))
                    | (ScalarType::Boolean, Json::Bool(_))
                    | (ScalarType::Integer, Json::I64(_) | Json::U64(_))
                    | (ScalarType::Text, Json::String(_))
            );
            if !fits {
                omissions.insert(Omission::TypeNotPermitted);
                continue;
            }
            if matches!(&value, Json::String(text) if text.len() > MAX_CAPTURE_TEXT_BYTES) {
                omissions.insert(Omission::ValueLimit);
                continue;
            }
            captured.insert(name, value);
        }
        captured
    }
}

fn scalar_matches(expected: ScalarType, value: &Json) -> bool {
    matches!(
        (expected, value),
        (ScalarType::Null, Json::Null(_))
            | (ScalarType::Boolean, Json::Bool(_))
            | (ScalarType::Integer, Json::I64(_) | Json::U64(_))
            | (ScalarType::Text, Json::String(_))
    )
}
fn capture_object(
    mut input: BTreeMap<String, Json>,
    paths: &[PathRule],
    depth: usize,
    omissions: &mut BTreeSet<Omission>,
) -> BTreeMap<String, Json> {
    let mut output = BTreeMap::new();
    let mut start = 0;
    while start < paths.len() {
        let key = &paths[start].path[depth];
        let mut end = start + 1;
        while end < paths.len() && paths[end].path[depth] == *key {
            end += 1;
        }
        if let Some(value) = input.remove(key) {
            if paths[start].path.len() == depth + 1 {
                if let Some(value) = capture_leaf(value, &paths[start].action, omissions) {
                    output.insert(key.clone(), value);
                }
            } else if let Json::Object(children) = value {
                let nested = capture_object(children, &paths[start..end], depth + 1, omissions);
                if !nested.is_empty() {
                    output.insert(key.clone(), Json::Object(nested));
                }
            } else {
                omissions.insert(Omission::TypeNotPermitted);
            }
        }
        start = end;
    }
    if !input.is_empty() {
        omissions.insert(Omission::PolicyExcluded);
    }
    output
}
fn capture_leaf(
    value: Json,
    action: &CaptureAction,
    omissions: &mut BTreeSet<Omission>,
) -> Option<Json> {
    match action {
        CaptureAction::Redact => {
            omissions.insert(Omission::Redacted);
            Some(Json::String(REDACTED_VALUE.into()))
        }
        CaptureAction::Scalar { scalar_type } => {
            if !scalar_matches(*scalar_type, &value) {
                omissions.insert(Omission::TypeNotPermitted);
                None
            } else if matches!(&value, Json::String(s) if s.len() > MAX_CAPTURE_TEXT_BYTES) {
                omissions.insert(Omission::ValueLimit);
                None
            } else {
                Some(value)
            }
        }
        CaptureAction::ScalarArray {
            scalar_type,
            max_items,
        } => {
            let Json::Array(values) = value else {
                omissions.insert(Omission::TypeNotPermitted);
                return None;
            };
            if values.len() > usize::from(*max_items) {
                omissions.insert(Omission::ValueLimit);
                None
            } else if values
                .iter()
                .any(|item| !scalar_matches(*scalar_type, item))
            {
                omissions.insert(Omission::TypeNotPermitted);
                None
            } else if values
                .iter()
                .any(|item| matches!(item, Json::String(s) if s.len() > MAX_CAPTURE_TEXT_BYTES))
            {
                omissions.insert(Omission::ValueLimit);
                None
            } else {
                Some(Json::Array(values))
            }
        }
    }
}

fn claim(bytes: &[u8]) -> DigestClaim {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut hex = String::with_capacity(64);
    for b in Sha256::digest(bytes) {
        hex.push(HEX[(b >> 4) as usize] as char);
        hex.push(HEX[(b & 15) as usize] as char);
    }
    DigestClaim {
        algorithm: DigestAlgorithm::Sha256,
        representation: DigestRepresentation::ExactBytes,
        hex,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, scalar_type: ScalarType) -> FieldRule {
        FieldRule {
            name: name.into(),
            scalar_type,
        }
    }
    fn policy() -> CapturePolicy {
        CapturePolicy::new(
            "policy-a".into(),
            4096,
            65536,
            vec![rule("allowed", ScalarType::Integer)],
        )
        .unwrap()
    }

    #[test]
    fn excludes_names_and_values_before_digest_with_independent_vectors() {
        let one = policy()
            .capture(
                br#"{"allowed":7,"forbidden-secret":"SENTINEL-ONE","nested":{"hidden":"LEAK"}}"#,
            )
            .unwrap();
        let two = policy()
            .capture(br#"{"allowed":7,"other-private":"SENTINEL-TWO"}"#)
            .unwrap();
        assert_eq!(one.bytes, br#"{"allowed":7}"#);
        assert_eq!(one.bytes, two.bytes);
        assert_eq!(one.digest, two.digest);
        assert_eq!(one.omissions, vec![Omission::PolicyExcluded]);
        assert_eq!(one.omissions, two.omissions);
        assert_eq!(
            one.digest.0.hex,
            "3964636761699061768bb4e417d5d69e0a6008b863d6473a10e36405094ccf46"
        );
        assert_eq!(
            one.policy_digest.0.hex,
            "96ccd3d813c73f9410069c1ad791aa80fa2cd5675cbf5b4028eadfce567759ea"
        );
        let serialized = serde_json::to_string(&one).unwrap();
        for forbidden in [
            "forbidden-secret",
            "SENTINEL-ONE",
            "nested",
            "hidden",
            "LEAK",
        ] {
            assert!(!serialized.contains(forbidden));
        }
    }

    #[test]
    fn default_deny_and_typed_scalar_omissions_do_not_emit_content() {
        let empty = CapturePolicy::new("deny".into(), 4096, 65536, vec![]).unwrap();
        let captured = empty.capture(br#"{"secret":"SENTINEL"}"#).unwrap();
        assert_eq!(captured.bytes, b"{}");
        assert_eq!(captured.omissions, vec![Omission::PolicyExcluded]);
        let p = CapturePolicy::new(
            "typed".into(),
            4096,
            65536,
            vec![
                rule("text", ScalarType::Text),
                rule("int", ScalarType::Integer),
            ],
        )
        .unwrap();
        let input = format!(
            "{{\"text\":\"{}\",\"int\":{{\"private\":\"SENTINEL\"}}}}",
            "x".repeat(MAX_CAPTURE_TEXT_BYTES + 1)
        );
        let captured = p.capture(input.as_bytes()).unwrap();
        assert_eq!(captured.bytes, b"{}");
        assert_eq!(
            captured.omissions,
            vec![Omission::TypeNotPermitted, Omission::ValueLimit]
        );
        assert!(
            !serde_json::to_string(&captured)
                .unwrap()
                .contains("SENTINEL")
        );
    }

    #[test]
    fn policy_order_is_canonical_and_invalid_rules_fail_closed() {
        let a = CapturePolicy::new(
            "p".into(),
            4096,
            65536,
            vec![rule("z", ScalarType::Text), rule("a", ScalarType::Boolean)],
        )
        .unwrap();
        let b = CapturePolicy::new(
            "p".into(),
            4096,
            65536,
            vec![rule("a", ScalarType::Boolean), rule("z", ScalarType::Text)],
        )
        .unwrap();
        assert_eq!(a.digest(), b.digest());
        assert_eq!(
            a.capture(br#"{"z":"yes","a":true}"#).unwrap().bytes,
            br#"{"a":true,"z":"yes"}"#
        );
        assert!(
            CapturePolicy::new(
                "p".into(),
                4096,
                65536,
                vec![rule("a", ScalarType::Text), rule("a", ScalarType::Integer)]
            )
            .is_err()
        );
        assert!(
            CapturePolicy::new(
                "p".into(),
                4096,
                65536,
                (0..=MAX_CAPTURE_RULES)
                    .map(|n| rule(&format!("a{n}"), ScalarType::Text))
                    .collect()
            )
            .is_err()
        );
        assert!(CapturePolicy::new("p".into(), 4097, 65536, vec![]).is_err());
        assert!(CapturePolicy::new("bad policy".into(), 4096, 65536, vec![]).is_err());
    }

    #[test]
    fn bounds_and_strict_json_fail_without_echoing_input() {
        let p = policy();
        for bad in [
            br#"{"allowed":7,"\u0061llowed":8}"#.as_slice(),
            br#"{"allowed":1.2}"#,
            br#"{"allowed":1e3}"#,
            br#"{"allowed":-0}"#,
            br#"{"allowed":18446744073709551616}"#,
            &[0xff],
        ] {
            assert_eq!(p.capture(bad), Err(CaptureError::InvalidInput));
        }
        assert_eq!(p.capture(&vec![b'x'; 4097]), Err(CaptureError::InputSize));
        let restricted = CapturePolicy::new("p".into(), 4096, 32, vec![]).unwrap();
        assert_eq!(
            restricted.capture(br#"{"SENTINEL":"secret"}"#),
            Err(CaptureError::WorkLimit)
        );
        let depth = format!(
            "{{\"hidden\":{}0{}}}",
            "[".repeat(MAX_APPLICATION_DEPTH + 1),
            "]".repeat(MAX_APPLICATION_DEPTH + 1)
        );
        assert_eq!(p.capture(depth.as_bytes()), Err(CaptureError::InvalidInput));
    }
}
