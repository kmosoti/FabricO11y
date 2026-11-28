use fabric_corpus::{
    Corpus, Kind, Predicate, Relation, RelationInsertion, RevisionBody, RevisionDigest,
    SourceClaim,
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

#[test]
fn declared_terminal_sources_group_by_digest_without_independence_claims() {
    use investigation::{SourceKind, TerminalState};
    let mut c = Corpus::default();
    let captured = |digest| SourceClaim::Captured {
        capture_id: "capture".into(),
        artifact_sha256: digest,
        locator: "file:fixture".into(),
        parser: "parser-v1".into(),
    };
    let admit = |c: &mut Corpus, name: &str, sources| {
        let mut revision = body(name);
        revision.sources = sources;
        c.admit(revision).unwrap()
    };
    let a = admit(&mut c, "a", vec![captured([1; 32])]);
    let b = admit(&mut c, "b", vec![captured([1; 32])]);
    let distinct = admit(&mut c, "distinct", vec![captured([2; 32])]);
    let external = admit(
        &mut c,
        "external",
        vec![SourceClaim::ExternalOnly {
            locator: "https://example.org/".into(),
            parser: "parser-v1".into(),
        }],
    );
    let unknown = admit(&mut c, "no-source", vec![]);
    let cycle_a = admit(&mut c, "cycle-a", vec![]);
    let cycle_b = admit(&mut c, "cycle-b", vec![]);
    let missing = body("missing").digest().unwrap();
    let omitted = body("omitted").digest().unwrap();
    let ab = edge(&mut c, cycle_b, cycle_a, "one");
    let ba = edge(&mut c, cycle_a, cycle_b, "two");
    let groups = BTreeSet::from([
        SupportGroup {
            conclusion: cycle_a,
            attributed_to: "one".into(),
            premises: BTreeMap::from([(cycle_b, ab)]),
        },
        SupportGroup {
            conclusion: cycle_b,
            attributed_to: "two".into(),
            premises: BTreeMap::from([(cycle_a, ba)]),
        },
    ]);
    let ids = BTreeSet::from([a, b, distinct, external, unknown, cycle_a, cycle_b, missing]);
    let mut availability: BTreeMap<_, _> = ids
        .iter()
        .map(|id| {
            (
                *id,
                AvailabilityClaim {
                    archive: Availability::ReferenceOnly,
                    current: Availability::Missing,
                },
            )
        })
        .collect();
    availability.get_mut(&a).unwrap().current = Availability::Exact;
    let view = AsOfView::capture(&c, ids, BTreeSet::from([ab, ba]), groups).unwrap();
    let (bytes, digest) = PortableView::new(view, availability)
        .unwrap()
        .export()
        .unwrap();
    let anchors = BTreeSet::from([a, b, distinct, external, unknown, cycle_a]);
    let selected = BTreeSet::from([a, b, distinct, external, unknown, cycle_a, missing, omitted]);
    let result = investigation::project_with_sources(
        &bytes,
        digest,
        anchors.clone(),
        selected.clone(),
        Limits::default(),
    )
    .unwrap();
    assert_eq!(
        result.frontier,
        investigation::project(&bytes, digest, anchors.clone(), Limits::default()).unwrap()
    );
    assert!(!result.frontier.cycle_edges.is_empty());
    assert_eq!(
        result.captured[&[1; 32]]
            .iter()
            .map(|reference| (reference.revision, reference.source_index))
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([(a, 0), (b, 0)])
    );
    assert_eq!(result.captured[&[2; 32]].len(), 1);
    assert_eq!(
        result.terminals[&b].availability.unwrap().current,
        Availability::Missing
    );
    assert_eq!(
        result.terminals[&external].claims[0].kind,
        SourceKind::ExternalOnly
    );
    assert!(result.terminals[&unknown].claims.is_empty());
    assert_eq!(
        result.terminals[&cycle_a].state,
        TerminalState::NotObservedTerminal
    );
    assert_eq!(result.terminals[&missing].state, TerminalState::Missing);
    assert_eq!(result.terminals[&omitted].state, TerminalState::Omitted);
    let limited = investigation::project_with_sources(
        &bytes,
        digest,
        anchors,
        selected,
        Limits {
            work: 0,
            ..Limits::default()
        },
    )
    .unwrap();
    assert!(limited.frontier.truncated);
    assert_eq!(
        limited.terminals[&a].state,
        TerminalState::NotObservedTerminal
    );
    assert!(limited.captured.is_empty());
    let mut corrupt = bytes.clone();
    corrupt[0] ^= 1;
    assert_eq!(
        investigation::project_with_sources(
            &corrupt,
            digest,
            BTreeSet::new(),
            BTreeSet::new(),
            Limits::default()
        ),
        Err(Error::Portable(PortableError::DigestMismatch))
    );
}

#[test]
fn declared_source_cap_refuses_atomically() {
    let mut c = Corpus::default();
    let mut revision = body("many-sources");
    revision.sources = vec![
        SourceClaim::ExternalOnly {
            locator: "file:fixture".into(),
            parser: "parser-v1".into()
        };
        investigation::MAX_TERMINAL_SOURCE_CLAIMS + 1
    ];
    let id = c.admit(revision).unwrap();
    let (bytes, digest) = encoded(&c, BTreeSet::from([id]), BTreeSet::new(), BTreeSet::new());
    assert_eq!(
        investigation::project_with_sources(
            &bytes,
            digest,
            BTreeSet::from([id]),
            BTreeSet::from([id]),
            Limits::default()
        ),
        Err(Error::Limits)
    );
}
