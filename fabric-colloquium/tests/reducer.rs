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

fn input_set(ids: &[u8]) -> BTreeSet<ContributionId> {
    ids.iter().copied().map(id).collect()
}

#[test]
fn review_profiles_are_opt_in_and_enforce_distinct_input_caps() {
    assert_eq!(ReviewProfile::new(0, 0), Err(ReviewError::InvalidLimits));
    assert_eq!(
        ReviewProfile::new(MAX_REVIEW_INPUTS + 1, 0),
        Err(ReviewError::InvalidLimits)
    );
    assert_eq!(
        ReviewProfile::new(1, MAX_REVIEW_REOPENINGS + 1),
        Err(ReviewError::InvalidLimits)
    );
    let mut protocol = Protocol::new(8, 4).unwrap();
    protocol.append(entry(1, Kind::Discussion, &[])).unwrap();
    protocol.append(entry(2, Kind::Discussion, &[])).unwrap();
    let selected = input_set(&[1, 2]);

    let mut tight = ReviewSession::new(ReviewProfile::new(1, 0).unwrap());
    assert_eq!(
        tight.seal(&protocol, &selected),
        Err(ReviewError::InputBudgetExceeded)
    );
    assert_eq!(
        tight.status(),
        ReviewStatus::Unresolved(ReviewUnresolvedReason::InputBudget)
    );
    assert!(tight.boundaries().is_empty());

    let mut roomy = ReviewSession::new(ReviewProfile::new(2, 0).unwrap());
    roomy.seal(&protocol, &selected).unwrap();
    assert_eq!(roomy.status(), ReviewStatus::Sealed);
    assert_eq!(roomy.current_boundary().unwrap().inputs(), &selected);

    // Existing Protocol construction and append remain the ordinary opt-in-free path.
    let mut ordinary = Protocol::new(8, 4).unwrap();
    ordinary.append(entry(3, Kind::Proposal, &[])).unwrap();
    assert_eq!(heads(&ordinary), BTreeSet::from([id(3)]));
}

#[test]
fn late_arrivals_are_excluded_until_explicit_reopening_preserves_old_boundary() {
    let mut protocol = Protocol::new(8, 4).unwrap();
    protocol.append(entry(1, Kind::Discussion, &[])).unwrap();
    let initial = input_set(&[1]);
    let expanded = input_set(&[1, 2]);
    let mut review = ReviewSession::new(ReviewProfile::new(4, 1).unwrap());
    review.seal(&protocol, &initial).unwrap();
    assert_eq!(
        review.reopen(&protocol, &expanded),
        Err(ReviewError::MissingContribution(id(2)))
    );
    assert_eq!(review.status(), ReviewStatus::Sealed);
    assert_eq!(review.boundaries().len(), 1);

    protocol.append(entry(2, Kind::Proposal, &[1])).unwrap();
    assert_eq!(review.current_boundary().unwrap().inputs(), &initial);
    review.reopen(&protocol, &expanded).unwrap();
    assert_eq!(review.boundaries().len(), 2);
    assert_eq!(review.boundaries()[0].sequence(), 0);
    assert_eq!(review.boundaries()[0].inputs(), &initial);
    assert_eq!(review.boundaries()[1].sequence(), 1);
    assert_eq!(review.boundaries()[1].inputs(), &expanded);
}

#[test]
fn exhausted_input_budget_is_unresolved_and_keeps_boundary_history() {
    let mut protocol = Protocol::new(8, 4).unwrap();
    for n in 1..=3 {
        protocol.append(entry(n, Kind::Discussion, &[])).unwrap();
    }
    let mut review = ReviewSession::new(ReviewProfile::new(3, 3).unwrap());
    let first = input_set(&[1]);
    let second = input_set(&[2]);
    let third = input_set(&[3]);
    let all = input_set(&[1, 2, 3]);
    review.seal(&protocol, &first).unwrap();
    review.reopen(&protocol, &second).unwrap();
    review.reopen(&protocol, &third).unwrap();
    let history = review.boundaries().to_vec();

    assert_eq!(
        review.reopen(&protocol, &all),
        Err(ReviewError::InputBudgetExceeded)
    );
    assert_eq!(
        review.status(),
        ReviewStatus::Unresolved(ReviewUnresolvedReason::InputBudget)
    );
    assert_eq!(review.boundaries(), history);
    assert_eq!(review.total_inputs(), 3);
}

#[test]
fn exhausted_reopening_budget_is_unresolved_without_replacing_history() {
    let mut protocol = Protocol::new(8, 4).unwrap();
    protocol.append(entry(1, Kind::Discussion, &[])).unwrap();
    protocol.append(entry(2, Kind::Discussion, &[])).unwrap();
    let mut review = ReviewSession::new(ReviewProfile::new(4, 1).unwrap());
    let first = input_set(&[1]);
    let second = input_set(&[2]);
    review.seal(&protocol, &first).unwrap();
    review.reopen(&protocol, &second).unwrap();
    let history = review.boundaries().to_vec();

    assert_eq!(
        review.reopen(&protocol, &input_set(&[1, 2])),
        Err(ReviewError::ReopenBudgetExceeded)
    );
    assert_eq!(
        review.status(),
        ReviewStatus::Unresolved(ReviewUnresolvedReason::ReopenBudget)
    );
    assert_eq!(review.boundaries(), history);
}

#[test]
fn reopening_rejects_a_different_history_with_reused_ids() {
    let mut original = Protocol::new(8, 4).unwrap();
    original.append(entry(1, Kind::Discussion, &[])).unwrap();
    let selected = input_set(&[1]);
    let mut review = ReviewSession::new(ReviewProfile::new(4, 1).unwrap());
    review.seal(&original, &selected).unwrap();
    let history = review.boundaries().to_vec();

    let mut forged = Protocol::new(8, 4).unwrap();
    forged.append(entry(1, Kind::Proposal, &[])).unwrap();
    assert_eq!(
        review.reopen(&forged, &selected),
        Err(ReviewError::HistoryMismatch)
    );
    assert_eq!(
        review.status(),
        ReviewStatus::Unresolved(ReviewUnresolvedReason::HistoryMismatch)
    );
    assert_eq!(review.boundaries(), history);
}
