use fabric_corpus::{
    Corpus, Kind, Predicate, Relation, RelationInsertion, RevisionBody, SourceClaim,
    as_of::{AsOfView, SupportGroup},
    reconcile::{ReconcileError, analyze, reconcile},
};
use std::collections::{BTreeMap, BTreeSet};

fn body(id: &str, text: &str, parents: BTreeSet<[u8; 32]>) -> RevisionBody {
    RevisionBody {
        object_id: id.into(),
        kind: Kind::Decision,
        text: text.into(),
        parents,
        sources: vec![],
        extensions: BTreeMap::new(),
    }
}
fn edge(
    corpus: &mut Corpus,
    subject: [u8; 32],
    predicate: Predicate,
    object: [u8; 32],
) -> [u8; 32] {
    match corpus
        .add_relation(Relation {
            subject,
            predicate,
            object,
            attributed_to: "reviewer".into(),
            extensions: BTreeMap::new(),
        })
        .unwrap()
    {
        RelationInsertion::Inserted(id) => id,
        _ => panic!("duplicate"),
    }
}
#[test]
fn competing_heads_require_exact_explicit_merge_and_keep_claims() {
    let mut c = Corpus::default();
    let root = c.admit(body("x", "root", BTreeSet::new())).unwrap();
    let left = c.admit(body("x", "left", BTreeSet::from([root]))).unwrap();
    let right = c.admit(body("x", "right", BTreeSet::from([root]))).unwrap();
    let unrelated = c.admit(body("other", "other", BTreeSet::new())).unwrap();
    let support = edge(&mut c, left, Predicate::Supports, right);
    let dissent = edge(&mut c, unrelated, Predicate::Falsifies, right);
    let group = SupportGroup {
        conclusion: right,
        attributed_to: "analyst".into(),
        premises: BTreeMap::from([(left, support)]),
    };
    let view = AsOfView::capture(
        &c,
        BTreeSet::from([root, left, right, unrelated]),
        BTreeSet::from([support, dissent]),
        BTreeSet::from([group.clone()]),
    )
    .unwrap();
    let frozen = view.clone();
    assert_eq!(
        analyze(&view, "x").unwrap().heads,
        BTreeSet::from([left, right])
    );
    assert_eq!(
        reconcile(&mut c, &view, body("x", "bad", BTreeSet::from([left]))),
        Err(ReconcileError::ParentMismatch)
    );
    assert_eq!(
        reconcile(
            &mut c,
            &view,
            body("x", "bad", BTreeSet::from([left, right, unrelated]))
        ),
        Err(ReconcileError::ParentMismatch)
    );
    let mut merged = body("x", "merged", BTreeSet::from([left, right]));
    merged.sources.push(SourceClaim::ExternalOnly {
        locator: "urn:origin".into(),
        parser: "p".into(),
    });
    let merged_id = reconcile(&mut c, &view, merged.clone()).unwrap();
    assert_eq!(c.revision(&merged_id), Some(&merged));
    assert_eq!(c.revision(&left).unwrap().text, "left");
    assert_eq!(c.revision(&right).unwrap().text, "right");
    assert_eq!(view, frozen);
    assert!(view.groups()[&group].is_resolved());
    assert_eq!(c.relations()[&dissent].predicate, Predicate::Falsifies);
    assert_eq!(c.relations()[&support].predicate, Predicate::Supports);
}
#[test]
fn withheld_or_missing_ancestry_is_explicit_and_blocks_merge() {
    let mut c = Corpus::default();
    let root = c.admit(body("x", "root", BTreeSet::new())).unwrap();
    let left = c.admit(body("x", "left", BTreeSet::from([root]))).unwrap();
    let right = c.admit(body("x", "right", BTreeSet::from([root]))).unwrap();
    let incomplete = AsOfView::capture(
        &c,
        BTreeSet::from([left, right]),
        BTreeSet::new(),
        BTreeSet::new(),
    )
    .unwrap();
    let a = analyze(&incomplete, "x").unwrap();
    assert_eq!(a.unselected_ancestry, BTreeSet::from([root]));
    assert!(!a.is_complete());
    assert_eq!(
        reconcile(
            &mut c,
            &incomplete,
            body("x", "merged", BTreeSet::from([left, right]))
        ),
        Err(ReconcileError::Incomplete(a))
    );
    let missing_id = [9; 32];
    let missing = AsOfView::capture(
        &c,
        BTreeSet::from([root, left, right, missing_id]),
        BTreeSet::new(),
        BTreeSet::new(),
    )
    .unwrap();
    let a = analyze(&missing, "x").unwrap();
    assert_eq!(a.missing_selected, BTreeSet::from([missing_id]));
    assert!(!a.is_complete());
    assert_eq!(
        reconcile(
            &mut c,
            &missing,
            body("x", "merged", BTreeSet::from([left, right]))
        ),
        Err(ReconcileError::Incomplete(a))
    );
}
#[test]
fn frozen_view_must_match_corpus_and_at_least_two_heads() {
    let mut original = Corpus::default();
    let a = original.admit(body("x", "a", BTreeSet::new())).unwrap();
    let b = original.admit(body("x", "b", BTreeSet::new())).unwrap();
    let view = AsOfView::capture(
        &original,
        BTreeSet::from([a, b]),
        BTreeSet::new(),
        BTreeSet::new(),
    )
    .unwrap();
    let mut another = Corpus::default();
    assert_eq!(
        reconcile(
            &mut another,
            &view,
            body("x", "merge", BTreeSet::from([a, b]))
        ),
        Err(ReconcileError::ViewMismatch)
    );
    let single = AsOfView::capture(
        &original,
        BTreeSet::from([a]),
        BTreeSet::new(),
        BTreeSet::new(),
    )
    .unwrap();
    assert_eq!(
        reconcile(
            &mut original,
            &single,
            body("x", "child", BTreeSet::from([a]))
        ),
        Err(ReconcileError::InsufficientHeads)
    );
}

#[test]
fn aggregate_ancestry_link_limit_is_enforced() {
    let mut corpus = Corpus::default();
    let mut ids = BTreeSet::new();
    for i in 0..65 {
        ids.insert(
            corpus
                .admit(body("x", &format!("root-{i}"), BTreeSet::new()))
                .unwrap(),
        );
    }
    let roots = ids.clone();
    for i in 0..64 {
        ids.insert(
            corpus
                .admit(body("x", &format!("child-{i}"), roots.clone()))
                .unwrap(),
        );
    }
    let view = AsOfView::capture(&corpus, ids, BTreeSet::new(), BTreeSet::new()).unwrap();
    assert_eq!(analyze(&view, "x"), Err(ReconcileError::Limit));
}
