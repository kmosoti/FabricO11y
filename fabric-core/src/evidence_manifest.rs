//! Bounded, Sans-I/O run and attempt provenance manifests.
//!
//! Manifest hashes are recomputed from the canonical typed payloads, excluding
//! each self-digest. Exact-byte claims can be verified when their bytes are
//! supplied; this module does not fetch the referenced artifacts or records.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{self, Write};

pub const MANIFEST_SCHEMA_VERSION: u16 = 1;
/// Maximum compact JSON envelope accepted or emitted by this contract.
pub const MAX_MANIFEST_WIRE_BYTES: usize = 64 * 1024;
pub const MAX_MANIFEST_ITEMS: usize = 64;
pub const MAX_TEXT_BYTES: usize = 1024;
pub const MAX_ID_BYTES: usize = 128;
pub const MAX_ARTIFACTS: usize = 32;
pub const MAX_OBSERVATIONS: usize = 64;
pub const MAX_ATTEMPT_INPUT_OVERRIDES: usize = 32;
pub const MAX_ATTEMPT_OVERRIDES: usize = MAX_ATTEMPT_INPUT_OVERRIDES + 3;

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
    /// Reserved for the closed run/attempt payload profile documented below.
    CanonicalJsonV1,
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
digest_role!(OriginalArtifactDigest);
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

impl ArtifactContentDigest {
    /// Checks an artifact's exact source bytes against its declared digest.
    pub fn verify_exact_bytes(&self, bytes: &[u8]) -> Result<(), ManifestError> {
        verify_exact_bytes(&self.0, bytes)
    }
}

impl OriginalArtifactDigest {
    /// Checks the original artifact bytes, including any trailing newline.
    pub fn verify_exact_bytes(&self, bytes: &[u8]) -> Result<(), ManifestError> {
        verify_exact_bytes(&self.0, bytes)
    }
}

impl CanonicalRecordDigest {
    /// Checks the exact canonical payload bytes retained for a record.
    pub fn verify_exact_bytes(&self, bytes: &[u8]) -> Result<(), ManifestError> {
        verify_exact_bytes(&self.0, bytes)
    }
}

impl PhysicalFileDigest {
    /// Checks the exact physical file bytes, including storage framing.
    pub fn verify_exact_bytes(&self, bytes: &[u8]) -> Result<(), ManifestError> {
        verify_exact_bytes(&self.0, bytes)
    }
}

fn verify_exact_bytes(claim: &DigestClaim, bytes: &[u8]) -> Result<(), ManifestError> {
    claim.validate()?;
    if claim.representation != DigestRepresentation::ExactBytes
        || hex_lower(&Sha256::digest(bytes)) != claim.hex
    {
        return Err(ManifestError::InvalidDigest);
    }
    Ok(())
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// All absence and disclosure states remain explicit on the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "state",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
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
        self.digest.validate().and_then(|()| {
            if self.digest.0.representation == DigestRepresentation::ExactBytes {
                Ok(())
            } else {
                Err(ManifestError::InvalidDigest)
            }
        })
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

#[derive(Serialize)]
struct RunManifestCanonical<'a> {
    schema_version: u16,
    immutable_id: &'a str,
    code: &'a Disclosure<ArtifactReference>,
    source_tree: &'a Disclosure<ArtifactReference>,
    configuration: &'a Disclosure<ArtifactReference>,
    inputs: &'a Disclosure<Vec<ArtifactReference>>,
    dataset: &'a Disclosure<ArtifactReference>,
    cohort: &'a Disclosure<ArtifactReference>,
    evaluator: &'a Disclosure<ArtifactReference>,
    toolchain: &'a Disclosure<ArtifactReference>,
    capture_policy: &'a Disclosure<ArtifactReference>,
    artifacts: &'a [ArtifactReference],
}

impl RunManifest {
    /// Canonical JSON v1 bytes, excluding this manifest's own digest field.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ManifestError> {
        let view = RunManifestCanonical {
            schema_version: self.schema_version,
            immutable_id: &self.immutable_id,
            code: &self.code,
            source_tree: &self.source_tree,
            configuration: &self.configuration,
            inputs: &self.inputs,
            dataset: &self.dataset,
            cohort: &self.cohort,
            evaluator: &self.evaluator,
            toolchain: &self.toolchain,
            capture_policy: &self.capture_policy,
            artifacts: &self.artifacts,
        };
        serialize_capped(&view, MAX_MANIFEST_WIRE_BYTES)
    }

    pub fn recompute_digest(&self) -> Result<RunManifestDigest, ManifestError> {
        Ok(RunManifestDigest(canonical_claim(&self.canonical_bytes()?)))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
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

#[derive(Serialize)]
struct AttemptManifestCanonical<'a> {
    schema_version: u16,
    immutable_id: &'a str,
    run: &'a ImmutableReference,
    overrides: &'a [AttemptOverride],
}

impl AttemptManifest {
    /// Canonical JSON v1 bytes, excluding this manifest's own digest field.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ManifestError> {
        let view = AttemptManifestCanonical {
            schema_version: self.schema_version,
            immutable_id: &self.immutable_id,
            run: &self.run,
            overrides: &self.overrides,
        };
        serialize_capped(&view, MAX_MANIFEST_WIRE_BYTES)
    }

    pub fn recompute_digest(&self) -> Result<AttemptManifestDigest, ManifestError> {
        Ok(AttemptManifestDigest(canonical_claim(
            &self.canonical_bytes()?,
        )))
    }
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
    pub original_artifact_digest: Disclosure<OriginalArtifactDigest>,
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

struct CappedWriter {
    bytes: Vec<u8>,
    limit: usize,
    overflowed: bool,
}

impl Write for CappedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            self.overflowed = true;
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "encoded byte budget exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn serialize_capped<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>, ManifestError> {
    let mut writer = CappedWriter {
        bytes: Vec::with_capacity(limit),
        limit,
        overflowed: false,
    };
    if serde_json::to_writer(&mut writer, value).is_err() {
        return if writer.overflowed {
            Err(ManifestError::Oversized)
        } else {
            Err(ManifestError::InvalidJson)
        };
    }
    Ok(writer.bytes)
}

fn canonical_claim(bytes: &[u8]) -> DigestClaim {
    DigestClaim {
        algorithm: DigestAlgorithm::Sha256,
        representation: DigestRepresentation::CanonicalJsonV1,
        hex: hex_lower(&Sha256::digest(bytes)),
    }
}

fn validate_canonical_claim(claim: &DigestClaim, bytes: &[u8]) -> Result<(), ManifestError> {
    claim.validate()?;
    if claim.representation != DigestRepresentation::CanonicalJsonV1
        || canonical_claim(bytes).hex != claim.hex
    {
        return Err(ManifestError::InvalidDigest);
    }
    Ok(())
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
        serialize_capped(self, MAX_MANIFEST_WIRE_BYTES)
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
    for (disclosed, expected_kind) in [
        (&v.code, ArtifactKind::Code),
        (&v.source_tree, ArtifactKind::SourceTree),
        (&v.configuration, ArtifactKind::Configuration),
        (&v.dataset, ArtifactKind::Dataset),
        (&v.cohort, ArtifactKind::Cohort),
        (&v.evaluator, ArtifactKind::Evaluator),
        (&v.toolchain, ArtifactKind::Toolchain),
        (&v.capture_policy, ArtifactKind::CapturePolicy),
    ] {
        validate_disclosed_artifact(disclosed)?;
        if let Disclosure::Available(a) = disclosed
            && a.kind != expected_kind
        {
            return Err(ManifestError::InvalidReference);
        }
    }
    if let Disclosure::Available(inputs) = &v.inputs {
        validate_artifacts(inputs)?;
        if inputs.iter().any(|a| a.kind != ArtifactKind::Input) {
            return Err(ManifestError::InvalidReference);
        }
    }
    validate_artifacts(&v.artifacts)?;
    validate_canonical_claim(&v.digest.0, &v.canonical_bytes()?)
}
fn validate_attempt(v: &AttemptManifest) -> Result<(), ManifestError> {
    version(v.schema_version)?;
    valid_id(&v.immutable_id)?;
    v.run.validate()?;
    if v.overrides.len() > MAX_ATTEMPT_OVERRIDES {
        return Err(ManifestError::InvalidBound);
    }
    let mut seen_singletons = std::collections::BTreeSet::new();
    let mut input_locators = std::collections::BTreeSet::new();
    let mut input_count = 0;
    for o in &v.overrides {
        o.artifact.validate()?;
        if o.artifact.kind != override_artifact_kind(o.kind) {
            return Err(ManifestError::InvalidReference);
        }
        if o.kind == OverrideKind::Input {
            input_count += 1;
            if !input_locators.insert(&o.artifact.locator) {
                return Err(ManifestError::InvalidReference);
            }
        } else if !seen_singletons.insert(o.kind) {
            return Err(ManifestError::InvalidReference);
        }
    }
    if input_count > MAX_ATTEMPT_INPUT_OVERRIDES {
        return Err(ManifestError::InvalidBound);
    }
    validate_canonical_claim(&v.digest.0, &v.canonical_bytes()?)
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
    if v.canonical_record_digest.0.representation != DigestRepresentation::ExactBytes {
        return Err(ManifestError::InvalidDigest);
    }
    if let Disclosure::Available(d) = &v.original_artifact_digest {
        d.validate()?;
        if d.0.representation != DigestRepresentation::ExactBytes {
            return Err(ManifestError::InvalidDigest);
        }
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

fn override_artifact_kind(kind: OverrideKind) -> ArtifactKind {
    match kind {
        OverrideKind::Input => ArtifactKind::Input,
        OverrideKind::Evaluator => ArtifactKind::Evaluator,
        OverrideKind::Toolchain => ArtifactKind::Toolchain,
        OverrideKind::CapturePolicy => ArtifactKind::CapturePolicy,
    }
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
        let source = name.as_bytes();
        ArtifactReference {
            kind,
            locator: format!("artifact:{name}"),
            media_type: "application/octet-stream".into(),
            digest: ArtifactContentDigest(DigestClaim {
                algorithm: DigestAlgorithm::Sha256,
                representation: DigestRepresentation::ExactBytes,
                hex: hex_lower(&Sha256::digest(source)),
            }),
        }
    }
    fn envelope() -> ManifestEnvelope {
        let mut run = RunManifest {
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
        run.digest = run.recompute_digest().unwrap();
        let attempt = |id: &str| AttemptManifest {
            schema_version: 1,
            immutable_id: id.into(),
            digest: AttemptManifestDigest(claim('0')),
            run: ImmutableReference {
                id: run.immutable_id.clone(),
                digest: run.digest.0.clone(),
            },
            overrides: vec![],
        };
        let mut a1 = attempt("attempt-1");
        a1.digest = a1.recompute_digest().unwrap();
        let mut a2 = attempt("attempt-2");
        a2.digest = a2.recompute_digest().unwrap();
        let obs = |id: &str, a: &AttemptManifest| ObservationReference {
            record_id: id.into(),
            attempt: ImmutableReference {
                id: a.immutable_id.clone(),
                digest: a.digest.0.clone(),
            },
            original_artifact_digest: Disclosure::Available(OriginalArtifactDigest(DigestClaim {
                algorithm: DigestAlgorithm::Sha256,
                representation: DigestRepresentation::ExactBytes,
                hex: hex_lower(&Sha256::digest(b"original artifact\n")),
            })),
            canonical_record_digest: CanonicalRecordDigest(DigestClaim {
                algorithm: DigestAlgorithm::Sha256,
                representation: DigestRepresentation::ExactBytes,
                hex: hex_lower(&Sha256::digest(b"canonical record bytes")),
            }),
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

        let mut z = envelope();
        z.run.artifacts[0].locator = "x".repeat(MAX_TEXT_BYTES + 1);
        z.run.digest = z.run.recompute_digest().unwrap();
        assert_eq!(z.validate(), Err(ManifestError::InvalidBound));
    }

    #[test]
    fn canonical_profile_excludes_self_digest_and_recomputes_claims() {
        let mut x = envelope();
        let before = x.run.canonical_bytes().unwrap();
        x.run.digest.0.hex = "f".repeat(64);
        assert_eq!(x.run.canonical_bytes().unwrap(), before);
        assert_eq!(
            x.run.recompute_digest().unwrap().0.hex,
            hex_lower(&Sha256::digest(&before))
        );
        assert_eq!(x.validate(), Err(ManifestError::InvalidDigest));

        let mut fixture = envelope();
        let mut attempt = fixture.attempts.remove(0);
        let before = attempt.canonical_bytes().unwrap();
        attempt.digest.0.hex = "f".repeat(64);
        assert_eq!(attempt.canonical_bytes().unwrap(), before);
        assert_eq!(
            validate_attempt(&attempt),
            Err(ManifestError::InvalidDigest)
        );
    }

    #[test]
    fn canonical_json_v1_has_independent_utf8_and_newline_vector() {
        let mut run = RunManifest {
            schema_version: 1,
            immutable_id: "run-vector".into(),
            digest: RunManifestDigest(claim('0')),
            code: Disclosure::Available(ArtifactReference {
                kind: ArtifactKind::Code,
                locator: "artifact:café\n".into(),
                media_type: "text/plain".into(),
                digest: ArtifactContentDigest(DigestClaim {
                    algorithm: DigestAlgorithm::Sha256,
                    representation: DigestRepresentation::ExactBytes,
                    hex: "0".repeat(64),
                }),
            }),
            source_tree: Disclosure::Unknown,
            configuration: Disclosure::Unknown,
            inputs: Disclosure::Unknown,
            dataset: Disclosure::Unknown,
            cohort: Disclosure::Unknown,
            evaluator: Disclosure::Unknown,
            toolchain: Disclosure::Unknown,
            capture_policy: Disclosure::Unknown,
            artifacts: vec![],
        };
        let expected = r#"{"schema_version":1,"immutable_id":"run-vector","code":{"state":"available","value":{"kind":"code","locator":"artifact:café\n","media_type":"text/plain","digest":{"algorithm":"sha256","representation":"exact_bytes","hex":"0000000000000000000000000000000000000000000000000000000000000000"}}},"source_tree":{"state":"unknown"},"configuration":{"state":"unknown"},"inputs":{"state":"unknown"},"dataset":{"state":"unknown"},"cohort":{"state":"unknown"},"evaluator":{"state":"unknown"},"toolchain":{"state":"unknown"},"capture_policy":{"state":"unknown"},"artifacts":[]}"#;
        assert_eq!(run.canonical_bytes().unwrap(), expected.as_bytes());
        let digest = run.recompute_digest().unwrap();
        assert_eq!(
            digest.0.hex,
            "5e9238dab2a307d06661114bb974540441b1699c19237399499d3f3381938ea9"
        );
        run.digest = digest;
        assert_eq!(validate_run(&run), Ok(()));
    }

    #[test]
    fn rejects_manifest_field_role_mismatch_and_duplicate_overrides() {
        let mut x = envelope();
        if let Disclosure::Available(code) = &mut x.run.code {
            code.kind = ArtifactKind::Input;
        }
        x.run.digest = x.run.recompute_digest().unwrap();
        assert_eq!(x.validate(), Err(ManifestError::InvalidReference));

        let mut x = envelope();
        x.attempts[0].overrides = vec![AttemptOverride {
            kind: OverrideKind::Evaluator,
            artifact: artifact(ArtifactKind::Toolchain, "wrong-role"),
        }];
        x.attempts[0].digest = x.attempts[0].recompute_digest().unwrap();
        for o in &mut x.observations {
            if o.attempt.id == x.attempts[0].immutable_id {
                o.attempt.digest = x.attempts[0].digest.0.clone();
            }
        }
        assert_eq!(x.validate(), Err(ManifestError::InvalidReference));

        let mut x = envelope();
        let tool = artifact(ArtifactKind::Toolchain, "other-toolchain");
        x.attempts[0].overrides = vec![
            AttemptOverride {
                kind: OverrideKind::Toolchain,
                artifact: tool.clone(),
            },
            AttemptOverride {
                kind: OverrideKind::Toolchain,
                artifact: tool,
            },
        ];
        x.attempts[0].digest = x.attempts[0].recompute_digest().unwrap();
        for o in &mut x.observations {
            if o.attempt.id == x.attempts[0].immutable_id {
                o.attempt.digest = x.attempts[0].digest.0.clone();
            }
        }
        assert_eq!(x.validate(), Err(ManifestError::InvalidReference));
    }

    #[test]
    fn enforces_input_override_cardinality_and_unique_locators() {
        let mut x = envelope();
        x.attempts[0].overrides = (0..=MAX_ATTEMPT_INPUT_OVERRIDES)
            .map(|n| AttemptOverride {
                kind: OverrideKind::Input,
                artifact: artifact(ArtifactKind::Input, &format!("input-{n}")),
            })
            .collect();
        x.attempts[0].digest = x.attempts[0].recompute_digest().unwrap();
        for o in &mut x.observations {
            if o.attempt.id == x.attempts[0].immutable_id {
                o.attempt.digest = x.attempts[0].digest.0.clone();
            }
        }
        assert_eq!(x.validate(), Err(ManifestError::InvalidBound));

        let mut x = envelope();
        let input = artifact(ArtifactKind::Input, "repeat");
        x.attempts[0].overrides = vec![
            AttemptOverride {
                kind: OverrideKind::Input,
                artifact: input.clone(),
            },
            AttemptOverride {
                kind: OverrideKind::Input,
                artifact: input,
            },
        ];
        x.attempts[0].digest = x.attempts[0].recompute_digest().unwrap();
        for o in &mut x.observations {
            if o.attempt.id == x.attempts[0].immutable_id {
                o.attempt.digest = x.attempts[0].digest.0.clone();
            }
        }
        assert_eq!(x.validate(), Err(ManifestError::InvalidReference));
    }

    #[test]
    fn exact_hash_roles_match_unicode_newline_known_answers() {
        let original = b"{\"label\":\"caf\\u00e9\"}\n";
        let canonical = "{\"label\":\"café\"}".as_bytes();
        let original_claim = OriginalArtifactDigest(DigestClaim {
            algorithm: DigestAlgorithm::Sha256,
            representation: DigestRepresentation::ExactBytes,
            hex: "426d8f9948cd8aa9aede8235c65e8f5fd79eb048055e962afbea5c2b5ca1b64d".into(),
        });
        let record_claim = CanonicalRecordDigest(DigestClaim {
            algorithm: DigestAlgorithm::Sha256,
            representation: DigestRepresentation::ExactBytes,
            hex: "2d7fba14f0a7cffed454bc268d791c0f32d11f62946998a323087ccf5df29075".into(),
        });
        assert_eq!(original_claim.verify_exact_bytes(original), Ok(()));
        assert_eq!(record_claim.verify_exact_bytes(canonical), Ok(()));
        assert_eq!(
            original_claim.verify_exact_bytes(canonical),
            Err(ManifestError::InvalidDigest)
        );
        assert_eq!(
            record_claim.verify_exact_bytes(original),
            Err(ManifestError::InvalidDigest)
        );
        assert_ne!(original_claim.0.hex, record_claim.0.hex);
    }

    #[test]
    fn refuses_oversized_escaped_envelope_with_capped_writer() {
        let mut x = envelope();
        let a = &x.attempts[0];
        let time = Disclosure::Available(ProvenancedTime {
            unix_ms: 7,
            clock: ClockProvenance {
                clock_id: "c".repeat(128),
                source: "\n".repeat(256),
                uncertainty_ms: None,
            },
        });
        x.observations = (0..MAX_OBSERVATIONS)
            .map(|n| ObservationReference {
                record_id: format!("record-{n}"),
                attempt: ImmutableReference {
                    id: a.immutable_id.clone(),
                    digest: a.digest.0.clone(),
                },
                original_artifact_digest: Disclosure::Unknown,
                canonical_record_digest: CanonicalRecordDigest(DigestClaim {
                    algorithm: DigestAlgorithm::Sha256,
                    representation: DigestRepresentation::ExactBytes,
                    hex: hex_lower(&Sha256::digest(b"row")),
                }),
                physical_file_digest: Disclosure::Unknown,
                source_time: time.clone(),
                observed_time: time.clone(),
                custody_time: time.clone(),
                verified_time: time.clone(),
            })
            .collect();
        assert!(matches!(x.validate(), Ok(())));
        assert_eq!(x.encode_bounded(), Err(ManifestError::Oversized));
    }

    #[test]
    fn refuses_malformed_json_before_model_validation() {
        assert_eq!(
            ManifestEnvelope::decode_bounded(b"{not-json"),
            Err(ManifestError::InvalidJson)
        );
        let bytes = envelope().encode_bounded().unwrap();
        let tail = std::str::from_utf8(&bytes)
            .unwrap()
            .strip_prefix("{\"schema_version\":1,")
            .unwrap();
        let duplicate = format!("{{\"schema_version\":1,\"schema_version\":1,{tail}");
        assert_eq!(
            ManifestEnvelope::decode_bounded(duplicate.as_bytes()),
            Err(ManifestError::InvalidJson)
        );
    }
}
