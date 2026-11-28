//! Caller-owned local comparison; neither a digest nor readback grants authority.
use fabric_corpus::{
    Corpus, Kind, RevisionBody, SourceClaim,
    as_of::AsOfView,
    portable::{Availability, AvailabilityClaim, PortableView},
};
use fabric_store::restored_evidence::{
    ExpectedRestore, Manifest, Profile, RestoreError, verify_restore_files,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    path::PathBuf,
};
const ARTIFACT: [u8; 32] = [
    0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
    0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
];
const MANIFEST: [u8; 32] = [
    0x5d, 0x42, 0x08, 0xf3, 0x5d, 0xfc, 0x66, 0xf1, 0xca, 0x68, 0x8b, 0xd8, 0xbf, 0x25, 0xe8, 0xe7,
    0xb8, 0x25, 0x65, 0xd5, 0xe6, 0x6f, 0xc3, 0x7e, 0x42, 0xea, 0xaf, 0xd4, 0x22, 0xff, 0xa3, 0xdf,
];
#[test]
fn frozen_missing_claim_and_independently_checked_local_read_are_distinct() {
    let expected = ExpectedRestore {
        authority: "fixture-authority".into(),
        origin: "fixture-origin".into(),
        candidate_id: 7,
        manifest_digest: MANIFEST,
        artifact_digest: ARTIFACT,
    };
    let descriptor = Manifest {
        version: 1,
        authority: "fixture-authority".into(),
        origin: "fixture-origin".into(),
        candidate_id: 7,
        profile: Profile::Exact,
        artifact_len: 3,
        artifact_digest: ARTIFACT,
    };
    let descriptor_bytes = serde_json::to_vec(&descriptor).unwrap();
    let mut corpus = Corpus::default();
    let id = corpus
        .admit(RevisionBody {
            object_id: "captured-source".into(),
            kind: Kind::Source,
            text: "observation".into(),
            parents: BTreeSet::new(),
            sources: vec![SourceClaim::Captured {
                capture_id: "fixture-capture".into(),
                artifact_sha256: ARTIFACT,
                locator: "urn:fixture".into(),
                parser: "fixture-parser".into(),
            }],
            extensions: BTreeMap::new(),
        })
        .unwrap();
    let view = AsOfView::capture(
        &corpus,
        BTreeSet::from([id]),
        BTreeSet::new(),
        BTreeSet::new(),
    )
    .unwrap();
    let claim = AvailabilityClaim {
        archive: Availability::Missing,
        current: Availability::Missing,
    };
    let (bytes, digest) = PortableView::new(view, BTreeMap::from([(id, claim)]))
        .unwrap()
        .export()
        .unwrap();
    let frozen = PortableView::import(&bytes, digest).unwrap();
    assert_eq!(frozen.availability()[&id], claim);
    assert_eq!(
        frozen
            .view()
            .revision(&id)
            .unwrap()
            .as_ref()
            .unwrap()
            .sources[0],
        SourceClaim::Captured {
            capture_id: "fixture-capture".into(),
            artifact_sha256: expected.artifact_digest,
            locator: "urn:fixture".into(),
            parser: "fixture-parser".into()
        }
    );
    let dir = tempfile::tempdir_in(PathBuf::from(
        std::env::var_os("TMPDIR").expect("disk-backed TMPDIR"),
    ))
    .unwrap();
    let manifest_path = dir.path().join("manifest");
    let artifact_path = dir.path().join("artifact");
    fs::write(&manifest_path, &descriptor_bytes).unwrap();
    let manifest = File::open(&manifest_path).unwrap();
    assert_eq!(
        verify_restore_files(Some(&manifest), None, &expected),
        Err(RestoreError::MissingArtifact)
    );
    fs::write(&artifact_path, b"abc").unwrap();
    let artifact = File::open(&artifact_path).unwrap();
    assert!(verify_restore_files(Some(&manifest), Some(&artifact), &expected).is_ok());
    let mut foreign = expected.clone();
    foreign.origin = "foreign".into();
    assert_eq!(
        verify_restore_files(Some(&manifest), Some(&artifact), &foreign),
        Err(RestoreError::Identity)
    );
    drop(artifact);
    fs::write(&artifact_path, b"abd").unwrap();
    let tampered = File::open(&artifact_path).unwrap();
    assert_eq!(
        verify_restore_files(Some(&manifest), Some(&tampered), &expected),
        Err(RestoreError::ArtifactDigest)
    );
    drop(tampered);
    fs::remove_file(&artifact_path).unwrap();
    assert_eq!(
        File::open(&artifact_path).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    assert_eq!(
        verify_restore_files(Some(&manifest), None, &expected),
        Err(RestoreError::MissingArtifact)
    );
    assert_eq!(PortableView::import(&bytes, digest).unwrap(), frozen);
    assert_eq!(frozen.availability()[&id], claim);
}
