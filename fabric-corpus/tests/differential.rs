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

#[test]
fn declared_dependency_graph_preserves_alternatives_and_unknown_cycles() {
    use fabric_corpus::differential::{Alternative, propagate};
    let mut c = Corpus::default();
    let p = c.admit(body("premise-p")).unwrap();
    let q = c.admit(body("premise-q")).unwrap();
    let a = c.admit(body("derived-a")).unwrap();
    let b = c.admit(body("derived-b")).unwrap();
    let child = c.admit(body("derived-child")).unwrap();
    let x = c.admit(body("cycle-x")).unwrap();
    let y = c.admit(body("cycle-y")).unwrap();
    let grounded = c.admit(body("grounded")).unwrap();
    let extra = c.admit(body("unrelated")).unwrap();
    let ids = BTreeSet::from([p, q, a, b, child, x, y, grounded]);
    let make = |changed: BTreeSet<[u8; 32]>, include_extra: bool| {
        let mut selected = ids.clone();
        if include_extra {
            selected.insert(extra);
        }
        let claims = selected
            .iter()
            .map(|id| {
                (
                    *id,
                    AvailabilityClaim {
                        archive: Availability::Exact,
                        current: if changed.contains(id) {
                            Availability::Missing
                        } else {
                            Availability::Exact
                        },
                    },
                )
            })
            .collect();
        PortableView::new(
            AsOfView::capture(&c, selected, BTreeSet::new(), BTreeSet::new()).unwrap(),
            claims,
        )
        .unwrap()
        .export()
        .unwrap()
    };
    let before = make(BTreeSet::new(), false);
    let after = make(BTreeSet::from([p]), true);
    let all_stale = make(BTreeSet::from([p, q]), true);
    let direct = |id| Alternative {
        direct: Dependencies {
            revisions: BTreeSet::from([id]),
            ..Default::default()
        },
        derived: BTreeSet::new(),
    };
    let link = |id| Alternative {
        direct: Dependencies::default(),
        derived: BTreeSet::from([id]),
    };
    let graph = BTreeMap::from([
        (a, vec![direct(p), direct(q)]),
        (b, vec![direct(p)]),
        (child, vec![link(b)]),
        (x, vec![link(y)]),
        (y, vec![link(x)]),
        (grounded, vec![link(x), direct(q)]),
    ]);
    let out = propagate(&before.0, before.1, &after.0, after.1, &graph).unwrap();
    assert_eq!(
        out.nodes[&a].alternatives,
        vec![Applicability::Stale, Applicability::Unchanged]
    );
    assert_eq!(out.nodes[&a].applicability, Applicability::Unchanged);
    assert_eq!(out.nodes[&b].applicability, Applicability::Stale);
    assert_eq!(out.nodes[&child].applicability, Applicability::Stale);
    assert_eq!(out.nodes[&x].applicability, Applicability::Unknown);
    assert_eq!(out.nodes[&y].applicability, Applicability::Unknown);
    assert_eq!(out.nodes[&grounded].applicability, Applicability::Unchanged);
    let other = propagate(&before.0, before.1, &all_stale.0, all_stale.1, &graph).unwrap();
    assert_eq!(other.nodes[&a].applicability, Applicability::Stale);
    assert_eq!(other.nodes[&grounded].applicability, Applicability::Unknown);
    let reconstructed = PortableView::import(&before.0, before.1)
        .unwrap()
        .export()
        .unwrap();
    assert_eq!(reconstructed, before);
    assert_eq!(
        propagate(&reconstructed.0, reconstructed.1, &after.0, after.1, &graph).unwrap(),
        out
    );
    let mut bad = after.0.clone();
    bad[0] ^= 1;
    assert!(propagate(&before.0, before.1, &bad, after.1, &graph).is_err());
}
#[test]
fn propagation_refuses_missing_historical_outputs_and_graph_limits() {
    use fabric_corpus::differential::{
        Alternative, MAX_PROPAGATION_NODES, PropagationError, propagate,
    };
    let mut c = Corpus::default();
    let a = c.admit(body("a")).unwrap();
    let base = snapshot(
        &c,
        BTreeSet::from([a]),
        BTreeSet::new(),
        BTreeSet::new(),
        Availability::Exact,
    );
    let direct = Alternative {
        direct: Dependencies {
            revisions: BTreeSet::from([a]),
            ..Default::default()
        },
        derived: BTreeSet::new(),
    };
    let missing = BTreeMap::from([([42; 32], vec![direct.clone()])]);
    assert_eq!(
        propagate(&base.0, base.1, &base.0, base.1, &missing),
        Err(PropagationError::MissingHistoricalOutput)
    );
    let foreign = BTreeMap::from([(
        a,
        vec![Alternative {
            direct: Dependencies::default(),
            derived: BTreeSet::from([[99; 32]]),
        }],
    )]);
    assert_eq!(
        propagate(&base.0, base.1, &base.0, base.1, &foreign),
        Err(PropagationError::InvalidGraph)
    );
    let too_many = (0..=MAX_PROPAGATION_NODES)
        .map(|n| {
            let mut id = [0; 32];
            id[0] = n as u8;
            (id, vec![direct.clone()])
        })
        .collect();
    assert_eq!(
        propagate(&base.0, base.1, &base.0, base.1, &too_many),
        Err(PropagationError::Limit)
    );
}
#[test]
fn graph_outputs_cannot_bypass_propagation_as_direct_revisions() {
    use fabric_corpus::differential::{Alternative, PropagationError, propagate};
    let mut c = Corpus::default();
    let a = c.admit(body("a")).unwrap();
    let b = c.admit(body("b")).unwrap();
    let external = c.admit(body("external")).unwrap();
    let base = snapshot(
        &c,
        BTreeSet::from([a, b, external]),
        BTreeSet::new(),
        BTreeSet::new(),
        Availability::Exact,
    );
    let direct = |id| Alternative {
        direct: Dependencies {
            revisions: BTreeSet::from([id]),
            ..Default::default()
        },
        derived: BTreeSet::new(),
    };
    let self_direct = BTreeMap::from([(a, vec![direct(a)])]);
    assert_eq!(
        propagate(&base.0, base.1, &base.0, base.1, &self_direct),
        Err(PropagationError::InvalidGraph)
    );
    let cross_direct = BTreeMap::from([(a, vec![direct(b)]), (b, vec![direct(external)])]);
    assert_eq!(
        propagate(&base.0, base.1, &base.0, base.1, &cross_direct),
        Err(PropagationError::InvalidGraph)
    );
}
#[test]
fn propagation_preflights_oversized_edges_and_group_names() {
    use fabric_corpus::{
        as_of::MAX_ATTRIBUTION_BYTES,
        differential::{
            Alternative, DifferentialError, MAX_PROPAGATION_EDGES, PropagationError, propagate,
        },
    };
    let output = [255; 32];
    let ids = (0..=MAX_PROPAGATION_EDGES)
        .map(|n| {
            let mut id = [0; 32];
            id[..8].copy_from_slice(&(n as u64).to_be_bytes());
            id
        })
        .collect();
    let oversized = BTreeMap::from([(
        output,
        vec![Alternative {
            direct: Dependencies {
                revisions: ids,
                ..Default::default()
            },
            derived: BTreeSet::new(),
        }],
    )]);
    assert_eq!(
        propagate(&[], [0; 32], &[], [0; 32], &oversized),
        Err(PropagationError::Limit)
    );
    for name in [
        "x".repeat(MAX_ATTRIBUTION_BYTES + 1),
        String::new(),
        "bad\nname".into(),
    ] {
        let group = GroupSelector {
            conclusion: [7; 32],
            attributed_to: name,
        };
        let graph = BTreeMap::from([(
            output,
            vec![Alternative {
                direct: Dependencies {
                    groups: BTreeSet::from([group]),
                    ..Default::default()
                },
                derived: BTreeSet::new(),
            }],
        )]);
        assert_eq!(
            propagate(&[], [0; 32], &[], [0; 32], &graph),
            Err(PropagationError::Differential(
                DifferentialError::InvalidSelector
            ))
        );
    }
}
