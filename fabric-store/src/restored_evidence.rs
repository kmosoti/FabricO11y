//! Bounded verification of already-read, local archive evidence.
//!
//! Trusted callers supply identity and digests independently. This module does
//! not authenticate a backend, retain an artifact, or perform eviction.
use fabric_core::retention::VerifiedArchive;
use serde::{Deserialize, Serialize};

pub const MAX_MANIFEST_BYTES: usize = 4096;
pub const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Exact,
    Aggregate,
}

/// Version-one canonical JSON: serde field order and no insignificant whitespace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u8,
    pub authority: String,
    pub origin: String,
    pub candidate_id: u64,
    pub profile: Profile,
    pub artifact_len: u64,
    pub artifact_digest: [u8; 32],
}

/// Values from a separately trusted authority and expected artifact identity.
#[derive(Clone, Debug)]
pub struct ExpectedRestore {
    pub authority: String,
    pub origin: String,
    pub candidate_id: u64,
    pub manifest_digest: [u8; 32],
    pub artifact_digest: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreError {
    MissingManifest,
    MissingArtifact,
    Size,
    MalformedManifest,
    ManifestDigest,
    Identity,
    UnsupportedProfile,
    ArtifactDigest,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}
fn digest_matches(bytes: &[u8], digest: &[u8; 32]) -> bool {
    let actual = crate::olap::sha256_bytes_hex(bytes);
    let expected: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    actual == expected
}

/// Verify the current restored bytes and only then construct a planner assertion.
pub fn verify_restore(
    manifest_bytes: Option<&[u8]>,
    artifact: Option<&[u8]>,
    expected: &ExpectedRestore,
) -> Result<VerifiedArchive, RestoreError> {
    let manifest_bytes = manifest_bytes.ok_or(RestoreError::MissingManifest)?;
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(RestoreError::Size);
    }
    if !digest_matches(manifest_bytes, &expected.manifest_digest) {
        return Err(RestoreError::ManifestDigest);
    }
    let manifest: Manifest =
        serde_json::from_slice(manifest_bytes).map_err(|_| RestoreError::MalformedManifest)?;
    if manifest.version != 1
        || serde_json::to_vec(&manifest).map_err(|_| RestoreError::MalformedManifest)?
            != manifest_bytes
    {
        return Err(RestoreError::MalformedManifest);
    }
    if !valid_id(&manifest.authority)
        || !valid_id(&manifest.origin)
        || manifest.authority != expected.authority
        || manifest.origin != expected.origin
        || manifest.candidate_id != expected.candidate_id
    {
        return Err(RestoreError::Identity);
    }
    if manifest.profile != Profile::Exact {
        return Err(RestoreError::UnsupportedProfile);
    }
    if manifest.artifact_len == 0 || manifest.artifact_len > MAX_ARTIFACT_BYTES as u64 {
        return Err(RestoreError::Size);
    }
    let artifact = artifact.ok_or(RestoreError::MissingArtifact)?;
    if artifact.len() > MAX_ARTIFACT_BYTES {
        return Err(RestoreError::Size);
    }
    if artifact.len() as u64 != manifest.artifact_len
        || manifest.artifact_digest != expected.artifact_digest
        || !digest_matches(artifact, &expected.artifact_digest)
    {
        return Err(RestoreError::ArtifactDigest);
    }
    Ok(VerifiedArchive {
        id: expected.candidate_id,
        digest: expected.artifact_digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fabric_core::retention::{Budget, Candidate, Decision, Fidelity, plan};
    use std::{fs, path::PathBuf};

    fn digest(bytes: &[u8]) -> [u8; 32] {
        let hex = crate::olap::sha256_bytes_hex(bytes);
        let mut result = [0; 32];
        for (i, byte) in result.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
        }
        result
    }
    fn budget() -> Budget {
        Budget {
            used: 91,
            free: 30,
            quota: 100,
            low: 60,
            high: 90,
            pressure: false,
            audit: 5,
        }
    }
    fn candidate(digest: [u8; 32]) -> Candidate {
        Candidate {
            id: 7,
            digest,
            fidelity: Fidelity::Exact,
            protected: true,
            active_dependency: false,
            bytes: 3,
            priority: 1,
            rewrite_scratch: None,
            archive: None,
        }
    }
    #[test]
    fn local_restore_binds_identity_and_only_exact_verified_bytes_feed_planner() {
        let root = tempfile::tempdir_in(PathBuf::from(
            std::env::var_os("TMPDIR").expect("disk-backed TMPDIR"),
        ))
        .unwrap();
        let archive = root.path().join("archive");
        let restore = root.path().join("restore");
        let manifest_path = root.path().join("manifest");
        let bytes = b"abc";
        let artifact_digest = digest(bytes);
        assert_eq!(
            crate::olap::sha256_bytes_hex(bytes),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let manifest = Manifest {
            version: 1,
            authority: "authority".into(),
            origin: "origin".into(),
            candidate_id: 7,
            profile: Profile::Exact,
            artifact_len: bytes.len() as u64,
            artifact_digest,
        };
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let expected = ExpectedRestore {
            authority: "authority".into(),
            origin: "origin".into(),
            candidate_id: 7,
            manifest_digest: digest(&manifest_bytes),
            artifact_digest,
        };
        fs::write(&archive, bytes).unwrap();
        fs::write(&manifest_path, &manifest_bytes).unwrap();
        fs::copy(&archive, &restore).unwrap();
        let captured_manifest = fs::read(&manifest_path).unwrap();
        let restored = fs::read(&restore).unwrap();
        let mut c = candidate(artifact_digest);
        assert_eq!(plan(budget(), &[c]), Ok(Decision::Backpressure));
        c.archive =
            Some(verify_restore(Some(&captured_manifest), Some(&restored), &expected).unwrap());
        assert_eq!(
            plan(budget(), &[c]),
            Ok(Decision::Evict { id: 7, audit: 5 })
        );
        c.active_dependency = true;
        assert_eq!(plan(budget(), &[c]), Ok(Decision::Backpressure));
        c.active_dependency = false;
        fs::remove_file(&restore).unwrap();
        assert_eq!(
            verify_restore(Some(&captured_manifest), None, &expected),
            Err(RestoreError::MissingArtifact)
        );
        c.archive = verify_restore(Some(&captured_manifest), None, &expected).ok();
        assert_eq!(plan(budget(), &[c]), Ok(Decision::Backpressure));
        assert_eq!(
            verify_restore(None, Some(bytes), &expected),
            Err(RestoreError::MissingManifest)
        );
        assert_eq!(
            verify_restore(Some(&captured_manifest), Some(&bytes[..2]), &expected),
            Err(RestoreError::ArtifactDigest)
        );
        assert_eq!(
            verify_restore(Some(&captured_manifest), Some(b"abd"), &expected),
            Err(RestoreError::ArtifactDigest)
        );
        assert_eq!(
            verify_restore(Some(b"tampered"), Some(bytes), &expected),
            Err(RestoreError::ManifestDigest)
        );
        for change in 0..4 {
            let mut wrong = expected.clone();
            match change {
                0 => wrong.authority = "other".into(),
                1 => wrong.origin = "other".into(),
                2 => wrong.candidate_id = 8,
                _ => wrong.artifact_digest = [1; 32],
            }
            assert!(verify_restore(Some(&captured_manifest), Some(bytes), &wrong).is_err());
        }
        let aggregate_artifact = b"aggregate:3";
        let aggregate = Manifest {
            profile: Profile::Aggregate,
            artifact_len: aggregate_artifact.len() as u64,
            artifact_digest: digest(aggregate_artifact),
            ..manifest.clone()
        };
        assert_ne!(aggregate.artifact_digest, artifact_digest);
        let aggregate_bytes = serde_json::to_vec(&aggregate).unwrap();
        let mut aggregate_expected = expected.clone();
        aggregate_expected.manifest_digest = digest(&aggregate_bytes);
        aggregate_expected.artifact_digest = aggregate.artifact_digest;
        assert_eq!(
            verify_restore(
                Some(&aggregate_bytes),
                Some(aggregate_artifact),
                &aggregate_expected
            ),
            Err(RestoreError::UnsupportedProfile)
        );
        let mut noncanonical = captured_manifest.clone();
        noncanonical.push(b' ');
        let mut noncanonical_expected = expected.clone();
        noncanonical_expected.manifest_digest = digest(&noncanonical);
        assert_eq!(
            verify_restore(Some(&noncanonical), Some(bytes), &noncanonical_expected),
            Err(RestoreError::MalformedManifest)
        );
        assert_eq!(
            verify_restore(
                Some(&vec![0; MAX_MANIFEST_BYTES + 1]),
                Some(bytes),
                &expected
            ),
            Err(RestoreError::Size)
        );
        assert_eq!(
            verify_restore(
                Some(&captured_manifest),
                Some(&vec![0; MAX_ARTIFACT_BYTES + 1]),
                &expected
            ),
            Err(RestoreError::Size)
        );
    }
}
