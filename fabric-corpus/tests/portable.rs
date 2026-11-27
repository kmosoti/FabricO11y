use fabric_corpus::as_of::{AsOfView, SupportGroup};
use fabric_corpus::portable::{
    Availability, AvailabilityClaim, MAX_PORTABLE_BYTES, PortableError, PortableView,
};
use fabric_corpus::{
    Corpus, Kind, Predicate, Relation, RelationInsertion, RevisionBody, RevisionDigest,
    SourceClaim, Value,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn body(id: &str, kind: Kind) -> RevisionBody {
    RevisionBody {
        object_id: id.into(),
        kind,
        text: format!("text-{id}"),
        parents: BTreeSet::new(),
        sources: vec![SourceClaim::Captured {
            capture_id: "capture".into(),
            artifact_sha256: [9; 32],
            locator: "file:claim".into(),
            parser: "parser".into(),
        }],
        extensions: BTreeMap::from([(
            "nested".into(),
            Value::Array(vec![
                Value::Null,
                Value::Bool(false),
                Value::Bool(true),
                Value::Object(BTreeMap::from([(
                    "key".into(),
                    Value::Text("value".into()),
                )])),
            ]),
        )]),
    }
}
fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn claim(archive: Availability, current: Availability) -> AvailabilityClaim {
    AvailabilityClaim { archive, current }
}
fn fixture() -> PortableView {
    let mut corpus = Corpus::default();
    let premise = corpus
        .admit(body(
            "premise",
            Kind::Custom {
                namespace: "x".into(),
                name: "kind".into(),
            },
        ))
        .unwrap();
    let target = corpus.admit(body("target", Kind::Decision)).unwrap();
    let dissent = corpus.admit(body("dissent", Kind::Dissent)).unwrap();
    let edge = Relation {
        subject: premise,
        predicate: Predicate::Supports,
        object: target,
        attributed_to: "author".into(),
        extensions: BTreeMap::new(),
    };
    let edge = match corpus.add_relation(edge).unwrap() {
        RelationInsertion::Inserted(id) => id,
        _ => unreachable!(),
    };
    let missing = [255; 32];
    let missing_edge = [254; 32];
    let groups = BTreeSet::from([SupportGroup {
        conclusion: target,
        attributed_to: "group-author".into(),
        premises: BTreeMap::from([(premise, edge), (missing, missing_edge)]),
    }]);
    let view = AsOfView::capture(
        &corpus,
        BTreeSet::from([premise, target, dissent, missing]),
        BTreeSet::from([edge, missing_edge]),
        groups,
    )
    .unwrap();
    let availability = BTreeMap::from([
        (premise, claim(Availability::Exact, Availability::Aggregate)),
        (
            target,
            claim(Availability::Aggregate, Availability::ReferenceOnly),
        ),
        (
            dissent,
            claim(Availability::ReferenceOnly, Availability::Missing),
        ),
        (missing, claim(Availability::Missing, Availability::Exact)),
    ]);
    PortableView::new(view, availability).unwrap()
}
#[test]
fn deterministic_roundtrip_preserves_missing_provenance_and_availability() {
    let expected = fixture();
    let (bytes, digest) = expected.export().unwrap();
    let decoded = PortableView::import(&bytes, digest).unwrap();
    assert_eq!(decoded, expected);
    assert_eq!(decoded.export().unwrap(), (bytes, digest));
    assert_eq!(decoded.view().revision(&[255; 32]), Some(&None));
    assert_eq!(decoded.view().relation(&[254; 32]), Some(&None));
    assert!(
        decoded
            .availability()
            .values()
            .any(|a| a.archive == Availability::Exact)
    );
    assert!(
        decoded
            .availability()
            .values()
            .any(|a| a.archive == Availability::Aggregate)
    );
    assert!(
        decoded
            .availability()
            .values()
            .any(|a| a.archive == Availability::ReferenceOnly)
    );
    assert!(
        decoded
            .availability()
            .values()
            .any(|a| a.archive == Availability::Missing)
    );
    assert_eq!(
        decoded.view().groups().keys().next().unwrap().attributed_to,
        "group-author"
    );
    assert!(
        !decoded
            .view()
            .groups()
            .values()
            .next()
            .unwrap()
            .is_resolved()
    );
}
#[test]
fn altered_records_membership_and_noncanonical_order_fail() {
    let (bytes, digest) = fixture().export().unwrap();
    let mut changed = bytes.clone();
    let position = changed
        .windows(b"text-premise".len())
        .position(|w| w == b"text-premise")
        .unwrap();
    changed[position] ^= 1;
    assert!(matches!(
        PortableView::import(&changed, digest),
        Err(PortableError::DigestMismatch)
    ));
    assert!(matches!(
        PortableView::import(&changed, hash(&changed)),
        Err(PortableError::DigestMismatch)
    ));
    let mut changed_membership = bytes.clone();
    let header_len = b"fabric-corpus/view\0\x01".len();
    changed_membership[header_len + 8] ^= 1;
    assert!(PortableView::import(&changed_membership, hash(&changed_membership)).is_err());
    let mut truncated = bytes.clone();
    truncated.pop();
    assert!(PortableView::import(&truncated, hash(&truncated)).is_err());
    assert!(matches!(
        PortableView::import(&vec![0; MAX_PORTABLE_BYTES + 1], [0; 32]),
        Err(PortableError::Limit)
    ));
}
#[test]
fn reordered_or_duplicate_missing_membership_is_rejected_even_with_new_digest() {
    let corpus = Corpus::default();
    let ids: BTreeSet<RevisionDigest> = [[1; 32], [2; 32]].into_iter().collect();
    let view = AsOfView::capture(&corpus, ids.clone(), BTreeSet::new(), BTreeSet::new()).unwrap();
    let claims = ids
        .into_iter()
        .map(|id| (id, claim(Availability::Missing, Availability::Missing)))
        .collect();
    let (mut bytes, _) = PortableView::new(view, claims).unwrap().export().unwrap();
    let offset = b"fabric-corpus/view\0\x01".len() + 8;
    let first = bytes[offset..offset + 35].to_vec();
    let second = bytes[offset + 35..offset + 70].to_vec();
    bytes[offset..offset + 35].copy_from_slice(&second);
    bytes[offset + 35..offset + 70].copy_from_slice(&first);
    assert!(matches!(
        PortableView::import(&bytes, hash(&bytes)),
        Err(PortableError::Invalid)
    ));
    bytes[offset + 35..offset + 70].copy_from_slice(&second);
    assert!(matches!(
        PortableView::import(&bytes, hash(&bytes)),
        Err(PortableError::Invalid)
    ));
}
