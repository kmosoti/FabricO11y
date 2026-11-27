use fabric_corpus::as_of::{
    AsOfView, MAX_GROUP_PREMISES, MAX_VIEW_REVISIONS, PremiseStatus, SupportGroup, ViewError,
};
use fabric_corpus::{
    Corpus, Kind, Predicate, Relation, RelationDigest, RelationInsertion, RevisionBody,
    RevisionDigest,
};
use std::collections::{BTreeMap, BTreeSet};

fn body(id: &str, kind: Kind) -> RevisionBody {
    RevisionBody {
        object_id: id.into(),
        kind,
        text: id.into(),
        parents: BTreeSet::new(),
        sources: vec![],
        extensions: BTreeMap::new(),
    }
}
fn relation(a: RevisionDigest, predicate: Predicate, b: RevisionDigest, author: &str) -> Relation {
    Relation {
        subject: a,
        predicate,
        object: b,
        attributed_to: author.into(),
        extensions: BTreeMap::new(),
    }
}
fn insert(corpus: &mut Corpus, edge: Relation) -> RelationDigest {
    match corpus.add_relation(edge).unwrap() {
        RelationInsertion::Inserted(id) => id,
        _ => panic!("unexpected duplicate"),
    }
}

#[test]
fn historical_conjunction_stays_unresolved_after_later_evidence_and_dissent_survives() {
    let mut corpus = Corpus::default();
    let conclusion = corpus.admit(body("decision", Kind::Decision)).unwrap();
    let premise_a = corpus.admit(body("a", Kind::Observation)).unwrap();
    let future_body = body("b", Kind::Analysis);
    let premise_b = future_body.digest().unwrap();
    let edge_a = insert(
        &mut corpus,
        relation(premise_a, Predicate::Supports, conclusion, "alice"),
    );
    let future_edge = relation(premise_b, Predicate::Supports, conclusion, "bob");
    let edge_b = future_edge.digest().unwrap();
    let dissent = corpus.admit(body("dissent", Kind::Dissent)).unwrap();
    let dissent_edge = insert(
        &mut corpus,
        relation(dissent, Predicate::Falsifies, conclusion, "critic"),
    );
    let group = SupportGroup {
        conclusion,
        attributed_to: "reviewer".into(),
        premises: BTreeMap::from([(premise_a, edge_a), (premise_b, edge_b)]),
    };
    let ids = BTreeSet::from([conclusion, premise_a, premise_b, dissent]);
    let edges = BTreeSet::from([edge_a, edge_b, dissent_edge]);
    let groups = BTreeSet::from([group.clone()]);
    let before = AsOfView::capture(&corpus, ids.clone(), edges.clone(), groups.clone()).unwrap();
    let snapshot = before.clone();
    assert_eq!(before.revision(&premise_b), Some(&None));
    assert_eq!(before.relation(&edge_b), Some(&None));
    assert!(!before.groups()[&group].is_resolved());
    assert_eq!(
        before.groups()[&group].premises()[&premise_b],
        PremiseStatus::MissingRevision
    );
    assert_eq!(
        before
            .relation(&dissent_edge)
            .unwrap()
            .as_ref()
            .unwrap()
            .attributed_to,
        "critic"
    );
    assert!(matches!(
        before.revision(&dissent).unwrap().as_ref().unwrap().kind,
        Kind::Dissent
    ));
    assert_eq!(corpus.admit(future_body).unwrap(), premise_b);
    assert_eq!(insert(&mut corpus, future_edge), edge_b);
    let after = AsOfView::capture(&corpus, ids, edges, groups).unwrap();
    assert!(after.groups()[&group].is_resolved());
    assert_eq!(before, snapshot);
    assert_eq!(before.revision(&premise_b), Some(&None));
    assert_ne!(before, after);
}

#[test]
fn no_vacuous_or_mismatched_support_and_unselected_are_explicit() {
    let mut corpus = Corpus::default();
    let target = corpus.admit(body("target", Kind::Decision)).unwrap();
    let premise = corpus.admit(body("premise", Kind::Observation)).unwrap();
    let wrong = insert(
        &mut corpus,
        relation(premise, Predicate::Falsifies, target, "critic"),
    );
    let group = SupportGroup {
        conclusion: target,
        attributed_to: "author".into(),
        premises: BTreeMap::from([(premise, wrong)]),
    };
    let ids = BTreeSet::from([target, premise]);
    let view = AsOfView::capture(
        &corpus,
        ids.clone(),
        BTreeSet::from([wrong]),
        BTreeSet::from([group.clone()]),
    )
    .unwrap();
    assert_eq!(
        view.groups()[&group].premises()[&premise],
        PremiseStatus::MismatchedRelation
    );
    assert!(!view.groups()[&group].is_resolved());
    let no_edge = AsOfView::capture(
        &corpus,
        ids.clone(),
        BTreeSet::new(),
        BTreeSet::from([group.clone()]),
    )
    .unwrap();
    assert_eq!(
        no_edge.groups()[&group].premises()[&premise],
        PremiseStatus::MissingRelation
    );
    assert_eq!(no_edge.relation(&wrong), None);
    let no_target = AsOfView::capture(
        &corpus,
        BTreeSet::from([premise]),
        BTreeSet::from([wrong]),
        BTreeSet::from([group.clone()]),
    )
    .unwrap();
    assert!(!no_target.groups()[&group].conclusion_present());
    assert!(!no_target.groups()[&group].is_resolved());
    let empty = SupportGroup {
        premises: BTreeMap::new(),
        ..group
    };
    assert!(matches!(
        AsOfView::capture(&corpus, ids, BTreeSet::new(), BTreeSet::from([empty])),
        Err(ViewError::InvalidGroup)
    ));
}

#[test]
fn bounds_and_order_are_deterministic() {
    let mut corpus = Corpus::default();
    let a = corpus.admit(body("a", Kind::Source)).unwrap();
    let b = corpus.admit(body("b", Kind::Result)).unwrap();
    let one = AsOfView::capture(
        &corpus,
        [a, b].into_iter().collect(),
        BTreeSet::new(),
        BTreeSet::new(),
    )
    .unwrap();
    let two = AsOfView::capture(
        &corpus,
        [b, a].into_iter().collect(),
        BTreeSet::new(),
        BTreeSet::new(),
    )
    .unwrap();
    assert_eq!(one, two);
    let ids = (0..=MAX_VIEW_REVISIONS)
        .map(|i| {
            let mut d = [0; 32];
            d[..2].copy_from_slice(&(i as u16).to_be_bytes());
            d
        })
        .collect();
    assert!(matches!(
        AsOfView::capture(&corpus, ids, BTreeSet::new(), BTreeSet::new()),
        Err(ViewError::Limit)
    ));
    let premises = (0..=MAX_GROUP_PREMISES)
        .map(|i| {
            let mut d = [0; 32];
            d[0] = i as u8;
            (d, [0; 32])
        })
        .collect();
    let huge_group = SupportGroup {
        conclusion: b,
        attributed_to: "author".into(),
        premises,
    };
    assert!(matches!(
        AsOfView::capture(
            &corpus,
            BTreeSet::new(),
            BTreeSet::new(),
            BTreeSet::from([huge_group])
        ),
        Err(ViewError::Limit)
    ));
    let mut large_ids = BTreeSet::new();
    for i in 0..5 {
        let mut item = body(&format!("large-{i}"), Kind::Source);
        item.text = "x".repeat(900_000);
        large_ids.insert(corpus.admit(item).unwrap());
    }
    assert!(matches!(
        AsOfView::capture(&corpus, large_ids, BTreeSet::new(), BTreeSet::new()),
        Err(ViewError::Limit)
    ));
}
