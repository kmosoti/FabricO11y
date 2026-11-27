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

/// Configuration must be established by a trusted caller, not an input record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapturePolicy {
    version: u16,
    policy_id: String,
    max_input_bytes: usize,
    max_work: usize,
    rules: Vec<FieldRule>,
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
    /// Deterministic compact UTF-8 JSON for permitted top-level scalar values.
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
        })
    }

    /// Decode an owner-supplied bounded policy and reapply all constructor checks.
    pub fn decode(bytes: &[u8]) -> Result<Self, CaptureError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            version: u16,
            policy_id: String,
            max_input_bytes: usize,
            max_work: usize,
            rules: Vec<FieldRule>,
        }
        if bytes.len() > 16384 {
            return Err(CaptureError::InvalidPolicy);
        }
        let wire: Wire = serde_json::from_slice(bytes).map_err(|_| CaptureError::InvalidPolicy)?;
        if wire.version != CAPTURE_POLICY_VERSION {
            return Err(CaptureError::InvalidPolicy);
        }
        Self::new(
            wire.policy_id,
            wire.max_input_bytes,
            wire.max_work,
            wire.rules,
        )
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
        // Conservative deterministic reservation for bounded parse, lookups and serialization.
        let work = input
            .len()
            .checked_mul(8)
            .and_then(|n| n.checked_add(self.rules.len().saturating_mul(MAX_CAPTURE_NAME_BYTES)))
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
        let mut captured = BTreeMap::new();
        let mut omissions = BTreeSet::new();
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
