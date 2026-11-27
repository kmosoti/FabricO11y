use fabric_corpus::{
    Corpus, Kind, MAX_CANONICAL_BYTES, RevisionBody, RevisionDigest, SourceClaim,
    source::{CitationError, CitationV1},
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn revision(corpus: &mut Corpus, name: &str, bytes: &[u8]) -> RevisionDigest {
    corpus
        .admit(RevisionBody {
            object_id: name.into(),
            kind: Kind::Source,
            text: "source claim".into(),
            parents: BTreeSet::new(),
            sources: vec![
                SourceClaim::Captured {
                    capture_id: "capture".into(),
                    artifact_sha256: Sha256::digest(bytes).into(),
                    locator: "file:opaque".into(),
                    parser: "parser-v1".into(),
                },
                SourceClaim::ExternalOnly {
                    locator: "https:external".into(),
                    parser: "external-parser".into(),
                },
            ],
            extensions: BTreeMap::new(),
        })
        .unwrap()
}

#[test]
fn captured_passage_roundtrip_binds_revision_source_and_bytes() {
    let mut corpus = Corpus::default();
    let input = "abé-tail".as_bytes();
    let id = revision(&mut corpus, "one", input);
    // Byte offsets are not required to land on Unicode character boundaries.
    let citation = CitationV1::cite(&corpus, id, 0, 3, 6, Some(input)).unwrap();
    let resolved = citation.resolve(&corpus, Some(input)).unwrap();
    assert_eq!(resolved.bytes, &input[3..6]);
    assert_eq!(resolved.bytes.as_ptr(), input[3..].as_ptr());
    assert_eq!(resolved.parser, "parser-v1");
    assert_eq!(
        resolved.artifact_sha256,
        <[u8; 32]>::from(Sha256::digest(input))
    );
    assert_eq!(
        citation.passage_sha256,
        <[u8; 32]>::from(Sha256::digest(&input[3..6]))
    );
    assert_ne!(citation.revision, resolved.artifact_sha256);
}

#[test]
fn unresolved_wrong_revision_and_changed_bytes_are_distinct() {
    let mut corpus = Corpus::default();
    let input = b"alpha beta";
    let id = revision(&mut corpus, "one", input);
    let other = revision(&mut corpus, "two", b"other data");
    let citation = CitationV1::cite(&corpus, id, 0, 0, 5, Some(input)).unwrap();
    assert_eq!(
        citation.resolve(&corpus, None),
        Err(CitationError::Unavailable)
    );
    assert_eq!(
        citation.resolve(&corpus, Some(b"alpha zeta")),
        Err(CitationError::ArtifactMismatch)
    );
    let mut wrong = citation.clone();
    wrong.revision = other;
    assert_eq!(
        wrong.resolve(&corpus, Some(input)),
        Err(CitationError::ArtifactMismatch)
    );
    wrong.revision = [0xff; 32];
    assert_eq!(
        wrong.resolve(&corpus, Some(input)),
        Err(CitationError::MissingRevision)
    );
    wrong = citation.clone();
    wrong.source_index = 10;
    assert_eq!(
        wrong.resolve(&corpus, Some(input)),
        Err(CitationError::MissingSource)
    );
    wrong.source_index = 1;
    assert_eq!(
        wrong.resolve(&corpus, Some(input)),
        Err(CitationError::ExternalOnly)
    );
    wrong = citation.clone();
    wrong.passage_sha256[0] ^= 1;
    assert_eq!(
        wrong.resolve(&corpus, Some(input)),
        Err(CitationError::PassageMismatch)
    );
    wrong = citation;
    wrong.start = 1;
    assert_eq!(
        wrong.resolve(&corpus, Some(input)),
        Err(CitationError::PassageMismatch)
    );
}

#[test]
fn range_and_entire_artifact_limits_fail_closed() {
    let mut corpus = Corpus::default();
    let input = vec![b'x'; MAX_CANONICAL_BYTES];
    let id = revision(&mut corpus, "bounded", &input);
    let exact = CitationV1::cite(
        &corpus,
        id,
        0,
        (input.len() - 1) as u64,
        input.len() as u64,
        Some(&input),
    )
    .unwrap();
    assert_eq!(exact.resolve(&corpus, Some(&input)).unwrap().bytes, b"x");
    for (start, end) in [
        (0, 0),
        (2, 1),
        (0, u64::MAX),
        (input.len() as u64, input.len() as u64 + 1),
    ] {
        assert_eq!(
            CitationV1::cite(&corpus, id, 0, start, end, Some(&input)),
            Err(CitationError::InvalidRange)
        );
    }
    let oversized = vec![b'x'; MAX_CANONICAL_BYTES + 1];
    assert_eq!(
        exact.resolve(&corpus, Some(&oversized)),
        Err(CitationError::OversizedArtifact)
    );
}
