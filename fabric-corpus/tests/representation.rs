use fabric_corpus::{
    Corpus, Kind, RevisionBody, SourceClaim,
    as_of::AsOfView,
    enrichment::{self, Availability, Limits, Preparation, Prepared, Request},
    handoff::{Continuation, Declaration, Handoff, Marker},
    portable::{Availability as Claim, AvailabilityClaim, PortableView},
    representation::{self, Choice, Entry, Error, Selected},
    source::CitationV1,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
struct Fixture {
    bytes: Vec<u8>,
    digest: [u8; 32],
    root: [u8; 32],
    citation: CitationV1,
    handoff: Handoff,
    key: Prepared,
}
fn fixture() -> Fixture {
    let mut c = Corpus::default();
    let root = c
        .admit(RevisionBody {
            object_id: "source".into(),
            kind: Kind::Source,
            text: "source".into(),
            parents: BTreeSet::new(),
            sources: vec![SourceClaim::Captured {
                capture_id: "fixture".into(),
                artifact_sha256: Sha256::digest(b"alpha beta").into(),
                locator: "urn:fixture".into(),
                parser: "fixture".into(),
            }],
            extensions: BTreeMap::new(),
        })
        .unwrap();
    let citation = CitationV1::cite(&c, root, 0, 0, 5, Some(b"alpha beta")).unwrap();
    let view =
        AsOfView::capture(&c, BTreeSet::from([root]), BTreeSet::new(), BTreeSet::new()).unwrap();
    let (bytes, digest) = PortableView::new(
        view,
        BTreeMap::from([(
            root,
            AvailabilityClaim {
                archive: Claim::Exact,
                current: Claim::ReferenceOnly,
            },
        )]),
    )
    .unwrap()
    .export()
    .unwrap();
    let d = Declaration {
        basis_digest: digest,
        root,
        policy_id: [3; 32],
        revisions: BTreeSet::from([root]),
        relations: BTreeSet::new(),
        citations: vec![citation.clone()],
        evidence_omission: Marker::Omitted,
        dissent: Marker::Unknown,
        continuation: Continuation::Unresolved,
    };
    let handoff = Handoff::create(d, &bytes, root, [3; 32], digest).unwrap();
    let request = Request {
        view_digest: digest,
        root,
        selected: vec![root],
        scope: "team-a".into(),
        provider: "p".into(),
        provider_version: "1".into(),
        model: "m".into(),
        model_version: "1".into(),
        contract: "abstract-v1".into(),
        config_digest: [4; 32],
        limits: Limits {
            input_bytes: bytes.len(),
            output_bytes: 32,
            selected: 1,
            operations: 1,
        },
    };
    let Preparation::Ready(key) =
        enrichment::prepare(Availability::Available, request, &bytes, digest, root).unwrap()
    else {
        panic!("ready")
    };
    Fixture {
        bytes,
        digest,
        root,
        citation,
        handoff,
        key: *key,
    }
}
fn entry<'a>(f: &'a Fixture, choice: Choice<'a>) -> Entry<'a> {
    Entry {
        revision: f.root,
        citation: &f.citation,
        choice,
    }
}
#[test]
fn exact_raw_unverified_abstract_and_omission_remain_distinct() {
    let f = fixture();
    let raw = [entry(&f, Choice::Raw(b"alpha beta"))];
    let result = representation::select(
        &f.handoff,
        &f.bytes,
        representation::Bindings {
            view: f.digest,
            root: f.root,
            policy: [3; 32],
            scope: "team-a",
        },
        &raw,
        16,
    )
    .unwrap();
    assert_eq!(
        result.entries[0].representation,
        Selected::Raw(b"alpha beta")
    );
    assert_eq!(result.supplied_bytes, 10);
    assert_eq!(
        (
            result.dissent,
            result.evidence_omission,
            result.continuation
        ),
        (Marker::Unknown, Marker::Omitted, Continuation::Unresolved)
    );
    let abstract_bytes = b"short";
    let digest = Sha256::digest(abstract_bytes).into();
    let choice = Choice::ProvidedAbstract {
        bytes: abstract_bytes,
        digest,
        expected: &f.key,
        stored: &f.key,
    };
    let input = [entry(&f, choice)];
    let result = representation::select(
        &f.handoff,
        &f.bytes,
        representation::Bindings {
            view: f.digest,
            root: f.root,
            policy: [3; 32],
            scope: "team-a",
        },
        &input,
        16,
    )
    .unwrap();
    assert_eq!(
        result.entries[0].representation,
        Selected::ProvidedAbstract(abstract_bytes)
    );
    assert_eq!(result.supplied_bytes, 5);
    let omitted = [entry(&f, Choice::Omitted)];
    let result = representation::select(
        &f.handoff,
        &f.bytes,
        representation::Bindings {
            view: f.digest,
            root: f.root,
            policy: [3; 32],
            scope: "team-a",
        },
        &omitted,
        16,
    )
    .unwrap();
    assert_eq!(result.entries[0].representation, Selected::Omitted);
    assert_eq!(result.supplied_bytes, 0);
}
#[test]
fn rejects_scope_binding_digest_order_and_budget() {
    let f = fixture();
    let data = b"short";
    let digest = Sha256::digest(data).into();
    let choice = Choice::ProvidedAbstract {
        bytes: data,
        digest,
        expected: &f.key,
        stored: &f.key,
    };
    let entries = [entry(&f, choice)];
    assert_eq!(
        representation::select(
            &f.handoff,
            &f.bytes,
            representation::Bindings {
                view: f.digest,
                root: f.root,
                policy: [3; 32],
                scope: "team-b"
            },
            &entries,
            10
        ),
        Err(Error::Binding)
    );
    assert_eq!(
        representation::select(
            &f.handoff,
            &f.bytes,
            representation::Bindings {
                view: [0; 32],
                root: f.root,
                policy: [3; 32],
                scope: "team-a"
            },
            &entries,
            10
        ),
        Err(Error::Binding)
    );
    assert_eq!(
        representation::select(
            &f.handoff,
            &f.bytes,
            representation::Bindings {
                view: f.digest,
                root: f.root,
                policy: [2; 32],
                scope: "team-a"
            },
            &entries,
            10
        ),
        Err(Error::Binding)
    );
    assert_eq!(
        representation::select(
            &f.handoff,
            &f.bytes,
            representation::Bindings {
                view: f.digest,
                root: f.root,
                policy: [3; 32],
                scope: "team-a"
            },
            &entries,
            4
        ),
        Err(Error::Limit)
    );
    let bad = [entry(
        &f,
        Choice::ProvidedAbstract {
            bytes: data,
            digest: [0; 32],
            expected: &f.key,
            stored: &f.key,
        },
    )];
    assert_eq!(
        representation::select(
            &f.handoff,
            &f.bytes,
            representation::Bindings {
                view: f.digest,
                root: f.root,
                policy: [3; 32],
                scope: "team-a"
            },
            &bad,
            10
        ),
        Err(Error::Abstract)
    );
    let duplicate = [entry(&f, Choice::Omitted), entry(&f, Choice::Omitted)];
    assert_eq!(
        representation::select(
            &f.handoff,
            &f.bytes,
            representation::Bindings {
                view: f.digest,
                root: f.root,
                policy: [3; 32],
                scope: "team-a"
            },
            &duplicate,
            10
        ),
        Err(Error::Coverage)
    );
    let raw = [entry(&f, Choice::Raw(b"alpha xeta"))];
    assert_eq!(
        representation::select(
            &f.handoff,
            &f.bytes,
            representation::Bindings {
                view: f.digest,
                root: f.root,
                policy: [3; 32],
                scope: "team-a"
            },
            &raw,
            20
        ),
        Err(Error::Raw)
    );
    let mut corrupt = f.bytes.clone();
    corrupt[0] ^= 1;
    assert_eq!(
        representation::select(
            &f.handoff,
            &corrupt,
            representation::Bindings {
                view: f.digest,
                root: f.root,
                policy: [3; 32],
                scope: "team-a"
            },
            &entries,
            20
        ),
        Err(Error::Portable)
    );
}
#[test]
fn every_handoff_citation_requires_one_ordered_explicit_representation() {
    let f = fixture();
    let bindings = representation::Bindings {
        view: f.digest,
        root: f.root,
        policy: [3; 32],
        scope: "team-a",
    };
    assert_eq!(
        representation::select(&f.handoff, &f.bytes, bindings, &[], 20),
        Err(Error::Coverage)
    );
    let mut second = f.citation.clone();
    second.start = 1;
    second.passage_sha256 = Sha256::digest(b"lpha").into();
    let mut declaration = f.handoff.declaration().clone();
    declaration.citations.push(second.clone());
    let multi = Handoff::create(declaration, &f.bytes, f.root, [3; 32], f.digest).unwrap();
    let first = Entry {
        revision: f.root,
        citation: &f.citation,
        choice: Choice::Omitted,
    };
    let second_entry = Entry {
        revision: f.root,
        citation: &second,
        choice: Choice::Omitted,
    };
    assert_eq!(
        representation::select(&multi, &f.bytes, bindings, &[first], 20),
        Err(Error::Coverage)
    );
    assert_eq!(
        representation::select(&multi, &f.bytes, bindings, &[second_entry, first], 20),
        Err(Error::Coverage)
    );
    assert_eq!(
        representation::select(&multi, &f.bytes, bindings, &[first, first], 20),
        Err(Error::Coverage)
    );
    let entries = [first, second_entry];
    let result = representation::select(&multi, &f.bytes, bindings, &entries, 20).unwrap();
    assert_eq!(result.entries.len(), 2);
    assert_eq!(result.entries[0].citation, &f.citation);
    assert_eq!(result.entries[1].citation, &second);
    assert!(
        result
            .entries
            .iter()
            .all(|entry| entry.representation == Selected::Omitted)
    );
    assert_eq!(
        result.evidence_omission,
        multi.declaration().evidence_omission
    );
}
