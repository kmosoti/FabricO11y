//! Bounded, Sans-I/O run and attempt provenance manifests.
//!
//! Digest values are declarations. This module validates their algorithm,
//! representation and syntax; it does not compute or verify content hashes.

use serde::{Deserialize, Serialize};

pub const MANIFEST_SCHEMA_VERSION: u16 = 1;
/// Maximum compact JSON envelope accepted or emitted by this contract.
pub const MAX_MANIFEST_WIRE_BYTES: usize = 64 * 1024;
pub const MAX_MANIFEST_ITEMS: usize = 64;
pub const MAX_TEXT_BYTES: usize = 1024;
pub const MAX_ID_BYTES: usize = 128;
pub const MAX_ARTIFACTS: usize = 32;
pub const MAX_OBSERVATIONS: usize = 64;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DigestAlgorithm {
    Sha256,
}

/// Describes exactly which bytes a declared digest names.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DigestRepresentation {
    ExactBytes,
    CanonicalJsonV1,
    Utf8Text,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DigestClaim {
    pub algorithm: DigestAlgorithm,
    pub representation: DigestRepresentation,
    /// Lower-case hexadecimal encoding of the digest bytes.
    pub hex: String,
}

impl DigestClaim {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.hex.len() != 64
            || !self
                .hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ManifestError::InvalidDigest);
        }
        Ok(())
    }
}

macro_rules! digest_role {
    ($name:ident) => {
        #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
        #[serde(transparent)]
        pub struct $name(pub DigestClaim);
        impl $name {
            pub fn validate(&self) -> Result<(), ManifestError> {
                self.0.validate()
            }
        }
    };
}

// These wrappers make the digest's semantic role part of its Rust type.
digest_role!(RunManifestDigest);
digest_role!(AttemptManifestDigest);
digest_role!(ArtifactContentDigest);
digest_role!(CanonicalRecordDigest);
digest_role!(PhysicalFileDigest);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImmutableReference {
    pub id: String,
    pub digest: DigestClaim,
}

impl ImmutableReference {
    pub fn validate(&self) -> Result<(), ManifestError> {
        bounded(&self.id, MAX_ID_BYTES)?;
        self.digest.validate()
    }
}

/// All absence and disclosure states remain explicit on the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum Disclosure<T> {
    Available(T),
    Unavailable,
    Redacted,
    Unknown,
    Inapplicable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Code,
    SourceTree,
    Configuration,
    Input,
    Dataset,
    Cohort,
    Evaluator,
    Toolchain,
    CapturePolicy,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReference {
    pub kind: ArtifactKind,
    pub locator: String,
    pub media_type: String,
    pub digest: ArtifactContentDigest,
}

impl ArtifactReference {
    fn validate(&self) -> Result<(), ManifestError> {
        bounded(&self.locator, MAX_TEXT_BYTES)?;
        bounded(&self.media_type, 128)?;
        self.digest.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RunManifest {
    pub schema_version: u16,
    pub immutable_id: String,
    pub digest: RunManifestDigest,
    pub code: Disclosure<ArtifactReference>,
    pub source_tree: Disclosure<ArtifactReference>,
    pub configuration: Disclosure<ArtifactReference>,
    pub inputs: Disclosure<Vec<ArtifactReference>>,
    pub dataset: Disclosure<ArtifactReference>,
    pub cohort: Disclosure<ArtifactReference>,
    pub evaluator: Disclosure<ArtifactReference>,
    pub toolchain: Disclosure<ArtifactReference>,
    pub capture_policy: Disclosure<ArtifactReference>,
    pub artifacts: Vec<ArtifactReference>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OverrideKind {
    Input,
    Evaluator,
    Toolchain,
    CapturePolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttemptOverride {
    pub kind: OverrideKind,
    pub artifact: ArtifactReference,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttemptManifest {
    pub schema_version: u16,
    pub immutable_id: String,
    pub digest: AttemptManifestDigest,
    pub run: ImmutableReference,
    pub overrides: Vec<AttemptOverride>,
}

/// A timestamp's origin is provenance; timestamps from different clocks are not
/// globally ordered by this contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClockProvenance {
    pub clock_id: String,
    pub source: String,
    pub uncertainty_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProvenancedTime {
    pub unix_ms: i64,
    pub clock: ClockProvenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObservationReference {
    pub record_id: String,
    pub attempt: ImmutableReference,
    pub canonical_record_digest: CanonicalRecordDigest,
    pub physical_file_digest: Disclosure<PhysicalFileDigest>,
    pub source_time: Disclosure<ProvenancedTime>,
    pub observed_time: Disclosure<ProvenancedTime>,
    pub custody_time: Disclosure<ProvenancedTime>,
    pub verified_time: Disclosure<ProvenancedTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManifestEnvelope {
    pub schema_version: u16,
    pub run: RunManifest,
    pub attempts: Vec<AttemptManifest>,
    pub observations: Vec<ObservationReference>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestError {
    Oversized,
    InvalidJson,
    UnsupportedVersion,
    InvalidDigest,
    InvalidId,
    InvalidBound,
    InvalidReference,
}

impl ManifestEnvelope {
    /// Checks byte length before JSON parsing, then validates every nested value.
    pub fn decode_bounded(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() > MAX_MANIFEST_WIRE_BYTES {
            return Err(ManifestError::Oversized);
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| ManifestError::InvalidJson)?;
        value.validate()?;
        Ok(value)
    }

    pub fn encode_bounded(&self) -> Result<Vec<u8>, ManifestError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| ManifestError::InvalidJson)?;
        if bytes.len() > MAX_MANIFEST_WIRE_BYTES {
            return Err(ManifestError::Oversized);
        }
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        version(self.schema_version)?;
        validate_run(&self.run)?;
        if self.attempts.len() > MAX_MANIFEST_ITEMS || self.observations.len() > MAX_OBSERVATIONS {
            return Err(ManifestError::InvalidBound);
        }
        for (index, a) in self.attempts.iter().enumerate() {
            validate_attempt(a)?;
            if a.run.id != self.run.immutable_id || a.run.digest != self.run.digest.0 {
                return Err(ManifestError::InvalidReference);
            }
            if self.attempts[..index]
                .iter()
                .any(|prior| prior.immutable_id == a.immutable_id)
            {
                return Err(ManifestError::InvalidReference);
            }
        }
        for (index, o) in self.observations.iter().enumerate() {
            validate_observation(o)?;
            if !self
                .attempts
                .iter()
                .any(|a| a.immutable_id == o.attempt.id && a.digest.0 == o.attempt.digest)
            {
                return Err(ManifestError::InvalidReference);
            }
            if self.observations[..index]
                .iter()
                .any(|prior| prior.record_id == o.record_id)
            {
                return Err(ManifestError::InvalidReference);
            }
        }
        Ok(())
    }
}

fn version(v: u16) -> Result<(), ManifestError> {
    if v == MANIFEST_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(ManifestError::UnsupportedVersion)
    }
}
fn bounded(s: &str, max: usize) -> Result<(), ManifestError> {
    if s.is_empty() || s.len() > max {
        Err(ManifestError::InvalidBound)
    } else {
        Ok(())
    }
}
fn valid_id(s: &str) -> Result<(), ManifestError> {
    bounded(s, MAX_ID_BYTES)?;
    if s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b':')
    {
        Ok(())
    } else {
        Err(ManifestError::InvalidId)
    }
}
fn validate_artifacts(v: &[ArtifactReference]) -> Result<(), ManifestError> {
    if v.len() > MAX_ARTIFACTS {
        return Err(ManifestError::InvalidBound);
    }
    for a in v {
        a.validate()?;
    }
    Ok(())
}
fn validate_disclosed_artifact(v: &Disclosure<ArtifactReference>) -> Result<(), ManifestError> {
    if let Disclosure::Available(a) = v {
        a.validate()?;
    }
    Ok(())
}
fn validate_run(v: &RunManifest) -> Result<(), ManifestError> {
    version(v.schema_version)?;
    valid_id(&v.immutable_id)?;
    v.digest.validate()?;
    if v.digest.0.representation != DigestRepresentation::CanonicalJsonV1 {
        return Err(ManifestError::InvalidDigest);
    }
    for a in [
        &v.code,
        &v.source_tree,
        &v.configuration,
        &v.dataset,
        &v.cohort,
        &v.evaluator,
        &v.toolchain,
        &v.capture_policy,
    ] {
        validate_disclosed_artifact(a)?;
    }
    if let Disclosure::Available(inputs) = &v.inputs {
        validate_artifacts(inputs)?;
    }
    validate_artifacts(&v.artifacts)
}
fn validate_attempt(v: &AttemptManifest) -> Result<(), ManifestError> {
    version(v.schema_version)?;
    valid_id(&v.immutable_id)?;
    v.digest.validate()?;
    v.run.validate()?;
    if v.digest.0.representation != DigestRepresentation::CanonicalJsonV1 {
        return Err(ManifestError::InvalidDigest);
    }
    if v.overrides.len() > MAX_MANIFEST_ITEMS {
        return Err(ManifestError::InvalidBound);
    }
    for o in &v.overrides {
        o.artifact.validate()?;
    }
    Ok(())
}
fn validate_time(v: &Disclosure<ProvenancedTime>) -> Result<(), ManifestError> {
    if let Disclosure::Available(t) = v {
        bounded(&t.clock.clock_id, 128)?;
        bounded(&t.clock.source, 256)?;
    }
    Ok(())
}
fn validate_observation(v: &ObservationReference) -> Result<(), ManifestError> {
    valid_id(&v.record_id)?;
    v.attempt.validate()?;
    v.canonical_record_digest.validate()?;
    if v.canonical_record_digest.0.representation != DigestRepresentation::CanonicalJsonV1 {
        return Err(ManifestError::InvalidDigest);
    }
    if let Disclosure::Available(d) = &v.physical_file_digest {
        d.validate()?;
        if d.0.representation != DigestRepresentation::ExactBytes {
            return Err(ManifestError::InvalidDigest);
        }
    }
    validate_time(&v.source_time)?;
    validate_time(&v.observed_time)?;
    validate_time(&v.custody_time)?;
    validate_time(&v.verified_time)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(hex: char) -> DigestClaim {
        DigestClaim {
            algorithm: DigestAlgorithm::Sha256,
            representation: DigestRepresentation::CanonicalJsonV1,
            hex: hex.to_string().repeat(64),
        }
    }
    fn artifact(kind: ArtifactKind, name: &str) -> ArtifactReference {
        ArtifactReference {
            kind,
            locator: format!("artifact:{name}"),
            media_type: "application/octet-stream".into(),
            digest: ArtifactContentDigest(claim('a')),
        }
    }
    fn envelope() -> ManifestEnvelope {
        let run = RunManifest {
            schema_version: 1,
            immutable_id: "run-1".into(),
            digest: RunManifestDigest(claim('b')),
            code: Disclosure::Available(artifact(ArtifactKind::Code, "code")),
            source_tree: Disclosure::Available(artifact(ArtifactKind::SourceTree, "tree")),
            configuration: Disclosure::Unknown,
            inputs: Disclosure::Available(vec![artifact(ArtifactKind::Input, "input")]),
            dataset: Disclosure::Unavailable,
            cohort: Disclosure::Redacted,
            evaluator: Disclosure::Available(artifact(ArtifactKind::Evaluator, "eval")),
            toolchain: Disclosure::Available(artifact(ArtifactKind::Toolchain, "rust")),
            capture_policy: Disclosure::Inapplicable,
            artifacts: vec![artifact(ArtifactKind::Other, "report")],
        };
        let attempt = |id: &str, c| AttemptManifest {
            schema_version: 1,
            immutable_id: id.into(),
            digest: AttemptManifestDigest(claim(c)),
            run: ImmutableReference {
                id: run.immutable_id.clone(),
                digest: run.digest.0.clone(),
            },
            overrides: vec![],
        };
        let a1 = attempt("attempt-1", 'c');
        let a2 = attempt("attempt-2", 'd');
        let obs = |id: &str, a: &AttemptManifest| ObservationReference {
            record_id: id.into(),
            attempt: ImmutableReference {
                id: a.immutable_id.clone(),
                digest: a.digest.0.clone(),
            },
            canonical_record_digest: CanonicalRecordDigest(claim('e')),
            physical_file_digest: Disclosure::Unknown,
            source_time: Disclosure::Available(ProvenancedTime {
                unix_ms: 10,
                clock: ClockProvenance {
                    clock_id: "source-clock".into(),
                    source: "agent runtime".into(),
                    uncertainty_ms: Some(3),
                },
            }),
            observed_time: Disclosure::Unavailable,
            custody_time: Disclosure::Redacted,
            verified_time: Disclosure::Inapplicable,
        };
        ManifestEnvelope {
            schema_version: 1,
            run,
            attempts: vec![a1.clone(), a2.clone()],
            observations: vec![
                obs("record-1", &a1),
                obs("record-2", &a1),
                obs("record-3", &a2),
            ],
        }
    }

    #[test]
    fn bounded_roundtrip_shares_run_and_attempt_manifests() {
        let x = envelope();
        let bytes = x.encode_bounded().unwrap();
        assert_eq!(ManifestEnvelope::decode_bounded(&bytes).unwrap(), x);
    }
    #[test]
    fn all_disclosure_states_are_distinct() {
        let states = [
            Disclosure::<String>::Available("x".into()),
            Disclosure::Unavailable,
            Disclosure::Redacted,
            Disclosure::Unknown,
            Disclosure::Inapplicable,
        ];
        let encoded: Vec<_> = states
            .iter()
            .map(|x| serde_json::to_string(x).unwrap())
            .collect();
        assert_eq!(
            encoded
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            5
        );
    }
    #[test]
    fn requires_clock_provenance_when_time_is_present() {
        let mut x = envelope();
        x.observations[0].source_time = Disclosure::Available(ProvenancedTime {
            unix_ms: 5,
            clock: ClockProvenance {
                clock_id: "".into(),
                source: "s".into(),
                uncertainty_ms: None,
            },
        });
        assert_eq!(x.validate(), Err(ManifestError::InvalidBound));
    }
    #[test]
    fn refuses_invalid_hashes_and_wrong_role_references() {
        let mut x = envelope();
        x.run.digest.0.hex = "AB".into();
        assert_eq!(x.validate(), Err(ManifestError::InvalidDigest));
        let mut x = envelope();
        x.observations[0].attempt.digest = claim('d');
        assert_eq!(x.validate(), Err(ManifestError::InvalidReference));
    }
    #[test]
    fn refuses_oversized_bytes_before_decoding() {
        assert_eq!(
            ManifestEnvelope::decode_bounded(&vec![b' '; MAX_MANIFEST_WIRE_BYTES + 1]),
            Err(ManifestError::Oversized)
        );
    }
    #[test]
    fn refuses_nested_bounds_and_version_mismatch() {
        let mut x = envelope();
        x.observations.push(x.observations[0].clone());
        x.observations[0].record_id = "".into();
        assert_eq!(x.validate(), Err(ManifestError::InvalidBound));
        let mut y = envelope();
        y.run.schema_version = 2;
        assert_eq!(y.validate(), Err(ManifestError::UnsupportedVersion));
    }
}
