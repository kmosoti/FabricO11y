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
