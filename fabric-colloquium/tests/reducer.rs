use fabric_colloquium::*;
use std::collections::BTreeSet;

fn id(n: u8) -> ContributionId {
    ContributionId([n; 32])
}
fn entry(n: u8, kind: Kind, parents: &[u8]) -> Contribution {
    Contribution {
        id: id(n),
        actor: ActorId([1; 32]),
        principal: PrincipalId([2; 32]),
        team: Some(TeamId([3; 32])),
        kind,
        parents: parents.iter().copied().map(id).collect(),
    }
}
fn heads(state: &Protocol) -> BTreeSet<ContributionId> {
    state.proposal_heads().clone()
}

#[test]
fn concurrent_proposals_remain_discoverable_and_merge_deterministically() {
    let root = entry(1, Kind::Discussion, &[]);
    let a = entry(2, Kind::Proposal, &[1]);
    let b = entry(3, Kind::Proposal, &[1]);
    let merge = entry(4, Kind::Proposal, &[2, 3]);
    let mut left = Protocol::new(16, 4).unwrap();
    let mut right = Protocol::new(16, 4).unwrap();
    for c in [root.clone(), a.clone(), b.clone(), merge.clone()] {
        left.append(c).unwrap();
    }
    for c in [root, b, a, merge] {
        right.append(c).unwrap();
    }
    assert_eq!(left, right);
    assert_eq!(heads(&left), BTreeSet::from([id(4)]));
    assert_eq!(left.turn(&id(4)), Some(2));
}

#[test]
fn rejects_missing_parents_duplicate_ids_and_overflow_without_mutation() {
    let mut s = Protocol::new(3, 1).unwrap();
    s.append(entry(1, Kind::Proposal, &[])).unwrap();
    let before = s.clone();
    assert_eq!(
        s.append(entry(2, Kind::Discussion, &[9])),
        Err(Error::MissingParent(id(9)))
    );
    assert_eq!(
        s.append(entry(1, Kind::Discussion, &[])),
        Err(Error::DuplicateId(id(1)))
    );
    assert_eq!(
        s.append(entry(2, Kind::Proposal, &[])),
        Err(Error::TooManyHeads)
    );
    assert_eq!(s, before);
    s.append(entry(2, Kind::Proposal, &[1])).unwrap();
    s.append(entry(
        3,
        Kind::Custom {
            namespace: "x".into(),
            name: "critique".into(),
        },
        &[2],
    ))
    .unwrap();
    let before = s.clone();
    assert_eq!(
        s.append(entry(4, Kind::Discussion, &[3])),
        Err(Error::TooManyContributions)
    );
    assert_eq!(s, before);
}

#[test]
fn stale_parent_proposals_are_concurrent_and_custom_names_are_inert() {
    let mut s = Protocol::new(8, 4).unwrap();
    s.append(entry(1, Kind::Proposal, &[])).unwrap();
    s.append(entry(2, Kind::Proposal, &[1])).unwrap();
    s.append(entry(3, Kind::Proposal, &[1])).unwrap();
    s.append(entry(
        4,
        Kind::Custom {
            namespace: "example".into(),
            name: "proposal".into(),
        },
        &[2],
    ))
    .unwrap();
    assert_eq!(heads(&s), BTreeSet::from([id(2), id(3)]));
    assert_eq!(s.turn(&id(4)), Some(2));
    let before = s.clone();
    assert_eq!(
        s.append(entry(
            5,
            Kind::Custom {
                namespace: "".into(),
                name: "x".into()
            },
            &[]
        )),
        Err(Error::InvalidKind)
    );
    assert_eq!(s, before);
}

#[test]
fn bounded_inputs_are_rejected_and_identity_roles_are_preserved() {
    assert_eq!(Protocol::new(0, 1), Err(Error::InvalidLimits));
    assert_eq!(
        Protocol::new(MAX_CONTRIBUTIONS + 1, 1),
        Err(Error::InvalidLimits)
    );
    assert_eq!(Protocol::new(2, 3), Err(Error::InvalidLimits));
    let mut s = Protocol::new(100, 4).unwrap();
    let original = entry(1, Kind::Discussion, &[]);
    s.append(original.clone()).unwrap();
    assert_eq!(s.contribution(&id(1)), Some(&original));
    let mut too_many = entry(2, Kind::Discussion, &[]);
    too_many.parents = (0..=MAX_PARENTS)
        .map(|n| ContributionId([n as u8; 32]))
        .collect();
    let before = s.clone();
    assert_eq!(s.append(too_many), Err(Error::TooManyParents));
    assert_eq!(
        s.append(entry(
            3,
            Kind::Custom {
                namespace: "x".repeat(MAX_KIND_COMPONENT_BYTES + 1),
                name: "n".into()
            },
            &[]
        )),
        Err(Error::InvalidKind)
    );
    assert_eq!(s, before);
}
