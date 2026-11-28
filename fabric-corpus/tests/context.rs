use fabric_corpus::{
    Corpus, Kind, Predicate, Relation, RelationInsertion, RevisionBody, SourceClaim,
    as_of::AsOfView,
    context::{
        CitationInput, ContextError, MAX_CONTEXT_CITATIONS, MAX_CONTEXT_PASSAGE_BYTES,
        MAX_CONTEXT_SCAN_BYTES, PassageState, assemble,
    },
    source::{CitationError, CitationV1},
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn fixture() -> (AsOfView, CitationV1, [u8; 32]) {
    let mut corpus = Corpus::default();
    let id = corpus
        .admit(RevisionBody {
            object_id: "source".into(),
            kind: Kind::Source,
            text: "claim".into(),
            parents: BTreeSet::new(),
            sources: vec![
                SourceClaim::Captured {
                    capture_id: "c".into(),
                    artifact_sha256: Sha256::digest(b"alpha beta").into(),
                    locator: "urn:source".into(),
                    parser: "p".into(),
                },
                SourceClaim::ExternalOnly {
                    locator: "urn:external".into(),
                    parser: "p".into(),
                },
            ],
            extensions: BTreeMap::new(),
        })
        .unwrap();
    let dissent = corpus
        .admit(RevisionBody {
            object_id: "dissent".into(),
            kind: Kind::Dissent,
            text: "disagree".into(),
            parents: BTreeSet::new(),
            sources: vec![],
            extensions: BTreeMap::new(),
        })
        .unwrap();
    let relation = Relation {
        subject: dissent,
        predicate: Predicate::Falsifies,
        object: id,
        attributed_to: "reviewer".into(),
        extensions: BTreeMap::new(),
    };
    let RelationInsertion::Inserted(relation_id) = corpus.add_relation(relation).unwrap() else {
        panic!("new relation")
    };
    let citation = CitationV1::cite(&corpus, id, 0, 0, 5, Some(b"alpha beta")).unwrap();
    let view = AsOfView::capture(
        &corpus,
        BTreeSet::from([id, dissent, [9; 32]]),
        BTreeSet::from([relation_id]),
        BTreeSet::new(),
    )
    .unwrap();
    (view, citation, dissent)
}
#[test]
fn verified_omitted_missing_and_dissent_remain_distinct() {
    let (view, citation, dissent) = fixture();
    let mut missing = citation.clone();
    missing.revision = [9; 32];
    let mut external = citation.clone();
    external.source_index = 1;
    let inputs = [
        CitationInput {
            citation: &citation,
            bytes: Some(b"alpha beta"),
        },
        CitationInput {
            citation: &citation,
            bytes: None,
        },
        CitationInput {
            citation: &missing,
            bytes: None,
        },
        CitationInput {
            citation: &external,
            bytes: None,
        },
    ];
    let context = assemble(&view, &inputs).unwrap();
    assert!(
        matches!(context.entries()[0].state(), PassageState::Verified(p) if p.bytes == b"alpha" && p.parser == "p")
    );
    assert_eq!(context.entries()[1].state(), &PassageState::Omitted);
    assert_eq!(context.entries()[2].state(), &PassageState::MissingRevision);
    assert_eq!(context.entries()[3].state(), &PassageState::ExternalOnly);
    assert_eq!(
        context
            .view()
            .revision(&dissent)
            .unwrap()
            .as_ref()
            .unwrap()
            .kind,
        Kind::Dissent
    );
    assert!(context.view().relations().values().any(|r| {
        r.as_ref()
            .is_some_and(|r| r.predicate == Predicate::Falsifies)
    }));
    assert_eq!(context.supplied_bytes(), 10);
    assert_eq!(context.requested_passage_bytes(), 20);
}
#[test]
fn forged_unselected_missing_index_and_wrong_bytes_fail() {
    let (view, citation, _) = fixture();
    let mut forged = citation.clone();
    forged.passage_sha256[0] ^= 1;
    assert_eq!(
        assemble(
            &view,
            &[CitationInput {
                citation: &forged,
                bytes: Some(b"alpha beta")
            }]
        )
        .unwrap_err(),
        ContextError::Citation(CitationError::PassageMismatch)
    );
    assert_eq!(
        assemble(
            &view,
            &[CitationInput {
                citation: &citation,
                bytes: Some(b"alpha zeta")
            }]
        )
        .unwrap_err(),
        ContextError::Citation(CitationError::ArtifactMismatch)
    );
    forged.revision = [8; 32];
    assert_eq!(
        assemble(
            &view,
            &[CitationInput {
                citation: &forged,
                bytes: None
            }]
        )
        .unwrap_err(),
        ContextError::UnselectedRevision
    );
    forged = citation;
    forged.source_index = 22;
    assert_eq!(
        assemble(
            &view,
            &[CitationInput {
                citation: &forged,
                bytes: None
            }]
        )
        .unwrap_err(),
        ContextError::Citation(CitationError::MissingSource)
    );
}
#[test]
fn aggregate_limits_precede_verification() {
    let (view, citation, _) = fixture();
    let input = CitationInput {
        citation: &citation,
        bytes: None,
    };
    assert!(assemble(&view, &vec![input; MAX_CONTEXT_CITATIONS]).is_ok());
    assert_eq!(
        assemble(&view, &vec![input; MAX_CONTEXT_CITATIONS + 1]).unwrap_err(),
        ContextError::Limit
    );
    let huge = vec![0; MAX_CONTEXT_SCAN_BYTES + 1];
    assert_eq!(
        assemble(
            &view,
            &[CitationInput {
                citation: &citation,
                bytes: Some(&huge)
            }]
        )
        .unwrap_err(),
        ContextError::Limit
    );
    let mut wide = citation;
    wide.end = MAX_CONTEXT_PASSAGE_BYTES + 1;
    assert_eq!(
        assemble(
            &view,
            &[CitationInput {
                citation: &wide,
                bytes: None
            }]
        )
        .unwrap_err(),
        ContextError::Limit
    );
}
