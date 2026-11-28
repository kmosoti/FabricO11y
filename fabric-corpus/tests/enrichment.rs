use fabric_corpus::{
    Corpus, Kind, RevisionBody,
    as_of::AsOfView,
    enrichment::{Availability as Mode, Error, Limits, Preparation, Request, prepare},
    portable::{Availability, AvailabilityClaim, PortableView},
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
type Fixture = (Vec<u8>, [u8; 32], [u8; 32], [u8; 32], Request);
fn fixture() -> Fixture {
    let mut corpus = Corpus::default();
    let mut make = |text: &str| {
        corpus
            .admit(RevisionBody {
                object_id: text.into(),
                kind: Kind::Observation,
                text: text.into(),
                parents: BTreeSet::new(),
                sources: vec![],
                extensions: BTreeMap::new(),
            })
            .unwrap()
    };
    let root = make("root");
    let other = make("other");
    let missing = [8; 32];
    let ids = BTreeSet::from([root, other, missing]);
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
    let view = PortableView::new(
        AsOfView::capture(&corpus, ids, BTreeSet::new(), BTreeSet::new()).unwrap(),
        claims,
    )
    .unwrap();
    let (bytes, digest) = view.export().unwrap();
    let request = Request {
        view_digest: digest,
        root,
        selected: vec![root],
        scope: "team-a".into(),
        provider: "provider".into(),
        provider_version: "v1".into(),
        model: "model".into(),
        model_version: "v1".into(),
        contract: "summary-v1".into(),
        config_digest: [7; 32],
        limits: Limits {
            input_bytes: bytes.len(),
            output_bytes: 10,
            selected: 3,
            operations: 10,
        },
    };
    (bytes, digest, root, other, request)
}
fn ready(
    request: Request,
    bytes: &[u8],
    view: [u8; 32],
    root: [u8; 32],
) -> fabric_corpus::enrichment::Prepared {
    let Preparation::Ready(p) = prepare(Mode::Available, request, bytes, view, root).unwrap()
    else {
        panic!("expected ready")
    };
    *p
}
#[test]
fn every_key_field_prevents_reuse() {
    let (bytes, digest, root, other, base) = fixture();
    let p = ready(base.clone(), &bytes, digest, root);
    let out = b"hello";
    let hash = Sha256::digest(out).into();
    assert_eq!(p.validate_cached(&p, out, 5, hash), Ok(out.as_slice()));
    let mut cases = Vec::new();
    let mut x = base.clone();
    x.scope = "team-b".into();
    cases.push(x);
    let mut x = base.clone();
    x.provider = "other".into();
    cases.push(x);
    let mut x = base.clone();
    x.provider_version = "v2".into();
    cases.push(x);
    let mut x = base.clone();
    x.model = "other".into();
    cases.push(x);
    let mut x = base.clone();
    x.model_version = "v2".into();
    cases.push(x);
    let mut x = base.clone();
    x.contract = "v2".into();
    cases.push(x);
    let mut x = base.clone();
    x.config_digest = [9; 32];
    cases.push(x);
    let mut x = base.clone();
    x.limits.operations += 1;
    cases.push(x);
    let mut x = base.clone();
    x.limits.output_bytes += 1;
    cases.push(x);
    let mut x = base.clone();
    x.limits.input_bytes += 1;
    cases.push(x);
    let mut x = base.clone();
    x.limits.selected += 1;
    cases.push(x);
    let mut x = base;
    x.selected.push(other);
    x.selected.sort();
    cases.push(x);
    for x in cases {
        let changed = ready(x, &bytes, digest, root);
        assert_eq!(
            p.validate_cached(&changed, out, 5, hash),
            Err(Error::CacheMismatch)
        );
    }
    assert_eq!(
        p.validate_cached(&p, out, 4, hash),
        Err(Error::OutputLength)
    );
    assert_eq!(
        p.validate_cached(&p, b"hellx", 5, hash),
        Err(Error::OutputDigest)
    );
    assert_eq!(
        p.validate_cached(&p, b"01234567890", 11, hash),
        Err(Error::Limit)
    );
}
#[test]
fn rejects_mismatches_missing_omitted_duplicates_and_limits() {
    let (bytes, digest, root, other, base) = fixture();
    assert_eq!(
        prepare(Mode::Available, base.clone(), &bytes, [0; 32], root),
        Err(Error::ViewMismatch)
    );
    assert_eq!(
        prepare(Mode::Available, base.clone(), &bytes, digest, other),
        Err(Error::RootMismatch)
    );
    let mut tampered = bytes.clone();
    tampered[0] ^= 1;
    assert_eq!(
        prepare(Mode::Available, base.clone(), &tampered, digest, root),
        Err(Error::Portable)
    );
    let mut x = base.clone();
    x.selected = vec![root, root];
    assert_eq!(
        prepare(Mode::Available, x, &bytes, digest, root),
        Err(Error::InvalidIdentity)
    );
    let mut x = base.clone();
    x.selected.push([8; 32]);
    x.selected.sort();
    assert_eq!(
        prepare(Mode::Available, x, &bytes, digest, root),
        Err(Error::Missing)
    );
    let mut x = base.clone();
    x.selected.push([9; 32]);
    x.selected.sort();
    assert_eq!(
        prepare(Mode::Available, x, &bytes, digest, root),
        Err(Error::Omitted)
    );
    let mut x = base.clone();
    x.limits.input_bytes = bytes.len() - 1;
    assert_eq!(
        prepare(Mode::Available, x, &bytes, digest, root),
        Err(Error::Limit)
    );
    let mut x = base.clone();
    x.limits.operations = 0;
    assert_eq!(
        prepare(Mode::Available, x, &bytes, digest, root),
        Err(Error::Limit)
    );
    let mut x = base;
    x.provider.clear();
    assert_eq!(
        prepare(Mode::Available, x, &bytes, digest, root),
        Err(Error::InvalidIdentity)
    );
}
#[test]
fn disabled_and_unavailable_do_not_consume_inputs() {
    let (_, _, root, _, mut request) = fixture();
    request.selected.clear();
    assert_eq!(
        prepare(Mode::Disabled, request.clone(), &[], [0; 32], root),
        Ok(Preparation::Disabled)
    );
    assert_eq!(
        prepare(Mode::Unavailable, request, &[], [0; 32], root),
        Ok(Preparation::Unavailable)
    );
}
#[test]
fn root_and_exact_view_changes_invalidate_cache() {
    let (bytes, digest, root, other, base) = fixture();
    let original = ready(base.clone(), &bytes, digest, root);
    let output = b"hello";
    let hash = Sha256::digest(output).into();
    let mut changed = base.clone();
    changed.root = other;
    changed.selected = vec![other];
    let other_root = ready(changed, &bytes, digest, other);
    assert_eq!(
        original.validate_cached(&other_root, output, 5, hash),
        Err(Error::CacheMismatch)
    );
    let view = PortableView::import(&bytes, digest).unwrap();
    let mut claims = view.availability().clone();
    claims.get_mut(&root).unwrap().current = Availability::Missing;
    let (changed_bytes, changed_digest) = PortableView::new(view.view().clone(), claims)
        .unwrap()
        .export()
        .unwrap();
    let mut changed = base;
    changed.view_digest = changed_digest;
    changed.limits.input_bytes = changed_bytes.len();
    let other_view = ready(changed, &changed_bytes, changed_digest, root);
    assert_eq!(
        original.validate_cached(&other_view, output, 5, hash),
        Err(Error::CacheMismatch)
    );
}
