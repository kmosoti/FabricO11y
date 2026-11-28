use fabric_corpus::{
    Corpus, Kind, RevisionBody,
    as_of::AsOfView,
    lexical::{LexicalError, Limits, MAX_TOKEN_BYTES, rebuild},
};
use std::collections::{BTreeMap, BTreeSet};

fn body(id: &str, text: &str) -> RevisionBody {
    RevisionBody {
        object_id: id.into(),
        kind: Kind::Observation,
        text: text.into(),
        parents: BTreeSet::new(),
        sources: vec![],
        extensions: BTreeMap::new(),
    }
}
fn fixture() -> (AsOfView, [u8; 32], [u8; 32]) {
    let mut corpus = Corpus::default();
    let a = corpus.admit(body("a", "A b A! éB")).unwrap();
    let b = corpus.admit(body("b", "b a")).unwrap();
    let ids = BTreeSet::from([b, [0; 32], a]);
    (
        AsOfView::capture(&corpus, ids, BTreeSet::new(), BTreeSet::new()).unwrap(),
        a,
        b,
    )
}
#[test]
fn hand_counted_pairs_provenance_missing_and_order() {
    let (view, a, b) = fixture();
    let result = rebuild(&view, Limits::default()).unwrap();
    assert_eq!(result.selected, BTreeSet::from([[0; 32], a, b]));
    assert_eq!(result.missing, BTreeSet::from([[0; 32]]));
    let ab = &result.pairs[&("a".into(), "b".into())];
    assert_eq!(ab.occurrences, 2);
    assert_eq!(ab.revisions, BTreeSet::from([a]));
    let ba = &result.pairs[&("b".into(), "a".into())];
    assert_eq!(ba.occurrences, 2);
    assert_eq!(ba.revisions, BTreeSet::from([a, b]));
    assert_eq!(result.cost.scanned_bytes, "A b A! éB".len() + "b a".len());
    assert_eq!(
        (
            result.cost.tokens,
            result.cost.pair_occurrences,
            result.cost.distinct_pairs,
            result.cost.provenance
        ),
        (6, 4, 2, 3)
    );
    assert_eq!(result.cost.output_bytes, 128 + 20 + 96);
    assert_eq!(
        result.pairs.keys().cloned().collect::<Vec<_>>(),
        vec![("a".into(), "b".into()), ("b".into(), "a".into())]
    );
    assert_eq!(rebuild(&view, Limits::default()).unwrap(), result);
}
#[test]
fn each_budget_rejects_before_return_and_exact_limits_pass() {
    let (view, _, _) = fixture();
    let expected = rebuild(&view, Limits::default()).unwrap();
    let c = expected.cost;
    let exact = Limits {
        scan_bytes: c.scanned_bytes,
        tokens: c.tokens,
        token_bytes: 1,
        distinct_pairs: c.distinct_pairs,
        provenance: c.provenance,
        output_bytes: c.output_bytes,
    };
    assert_eq!(rebuild(&view, exact).unwrap(), expected);
    let cases = [
        Limits {
            scan_bytes: c.scanned_bytes - 1,
            ..exact
        },
        Limits {
            tokens: c.tokens - 1,
            ..exact
        },
        Limits {
            token_bytes: 0,
            ..exact
        },
        Limits {
            distinct_pairs: c.distinct_pairs - 1,
            ..exact
        },
        Limits {
            provenance: c.provenance - 1,
            ..exact
        },
        Limits {
            output_bytes: c.output_bytes - 1,
            ..exact
        },
    ];
    for limits in cases {
        assert_eq!(rebuild(&view, limits), Err(LexicalError::Limit));
    }
    assert_eq!(
        rebuild(
            &view,
            Limits {
                token_bytes: MAX_TOKEN_BYTES + 1,
                ..exact
            }
        ),
        Err(LexicalError::InvalidLimits)
    );
}

#[test]
fn incremental_matches_frozen_rebuild_and_reports_work() {
    use fabric_corpus::lexical::{Apply, Incremental, RevisionEvent};
    let a = body("a", "A b A! éB");
    let b = body("b", "b a");
    let mut corpus = Corpus::default();
    let aid = corpus.admit(a.clone()).unwrap();
    let bid = corpus.admit(b.clone()).unwrap();
    let mut incremental = Incremental::new(40, Limits::default()).unwrap();
    let events = [
        (41, bid, Some(&b)),
        (42, [0; 32], None),
        (43, aid, Some(&a)),
    ];
    let mut ids = BTreeSet::new();
    for (frontier, revision, body) in events {
        let outcome = incremental
            .append(RevisionEvent {
                frontier,
                revision,
                body,
            })
            .unwrap();
        if let Apply::Applied { work } = outcome {
            assert_eq!(work.scanned_bytes, body.map_or(0, |b| b.text.len()));
        } else {
            panic!("expected insert");
        }
        ids.insert(revision);
        let snapshot =
            AsOfView::capture(&corpus, ids.clone(), BTreeSet::new(), BTreeSet::new()).unwrap();
        assert_eq!(incremental.frontier(), frontier);
        assert_eq!(
            incremental.view(),
            &rebuild(&snapshot, Limits::default()).unwrap()
        );
    }
    assert_eq!(
        incremental.append(RevisionEvent {
            frontier: 43,
            revision: aid,
            body: Some(&a)
        }),
        Ok(Apply::Duplicate)
    );
}
#[test]
fn incremental_refusals_are_atomic() {
    use fabric_corpus::lexical::{Apply, Incremental, IncrementalError, RevisionEvent};
    let a = body("a", "a b a");
    let aid = a.digest().unwrap();
    let mut inc = Incremental::new(0, Limits::default()).unwrap();
    fn event(frontier: u64, revision: [u8; 32], body: Option<&RevisionBody>) -> RevisionEvent<'_> {
        RevisionEvent {
            frontier,
            revision,
            body,
        }
    }
    assert!(matches!(
        inc.append(event(1, aid, Some(&a))),
        Ok(Apply::Applied { .. })
    ));
    assert_eq!(inc.append(event(1, aid, Some(&a))), Ok(Apply::Duplicate));
    let other = body("other", "x y");
    let oid = other.digest().unwrap();
    for (e, expected) in [
        (event(3, oid, Some(&other)), IncrementalError::Gap),
        (event(0, aid, Some(&a)), IncrementalError::OlderFrontier),
        (
            event(1, oid, Some(&other)),
            IncrementalError::ConflictingReplay,
        ),
        (
            event(2, aid, Some(&other)),
            IncrementalError::DigestMismatch,
        ),
        (event(2, aid, Some(&a)), IncrementalError::RebuildRequired),
    ] {
        let before = inc.clone();
        assert_eq!(inc.append(e), Err(expected));
        assert_eq!(inc, before);
    }
    assert!(matches!(
        inc.append(event(2, oid, None)),
        Ok(Apply::Applied { .. })
    ));
    let before = inc.clone();
    assert_eq!(
        inc.append(event(3, oid, Some(&other))),
        Err(IncrementalError::RebuildRequired)
    );
    assert_eq!(inc, before);
}
#[test]
fn incremental_limits_fail_without_mutating_view() {
    use fabric_corpus::lexical::{Incremental, IncrementalError, RevisionEvent};
    let a = body("a", "a b");
    let b = body("b", "a b");
    let mut inc = Incremental::new(
        0,
        Limits {
            scan_bytes: 3,
            ..Limits::default()
        },
    )
    .unwrap();
    inc.append(RevisionEvent {
        frontier: 1,
        revision: a.digest().unwrap(),
        body: Some(&a),
    })
    .unwrap();
    let before = inc.clone();
    assert_eq!(
        inc.append(RevisionEvent {
            frontier: 2,
            revision: b.digest().unwrap(),
            body: Some(&b)
        }),
        Err(IncrementalError::Projection(LexicalError::Limit))
    );
    assert_eq!(inc, before);
    let mut full = Incremental::new(0, Limits::default()).unwrap();
    for n in 0..fabric_corpus::as_of::MAX_VIEW_REVISIONS {
        let mut id = [0; 32];
        id[..8].copy_from_slice(&(n as u64).to_be_bytes());
        full.append(RevisionEvent {
            frontier: n as u64 + 1,
            revision: id,
            body: None,
        })
        .unwrap();
    }
    let before = full.clone();
    let mut extra = [0; 32];
    extra[0] = 1;
    assert_eq!(
        full.append(RevisionEvent {
            frontier: 257,
            revision: extra,
            body: None
        }),
        Err(IncrementalError::Limit)
    );
    assert_eq!(full, before);
}
