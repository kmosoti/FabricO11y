use fabric_corpus::{
    Corpus, Kind, RevisionBody, Value,
    as_of::AsOfView,
    investigation_proposal::{
        self as proposal, Alternative, DuplicateCheck, Error, Extension, Input, Proposal,
    },
    portable::{Availability, AvailabilityClaim, PortableError, PortableView},
};
use std::collections::{BTreeMap, BTreeSet};
fn body(id: &str) -> RevisionBody {
    RevisionBody {
        object_id: id.into(),
        kind: Kind::Observation,
        text: id.into(),
        parents: BTreeSet::new(),
        sources: vec![],
        extensions: BTreeMap::new(),
    }
}
type Basis = (Vec<u8>, [u8; 32], [u8; 32], [u8; 32], [u8; 32]);
fn basis() -> Basis {
    let mut c = Corpus::default();
    let a = c.admit(body("a")).unwrap();
    let b = c.admit(body("b")).unwrap();
    let missing = body("missing").digest().unwrap();
    let ids = BTreeSet::from([a, b, missing]);
    let availability = ids
        .iter()
        .map(|id| {
            (
                *id,
                AvailabilityClaim {
                    archive: Availability::Missing,
                    current: Availability::Missing,
                },
            )
        })
        .collect();
    let (bytes, digest) = PortableView::new(
        AsOfView::capture(&c, ids, BTreeSet::new(), BTreeSet::new()).unwrap(),
        availability,
    )
    .unwrap()
    .export()
    .unwrap();
    (bytes, digest, a, b, missing)
}
fn input(digest: [u8; 32], a: [u8; 32], b: [u8; 32]) -> Input {
    Input {
        version: 1,
        proposal_id: "p1".into(),
        basis_digest: digest,
        anchors: vec![a, b],
        alternatives: vec![
            Alternative {
                id: "a".into(),
                description: "first possibility".into(),
            },
            Alternative {
                id: "b".into(),
                description: "second possibility".into(),
            },
        ],
        discriminating_test: "Compare measured outcomes".into(),
        duplicate_check: DuplicateCheck::Unknown,
        extensions: vec![Extension {
            namespace: "example.test".into(),
            name: "note".into(),
            value: Value::Text("untrusted".into()),
        }],
    }
}
#[test]
fn deterministic_immutable_envelope_and_batch_identity() {
    let (bytes, digest, a, b, _) = basis();
    let spec = input(digest, a, b);
    let p = Proposal::new(&bytes, spec.clone()).unwrap();
    let mut reordered = spec;
    reordered.anchors.reverse();
    reordered.alternatives.reverse();
    let same = Proposal::new(&bytes, reordered).unwrap();
    assert_eq!(p, same);
    assert_eq!(p.digest(), same.digest());
    assert!(!p.canonical_bytes().is_empty());
    assert_eq!(
        proposal::check_unique(&[p.clone(), same]),
        Err(Error::DuplicateIdentity)
    );
    let mut another = input(digest, a, b);
    another.proposal_id = "p2".into();
    another.duplicate_check = DuplicateCheck::PossibleMatch {
        proposal_id: "p1".into(),
    };
    let q = Proposal::new(&bytes, another).unwrap();
    assert_ne!(p.digest(), q.digest());
    assert_eq!(proposal::check_unique(&[p, q]), Ok(()));
}
#[test]
fn anchors_and_basis_are_checked() {
    let (bytes, digest, a, b, missing) = basis();
    let mut i = input(digest, a, b);
    i.anchors = vec![missing];
    assert_eq!(Proposal::new(&bytes, i), Err(Error::AnchorMissing));
    let mut i = input(digest, a, b);
    i.anchors = vec![[0; 32]];
    assert_eq!(Proposal::new(&bytes, i), Err(Error::AnchorOmitted));
    let mut tampered = bytes.clone();
    tampered[0] ^= 1;
    assert_eq!(
        Proposal::new(&tampered, input(digest, a, b)),
        Err(Error::Portable(PortableError::DigestMismatch))
    );
    let mut i = input(digest, a, b);
    i.basis_digest = [1; 32];
    assert_eq!(
        Proposal::new(&bytes, i),
        Err(Error::Portable(PortableError::DigestMismatch))
    );
}
#[test]
fn malformed_duplicates_and_bounds_fail_closed() {
    let (bytes, digest, a, b, _) = basis();
    let mut i = input(digest, a, b);
    i.version = 2;
    assert_eq!(Proposal::new(&bytes, i), Err(Error::Version));
    let mut i = input(digest, a, b);
    i.anchors = vec![a, a];
    assert_eq!(Proposal::new(&bytes, i), Err(Error::DuplicateIdentity));
    let mut i = input(digest, a, b);
    i.alternatives[1].id = i.alternatives[0].id.clone();
    assert_eq!(Proposal::new(&bytes, i), Err(Error::DuplicateIdentity));
    let mut i = input(digest, a, b);
    i.alternatives[1].description = i.alternatives[0].description.clone();
    assert_eq!(Proposal::new(&bytes, i), Err(Error::DuplicateIdentity));
    let mut i = input(digest, a, b);
    i.alternatives.pop();
    assert_eq!(Proposal::new(&bytes, i), Err(Error::Limit));
    let mut i = input(digest, a, b);
    i.discriminating_test = " ".into();
    assert_eq!(Proposal::new(&bytes, i), Err(Error::Invalid));
    let mut i = input(digest, a, b);
    i.extensions.push(i.extensions[0].clone());
    assert_eq!(Proposal::new(&bytes, i), Err(Error::DuplicateIdentity));
    let mut i = input(digest, a, b);
    i.extensions[0].namespace = "invalid scope".into();
    assert_eq!(Proposal::new(&bytes, i), Err(Error::Invalid));
    let mut i = input(digest, a, b);
    i.extensions[0].value = Value::Text("x".repeat(proposal::MAX_PROPOSAL_BYTES));
    assert_eq!(Proposal::new(&bytes, i), Err(Error::Limit));
}
