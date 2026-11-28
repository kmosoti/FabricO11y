//! Synthetic held-out control only: no provider, social authority, or general accuracy claim.
//! Byte and operation caps are logical fixture budgets, not equal CPU or I/O costs.
use fabric_corpus::{
    Corpus, Kind, Predicate, Relation, RelationInsertion, RevisionBody, RevisionDigest,
    SourceClaim,
    as_of::{AsOfView, SupportGroup},
    context::{self, CitationInput, PassageState},
    investigation, lexical,
    portable::{Availability, AvailabilityClaim, PortableView},
    source::CitationV1,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Budget {
    input: usize,
    candidates: usize,
    output: usize,
    evaluations: usize,
    work: usize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counts {
    input: usize,
    candidates: usize,
    output: usize,
    evaluations: usize,
    native_work: usize,
}
#[derive(Debug, PartialEq, Eq)]
struct Score {
    counts: Counts,
    hits: usize,
    labels: usize,
    dissent: usize,
    omitted: usize,
    unknown: usize,
}
struct Fixture {
    bytes: Vec<u8>,
    digest: [u8; 32],
    anchor: RevisionDigest,
    label: RevisionDigest,
    dissent: RevisionDigest,
    missing: RevisionDigest,
    citations: BTreeMap<RevisionDigest, CitationV1>,
    pair: (String, String),
}
fn body(id: &str, kind: Kind, text: &str, source: bool) -> RevisionBody {
    RevisionBody {
        object_id: id.into(),
        kind,
        text: text.into(),
        parents: BTreeSet::new(),
        sources: if source {
            vec![SourceClaim::Captured {
                capture_id: "fixture".into(),
                artifact_sha256: Sha256::digest(b"heldout source").into(),
                locator: "urn:heldout".into(),
                parser: "fixture".into(),
            }]
        } else {
            vec![]
        },
        extensions: BTreeMap::new(),
    }
}
fn edge(c: &mut Corpus, a: RevisionDigest, b: RevisionDigest, p: Predicate) -> RevisionDigest {
    let RelationInsertion::Inserted(id) = c
        .add_relation(Relation {
            subject: a,
            predicate: p,
            object: b,
            attributed_to: "caller-selected".into(),
            extensions: BTreeMap::new(),
        })
        .unwrap()
    else {
        panic!("duplicate")
    };
    id
}
fn fixture(scope: &str, pair: (&str, &str)) -> Fixture {
    let mut c = Corpus::default();
    let anchor = c
        .admit(body(
            &format!("{scope}-question"),
            Kind::Hypothesis,
            "question",
            false,
        ))
        .unwrap();
    let label = c
        .admit(body(
            &format!("{scope}-evidence"),
            Kind::Source,
            &format!("{} {}", pair.0, pair.1),
            true,
        ))
        .unwrap();
    let distractor = c
        .admit(body(
            &format!("{scope}-other"),
            Kind::Source,
            "unrelated words",
            true,
        ))
        .unwrap();
    let dissent = c
        .admit(body(
            &format!("{scope}-dissent"),
            Kind::Dissent,
            "disagree",
            false,
        ))
        .unwrap();
    let missing = [9; 32];
    let support = edge(&mut c, label, anchor, Predicate::Supports);
    let falsifies = edge(&mut c, dissent, anchor, Predicate::Falsifies);
    let group = SupportGroup {
        conclusion: anchor,
        attributed_to: "caller-selected".into(),
        premises: BTreeMap::from([(label, support)]),
    };
    let ids = BTreeSet::from([anchor, label, distractor, dissent, missing]);
    let claims = ids
        .iter()
        .map(|id| {
            (
                *id,
                AvailabilityClaim {
                    archive: Availability::Exact,
                    current: Availability::ReferenceOnly,
                },
            )
        })
        .collect();
    let view = AsOfView::capture(
        &c,
        ids,
        BTreeSet::from([support, falsifies]),
        BTreeSet::from([group]),
    )
    .unwrap();
    let (bytes, digest) = PortableView::new(view, claims).unwrap().export().unwrap();
    let citations = [label, distractor]
        .into_iter()
        .map(|id| {
            (
                id,
                CitationV1::cite(&c, id, 0, 0, 4, Some(b"heldout source")).unwrap(),
            )
        })
        .collect();
    Fixture {
        bytes,
        digest,
        anchor,
        label,
        dissent,
        missing,
        citations,
        pair: (pair.0.into(), pair.1.into()),
    }
}
fn compare(f: &Fixture, a: Budget, b: Budget) -> Result<(Score, Score), &'static str> {
    if a != b {
        return Err("incomparable budgets");
    }
    if f.bytes.len() > a.input
        || a.candidates < 1
        || a.output < 1
        || a.evaluations < 1
        || a.work == 0
        || a.work > 1024
    {
        return Err("budget");
    }
    let view = PortableView::import(&f.bytes, f.digest).map_err(|_| "view")?;
    let lexical = lexical::rebuild(
        view.view(),
        lexical::Limits {
            scan_bytes: a.input,
            tokens: a.work,
            ..lexical::Limits::default()
        },
    )
    .map_err(|_| "lexical")?;
    let candidates = lexical
        .pairs
        .get(&f.pair)
        .map(|p| p.revisions.clone())
        .unwrap_or_default();
    if candidates.len() > a.candidates {
        return Err("candidate limit");
    }
    let lexical_ids: BTreeSet<_> = candidates.iter().copied().take(a.output).collect();
    let graph = investigation::project(
        &f.bytes,
        f.digest,
        BTreeSet::from([f.anchor]),
        investigation::Limits {
            work: a.work,
            ..investigation::Limits::default()
        },
    )
    .map_err(|_| "graph")?;
    if graph.truncated {
        return Err("truncated");
    }
    let graph_candidates: BTreeSet<_> = graph
        .terminal_anchors
        .keys()
        .filter(|id| matches!(view.view().revision(id), Some(Some(_))))
        .copied()
        .collect();
    if graph_candidates.len() > a.candidates {
        return Err("candidate limit");
    }
    let graph_ids: BTreeSet<_> = graph_candidates.iter().copied().take(a.output).collect();
    let evaluate = |ids: &BTreeSet<RevisionDigest>,
                    candidate_count: usize,
                    work: usize|
     -> Result<Score, &'static str> {
        if ids.len() > a.output || ids.len() > a.evaluations {
            return Err("evaluation limit");
        }
        let mut missing = f.citations.values().next().ok_or("no citations")?.clone();
        missing.revision = f.missing;
        let mut inputs: Vec<_> = ids
            .iter()
            .map(|id| {
                f.citations
                    .get(id)
                    .map(|citation| CitationInput {
                        citation,
                        bytes: None,
                    })
                    .ok_or("non-source candidate")
            })
            .collect::<Result<_, _>>()?;
        inputs.push(CitationInput {
            citation: &missing,
            bytes: None,
        });
        let context = context::assemble(view.view(), &inputs).map_err(|_| "context")?;
        let omitted = context
            .entries()
            .iter()
            .filter(|x| matches!(x.state(), PassageState::Omitted))
            .count();
        let unknown = context
            .entries()
            .iter()
            .filter(|x| matches!(x.state(), PassageState::MissingRevision))
            .count();
        let dissent = usize::from(
            context
                .view()
                .revision(&f.dissent)
                .and_then(Option::as_ref)
                .is_some_and(|b| b.kind == Kind::Dissent)
                && context.view().relations().values().any(|r| {
                    r.as_ref().is_some_and(|r| {
                        r.subject == f.dissent && r.predicate == Predicate::Falsifies
                    })
                }),
        );
        Ok(Score {
            counts: Counts {
                input: f.bytes.len(),
                candidates: candidate_count,
                output: ids.len(),
                evaluations: ids.len(),
                native_work: work,
            },
            hits: usize::from(ids.contains(&f.label)),
            labels: 1,
            dissent,
            omitted,
            unknown,
        })
    };
    Ok((
        evaluate(&lexical_ids, candidates.len(), lexical.cost.scanned_bytes)?,
        evaluate(&graph_ids, graph_candidates.len(), graph.work)?,
    ))
}
#[test]
fn held_out_scopes_have_matched_bounds_and_preserve_uncertainty() {
    let fixtures = [
        fixture("team-a", ("amber", "leaf")),
        fixture("team-b", ("cobalt", "stone")),
    ];
    assert_ne!(fixtures[0].label, fixtures[1].label);
    let budget = Budget {
        input: 64 * 1024,
        candidates: 4,
        output: 1,
        evaluations: 1,
        work: 1024,
    };
    for (i, f) in fixtures.iter().enumerate() {
        let (lexical, graph) = compare(f, budget, budget).unwrap();
        assert_eq!((lexical.hits, graph.hits), (1, 1));
        for s in [&lexical, &graph] {
            assert_eq!((s.labels, s.dissent, s.omitted, s.unknown), (1, 1, 1, 1));
            assert_eq!(
                (
                    s.counts.input,
                    s.counts.candidates,
                    s.counts.output,
                    s.counts.evaluations
                ),
                (f.bytes.len(), 1, 1, 1)
            );
        }
        assert_ne!(f.label, fixtures[1 - i].label);
        println!("scope={i} lexical={lexical:?} graph={graph:?}");
    }
}
#[test]
fn unequal_or_exceeded_budgets_refuse_comparison() {
    let f = fixture("team-a", ("amber", "leaf"));
    let b = Budget {
        input: 64 * 1024,
        candidates: 4,
        output: 1,
        evaluations: 1,
        work: 1024,
    };
    let mut other = b;
    other.evaluations = 2;
    assert_eq!(compare(&f, b, other), Err("incomparable budgets"));
    other = b;
    other.input = 1;
    assert_eq!(compare(&f, other, other), Err("budget"));
}

#[test]
fn caller_replay_matches_portable_rebuild_and_selected_result() {
    use lexical::{Apply, Incremental, IncrementalError, RevisionEvent};
    for f in [
        fixture("team-a", ("amber", "leaf")),
        fixture("team-b", ("cobalt", "stone")),
    ] {
        let portable = PortableView::import(&f.bytes, f.digest).unwrap();
        let limits = lexical::Limits {
            scan_bytes: 64 * 1024,
            tokens: 1024,
            ..lexical::Limits::default()
        };
        let rebuilt = lexical::rebuild(portable.view(), limits).unwrap();
        let mut replay = Incremental::new(0, limits).unwrap();
        let target = u64::try_from(portable.view().revisions().len()).unwrap();
        assert!(target > 1);
        let (&first_id, first_body) = portable
            .view()
            .revisions()
            .iter()
            .find(|(_, body)| body.is_some())
            .unwrap();
        let before = replay.clone();
        assert_eq!(
            replay.append(RevisionEvent {
                frontier: 2,
                revision: first_id,
                body: first_body.as_ref()
            }),
            Err(IncrementalError::Gap)
        );
        assert_eq!(replay, before);
        let mut tampered = first_body.as_ref().unwrap().clone();
        tampered.text.push_str(" altered");
        assert_eq!(
            replay.append(RevisionEvent {
                frontier: 1,
                revision: first_id,
                body: Some(&tampered)
            }),
            Err(IncrementalError::DigestMismatch)
        );
        assert_eq!(replay, before);
        let mut scanned = 0;
        let mut tokens = 0;
        for (index, (revision, body)) in portable.view().revisions().iter().enumerate() {
            let next = u64::try_from(index).unwrap() + 1;
            assert_eq!(target - replay.frontier(), target - next + 1);
            let Apply::Applied { work } = replay
                .append(RevisionEvent {
                    frontier: next,
                    revision: *revision,
                    body: body.as_ref(),
                })
                .unwrap()
            else {
                panic!("new selection")
            };
            scanned += work.scanned_bytes;
            tokens += work.tokens;
            assert!(scanned <= limits.scan_bytes && tokens <= limits.tokens);
            assert_eq!(replay.view().cost.scanned_bytes, scanned);
            assert_eq!(replay.view().cost.tokens, tokens);
            assert_eq!(target - replay.frontier(), target - next);
        }
        assert_eq!(replay.frontier(), target);
        assert_eq!(replay.view(), &rebuilt);
        let selected = |view: &lexical::LexicalView| -> BTreeSet<RevisionDigest> {
            view.pairs
                .get(&f.pair)
                .map(|pair| pair.revisions.iter().copied().take(1).collect())
                .unwrap_or_default()
        };
        assert_eq!(selected(replay.view()), selected(&rebuilt));
        assert_eq!(selected(&rebuilt), BTreeSet::from([f.label]));
        println!("caller_frontier={target} caller_pending=0 scanned={scanned} tokens={tokens}");
    }
}
