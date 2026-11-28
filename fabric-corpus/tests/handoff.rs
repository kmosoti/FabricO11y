use fabric_corpus::{
    Corpus, Kind, RevisionBody, SourceClaim,
    as_of::AsOfView,
    differential::Applicability,
    handoff::{Continuation, Declaration, Error, Handoff, Marker, Selection},
    portable::{Availability, AvailabilityClaim, PortableView},
    source::CitationV1,
};
use std::collections::{BTreeMap, BTreeSet};
fn snapshot(c: &Corpus, ids: BTreeSet<[u8; 32]>, current: Availability) -> (Vec<u8>, [u8; 32]) {
    let claims = ids
        .iter()
        .map(|id| {
            (
                *id,
                AvailabilityClaim {
                    archive: Availability::ReferenceOnly,
                    current,
                },
            )
        })
        .collect();
    PortableView::new(
        AsOfView::capture(c, ids, BTreeSet::new(), BTreeSet::new()).unwrap(),
        claims,
    )
    .unwrap()
    .export()
    .unwrap()
}
type Fixture = (Corpus, [u8; 32], (Vec<u8>, [u8; 32]), Declaration);
fn fixture() -> Fixture {
    let mut c = Corpus::default();
    let root = c
        .admit(RevisionBody {
            object_id: "root".into(),
            kind: Kind::Observation,
            text: "claim".into(),
            parents: BTreeSet::new(),
            sources: vec![SourceClaim::ExternalOnly {
                locator: "https:external".into(),
                parser: "none".into(),
            }],
            extensions: BTreeMap::new(),
        })
        .unwrap();
    let missing = [8; 32];
    let basis = snapshot(
        &c,
        BTreeSet::from([root, missing]),
        Availability::ReferenceOnly,
    );
    let d = Declaration {
        basis_digest: basis.1,
        root,
        policy_id: [3; 32],
        revisions: BTreeSet::from([root, missing]),
        relations: BTreeSet::new(),
        citations: vec![CitationV1 {
            revision: root,
            source_index: 0,
            start: 0,
            end: 4,
            passage_sha256: [7; 32],
        }],
        evidence_omission: Marker::Omitted,
        dissent: Marker::Unknown,
        continuation: Continuation::Unresolved,
    };
    (c, root, basis, d)
}
#[test]
fn fresh_consumer_recovers_declarations_and_selected_states() {
    let (c, root, basis, d) = fixture();
    let h = Handoff::create(d.clone(), &basis.0, root, [3; 32], basis.1).unwrap();
    let consumer =
        Handoff::import(h.bytes(), h.digest(), &basis.0, root, [3; 32], basis.1).unwrap();
    assert_eq!(consumer.declaration(), &d);
    let recreated = PortableView::import(&basis.0, basis.1)
        .unwrap()
        .export()
        .unwrap();
    assert_eq!(recreated, basis);
    assert!(matches!(
        consumer.revision_selection(&basis.0, &root),
        Ok(Selection::Present(_))
    ));
    assert!(matches!(
        consumer.revision_selection(&basis.0, &[8; 32]),
        Ok(Selection::Missing(_))
    ));
    assert_eq!(
        consumer.revision_selection(&basis.0, &[9; 32]),
        Err(Error::Omitted)
    );
    assert_eq!(
        consumer.compare_current(&basis.0, &basis.0, basis.1, root, [3; 32]),
        Ok(Applicability::Unknown)
    );
    let only_root = snapshot(&c, BTreeSet::from([root]), Availability::Missing);
    assert_eq!(
        consumer.compare_current(&basis.0, &only_root.0, only_root.1, root, [3; 32]),
        Ok(Applicability::Stale)
    );
    assert_eq!(consumer.declaration().dissent, Marker::Unknown);
    assert_eq!(consumer.declaration().evidence_omission, Marker::Omitted);
    assert_eq!(
        consumer.declaration().continuation,
        Continuation::Unresolved
    );
}
#[test]
fn rejects_mismatches_tampering_and_foreign_or_unselected_claims() {
    let (_, root, basis, d) = fixture();
    let h = Handoff::create(d.clone(), &basis.0, root, [3; 32], basis.1).unwrap();
    assert_eq!(
        Handoff::import(h.bytes(), h.digest(), &basis.0, root, [4; 32], basis.1),
        Err(Error::PolicyMismatch)
    );
    assert_eq!(
        Handoff::import(h.bytes(), h.digest(), &basis.0, [9; 32], [3; 32], basis.1),
        Err(Error::RootMismatch)
    );
    assert_eq!(
        Handoff::import(h.bytes(), h.digest(), &basis.0, root, [3; 32], [9; 32]),
        Err(Error::BasisMismatch)
    );
    let mut tampered = h.bytes().to_vec();
    tampered[10] ^= 1;
    assert_eq!(
        Handoff::import(&tampered, h.digest(), &basis.0, root, [3; 32], basis.1),
        Err(Error::Digest)
    );
    let mut bad_basis = basis.0.clone();
    bad_basis[0] ^= 1;
    assert_eq!(
        Handoff::import(h.bytes(), h.digest(), &bad_basis, root, [3; 32], basis.1),
        Err(Error::Portable)
    );
    let mut foreign = d.clone();
    foreign.revisions.insert([9; 32]);
    assert_eq!(
        Handoff::create(foreign, &basis.0, root, [3; 32], basis.1),
        Err(Error::Omitted)
    );
    let mut bad = d;
    bad.citations[0].revision = [8; 32];
    assert_eq!(
        Handoff::create(bad, &basis.0, root, [3; 32], basis.1),
        Err(Error::Citation)
    );
}
#[test]
fn canonical_bounds_and_duplicate_citations_are_refused() {
    let (_, root, basis, d) = fixture();
    let mut duplicate = d.clone();
    duplicate.citations.push(duplicate.citations[0].clone());
    assert_eq!(
        Handoff::create(duplicate, &basis.0, root, [3; 32], basis.1),
        Err(Error::Invalid)
    );
    let mut oversized = d;
    oversized.citations =
        vec![oversized.citations[0].clone(); fabric_corpus::handoff::MAX_CITATIONS + 1];
    assert_eq!(
        Handoff::create(oversized, &basis.0, root, [3; 32], basis.1),
        Err(Error::Limit)
    );
}
