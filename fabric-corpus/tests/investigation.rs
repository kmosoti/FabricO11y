use fabric_corpus::{
    Corpus, Kind, Predicate, Relation, RelationInsertion, RevisionBody, RevisionDigest,
    as_of::{AsOfView, SupportGroup},
    investigation::{self, Error, Limits, Selection},
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
fn edge(
    c: &mut Corpus,
    premise: RevisionDigest,
    conclusion: RevisionDigest,
    author: &str,
) -> RevisionDigest {
    match c
        .add_relation(Relation {
            subject: premise,
            predicate: Predicate::Supports,
            object: conclusion,
            attributed_to: author.into(),
            extensions: BTreeMap::new(),
        })
        .unwrap()
    {
        RelationInsertion::Inserted(id) => id,
        _ => panic!("duplicate"),
    }
}
fn encoded(
    c: &Corpus,
    ids: BTreeSet<RevisionDigest>,
    edges: BTreeSet<RevisionDigest>,
    groups: BTreeSet<SupportGroup>,
) -> (Vec<u8>, [u8; 32]) {
    let availability = ids
        .iter()
        .map(|id| {
            (
                *id,
                AvailabilityClaim {
                    archive: Availability::Exact,
                    current: Availability::Exact,
                },
            )
        })
        .collect();
    PortableView::new(
        AsOfView::capture(c, ids, edges, groups).unwrap(),
        availability,
    )
    .unwrap()
    .export()
    .unwrap()
}
#[test]
fn cycle_without_structural_terminal() {
    let mut c = Corpus::default();
    let a = c.admit(body("a")).unwrap();
    let b = c.admit(body("b")).unwrap();
    let ab = edge(&mut c, a, b, "ab");
    let ba = edge(&mut c, b, a, "ba");
    let groups = BTreeSet::from([
        SupportGroup {
            conclusion: a,
            attributed_to: "alice".into(),
            premises: BTreeMap::from([(b, ba)]),
        },
        SupportGroup {
            conclusion: b,
            attributed_to: "bob".into(),
            premises: BTreeMap::from([(a, ab)]),
        },
    ]);
    let (bytes, digest) = encoded(&c, BTreeSet::from([a, b]), BTreeSet::from([ab, ba]), groups);
    let result =
        investigation::project(&bytes, digest, BTreeSet::from([a]), Limits::default()).unwrap();
    assert!(!result.truncated);
    assert_eq!(result.nodes.len(), 2);
    assert!(!result.cycle_edges.is_empty());
    assert!(result.terminal_anchors.is_empty());
    assert_eq!(
        result,
        investigation::project(&bytes, digest, BTreeSet::from([a]), Limits::default()).unwrap()
    );
}
#[test]
fn shared_terminal_and_alternative_groups_keep_attribution() {
    let mut c = Corpus::default();
    let a = c.admit(body("a")).unwrap();
    let b = c.admit(body("b")).unwrap();
    let root = c.admit(body("root")).unwrap();
    let other = c.admit(body("other")).unwrap();
    let ra = edge(&mut c, root, a, "ra");
    let rb = edge(&mut c, root, b, "rb");
    let oa = edge(&mut c, other, a, "oa");
    let groups = BTreeSet::from([
        SupportGroup {
            conclusion: a,
            attributed_to: "alice".into(),
            premises: BTreeMap::from([(root, ra)]),
        },
        SupportGroup {
            conclusion: a,
            attributed_to: "bob".into(),
            premises: BTreeMap::from([(other, oa)]),
        },
        SupportGroup {
            conclusion: b,
            attributed_to: "carol".into(),
            premises: BTreeMap::from([(root, rb)]),
        },
    ]);
    let (bytes, digest) = encoded(
        &c,
        BTreeSet::from([a, b, root, other]),
        BTreeSet::from([ra, rb, oa]),
        groups,
    );
    let anchors = BTreeSet::from([a, b]);
    let result =
        investigation::project(&bytes, digest, anchors.clone(), Limits::default()).unwrap();
    assert!(!result.truncated);
    assert_eq!(result.nodes[&a].groups.len(), 2);
    assert_eq!(result.terminal_anchors[&root], anchors);
    assert_eq!(result.terminal_anchors[&other], BTreeSet::from([a]));
    assert!(result.cycle_edges.is_empty());
    let restored = PortableView::import(&bytes, digest)
        .unwrap()
        .export()
        .unwrap();
    assert_eq!(
        investigation::project(
            &restored.0,
            restored.1,
            BTreeSet::from([a, b]),
            Limits::default()
        )
        .unwrap(),
        result
    );
}
#[test]
fn omission_missing_and_bounds_are_explicit() {
    let mut c = Corpus::default();
    let a = c.admit(body("a")).unwrap();
    let missing = body("missing").digest().unwrap();
    let omitted = body("omitted").digest().unwrap();
    let absent_edge = [7; 32];
    let omitted_edge = [8; 32];
    let groups = BTreeSet::from([SupportGroup {
        conclusion: a,
        attributed_to: "reviewer".into(),
        premises: BTreeMap::from([(missing, absent_edge), (omitted, omitted_edge)]),
    }]);
    let (bytes, digest) = encoded(
        &c,
        BTreeSet::from([a, missing]),
        BTreeSet::from([absent_edge]),
        groups,
    );
    let r = investigation::project(&bytes, digest, BTreeSet::from([a]), Limits::default()).unwrap();
    assert!(r.nodes[&a].groups[0].enumeration_complete);
    assert!(!r.truncated);
    let premises = &r.nodes[&a].groups[0].premises;
    let m = premises.iter().find(|p| p.revision == missing).unwrap();
    assert_eq!(
        (m.revision_selection, m.relation_selection),
        (Selection::Missing, Selection::Missing)
    );
    let o = premises.iter().find(|p| p.revision == omitted).unwrap();
    assert_eq!(
        (o.revision_selection, o.relation_selection),
        (Selection::Omitted, Selection::Omitted)
    );
    let limited = investigation::project(
        &bytes,
        digest,
        BTreeSet::from([a]),
        Limits {
            edges: 1,
            ..Limits::default()
        },
    )
    .unwrap();
    assert!(limited.truncated);
    assert!(!limited.nodes[&a].groups[0].enumeration_complete);
    assert_eq!(
        limited,
        investigation::project(
            &bytes,
            digest,
            BTreeSet::from([a]),
            Limits {
                edges: 1,
                ..Limits::default()
            }
        )
        .unwrap()
    );
    assert!(
        investigation::project(
            &bytes,
            digest,
            BTreeSet::from([a]),
            Limits {
                work: 0,
                ..Limits::default()
            }
        )
        .unwrap()
        .truncated
    );
    assert_eq!(
        investigation::project(
            &bytes,
            digest,
            BTreeSet::from([a]),
            Limits {
                nodes: investigation::MAX_NODES + 1,
                ..Limits::default()
            }
        ),
        Err(Error::Limits)
    );
    let mut tampered = bytes.clone();
    tampered[0] ^= 1;
    assert_eq!(
        investigation::project(&tampered, digest, BTreeSet::from([a]), Limits::default()),
        Err(Error::Portable(PortableError::DigestMismatch))
    );
}
