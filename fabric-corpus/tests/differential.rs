use fabric_corpus::{
    Corpus, Kind, Predicate, Relation, RelationInsertion, RevisionBody,
    as_of::{AsOfView, SupportGroup},
    differential::{
        Applicability, Dependencies, DifferentialError, GroupSelector, RelationState,
        RevisionState, compare,
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
fn snapshot(
    c: &Corpus,
    ids: BTreeSet<[u8; 32]>,
    edges: BTreeSet<[u8; 32]>,
    groups: BTreeSet<SupportGroup>,
    current: Availability,
) -> (Vec<u8>, [u8; 32]) {
    let availability = ids
        .iter()
        .map(|id| {
            (
                *id,
                AvailabilityClaim {
                    archive: Availability::Exact,
                    current,
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
fn narrow_dependencies_survive_unrelated_change_and_fresh_import() {
    let mut c = Corpus::default();
    let a = c.admit(body("a")).unwrap();
    let extra = c.admit(body("extra")).unwrap();
    let from = snapshot(
        &c,
        BTreeSet::from([a]),
        BTreeSet::new(),
        BTreeSet::new(),
        Availability::Exact,
    );
    let to = snapshot(
        &c,
        BTreeSet::from([a, extra]),
        BTreeSet::new(),
        BTreeSet::new(),
        Availability::Exact,
    );
    let deps = Dependencies {
        revisions: BTreeSet::from([a]),
        ..Default::default()
    };
    let original = compare(&from.0, from.1, &to.0, to.1, &deps).unwrap();
    assert_ne!(original.from_digest(), original.to_digest());
    assert_eq!(original.applicability(), Applicability::Unchanged);
    let fresh_from = PortableView::import(&from.0, from.1)
        .unwrap()
        .export()
        .unwrap();
    let fresh_to = PortableView::import(&to.0, to.1).unwrap().export().unwrap();
    assert_eq!(
        compare(&fresh_from.0, fresh_from.1, &fresh_to.0, fresh_to.1, &deps).unwrap(),
        original
    );
    assert_eq!(
        PortableView::import(&from.0, from.1)
            .unwrap()
            .view()
            .revisions()
            .len(),
        1
    );
}
#[test]
fn availability_changes_and_omitted_or_missing_are_distinct() {
    let mut c = Corpus::default();
    let a = c.admit(body("a")).unwrap();
    let unknown = [8; 32];
    let from = snapshot(
        &c,
        BTreeSet::from([a, unknown]),
        BTreeSet::new(),
        BTreeSet::new(),
        Availability::Exact,
    );
    let changed = snapshot(
        &c,
        BTreeSet::from([a]),
        BTreeSet::new(),
        BTreeSet::new(),
        Availability::Missing,
    );
    let deps = Dependencies {
        revisions: BTreeSet::from([a, unknown]),
        relations: BTreeSet::from([[9; 32]]),
        ..Default::default()
    };
    let result = compare(&from.0, from.1, &changed.0, changed.1, &deps).unwrap();
    assert_eq!(result.applicability(), Applicability::Stale);
    assert!(matches!(
        result.revisions()[&unknown].from,
        RevisionState::Missing(_)
    ));
    assert_eq!(result.revisions()[&unknown].to, RevisionState::Omitted);
    assert_eq!(result.relations()[&[9; 32]].from, RelationState::Omitted);
    let only_unknown = Dependencies {
        revisions: BTreeSet::from([unknown]),
        ..Default::default()
    };
    assert_eq!(
        compare(&from.0, from.1, &changed.0, changed.1, &only_unknown)
            .unwrap()
            .applicability(),
        Applicability::Unknown
    );
    let new_id = c.admit(body("new")).unwrap();
    let new_dep = Dependencies {
        revisions: BTreeSet::from([new_id]),
        ..Default::default()
    };
    let new_to = snapshot(
        &c,
        BTreeSet::from([a, new_id]),
        BTreeSet::new(),
        BTreeSet::new(),
        Availability::Exact,
    );
    let result = compare(&from.0, from.1, &new_to.0, new_to.1, &new_dep).unwrap();
    assert_eq!(result.revisions()[&new_id].from, RevisionState::Omitted);
    assert!(matches!(
        result.revisions()[&new_id].to,
        RevisionState::Present(_)
    ));
    assert_eq!(result.applicability(), Applicability::Unknown);
}
#[test]
fn selected_group_changes_and_bad_bytes_are_rejected() {
    let mut c = Corpus::default();
    let a = c.admit(body("a")).unwrap();
    let b = c.admit(body("b")).unwrap();
    let edge = Relation {
        subject: a,
        predicate: Predicate::Supports,
        object: b,
        attributed_to: "alice".into(),
        extensions: BTreeMap::new(),
    };
    let RelationInsertion::Inserted(edge) = c.add_relation(edge).unwrap() else {
        panic!("new edge")
    };
    let group = SupportGroup {
        conclusion: b,
        attributed_to: "reviewer".into(),
        premises: BTreeMap::from([(a, edge)]),
    };
    let ids = BTreeSet::from([a, b]);
    let from = snapshot(
        &c,
        ids.clone(),
        BTreeSet::from([edge]),
        BTreeSet::from([group.clone()]),
        Availability::Exact,
    );
    let to = snapshot(
        &c,
        ids,
        BTreeSet::new(),
        BTreeSet::from([group]),
        Availability::Exact,
    );
    let deps = Dependencies {
        groups: BTreeSet::from([GroupSelector {
            conclusion: b,
            attributed_to: "reviewer".into(),
        }]),
        ..Default::default()
    };
    assert_eq!(
        compare(&from.0, from.1, &to.0, to.1, &deps)
            .unwrap()
            .applicability(),
        Applicability::Stale
    );
    assert_eq!(
        compare(&to.0, to.1, &to.0, to.1, &deps)
            .unwrap()
            .applicability(),
        Applicability::Unknown
    );
    let mut tampered = from.0.clone();
    *tampered.last_mut().unwrap() ^= 1;
    assert_eq!(
        compare(&tampered, from.1, &to.0, to.1, &deps).unwrap_err(),
        DifferentialError::From(PortableError::DigestMismatch)
    );
    assert_eq!(
        compare(&to.0, from.1, &to.0, to.1, &deps).unwrap_err(),
        DifferentialError::From(PortableError::DigestMismatch)
    );
    assert_eq!(
        compare(&from.0, from.1, &to.0, to.1, &Dependencies::default()).unwrap_err(),
        DifferentialError::InvalidSelector
    );
}

#[test]
fn dependency_limit_is_checked_before_import() {
    let mut deps = Dependencies::default();
    for i in 0..=fabric_corpus::as_of::MAX_VIEW_REVISIONS {
        let mut id = [0; 32];
        id[..8].copy_from_slice(&(i as u64).to_be_bytes());
        deps.revisions.insert(id);
    }
    assert_eq!(
        compare(&[], [0; 32], &[], [0; 32], &deps).unwrap_err(),
        DifferentialError::Limit
    );
}
