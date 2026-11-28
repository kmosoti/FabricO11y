use fabric_corpus::{
    Corpus, Kind, Predicate, Relation, RelationInsertion, RevisionBody,
    as_of::AsOfView,
    portable::{Availability, AvailabilityClaim, PortableView},
    wiki::{self, Endpoint, Limits},
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
// Construct an independently encoded selected-missing record while preserving selected relations.
fn with_missing(mut bytes: Vec<u8>, target: [u8; 32]) -> (Vec<u8>, [u8; 32]) {
    let mut pos = b"fabric-corpus/view\0\x01".len();
    let count = u64::from_be_bytes(bytes[pos..pos + 8].try_into().unwrap());
    pos += 8;
    for _ in 0..count {
        let id: [u8; 32] = bytes[pos..pos + 32].try_into().unwrap();
        pos += 32;
        let tag = pos;
        assert_eq!(bytes[pos], 1);
        pos += 1;
        let len = u64::from_be_bytes(bytes[pos..pos + 8].try_into().unwrap()) as usize;
        pos += 8;
        let end = pos + len;
        if id == target {
            bytes.splice(tag..end, [0]);
            let digest = Sha256::digest(&bytes).into();
            return (bytes, digest);
        }
        pos = end + 2;
    }
    panic!("selected record absent")
}
fn body(name: &str) -> RevisionBody {
    RevisionBody {
        object_id: name.into(),
        kind: Kind::Observation,
        text: name.into(),
        parents: BTreeSet::new(),
        sources: vec![],
        extensions: BTreeMap::new(),
    }
}
fn edge(
    c: &mut Corpus,
    subject: [u8; 32],
    object: [u8; 32],
    who: &str,
    predicate: Predicate,
) -> [u8; 32] {
    let RelationInsertion::Inserted(id) = c
        .add_relation(Relation {
            subject,
            object,
            predicate,
            attributed_to: who.into(),
            extensions: BTreeMap::new(),
        })
        .unwrap()
    else {
        panic!("new relation")
    };
    id
}
#[test]
fn selected_backlinks_preserve_attribution_cycles_and_missingness() {
    let mut c = Corpus::default();
    let a = c.admit(body("a")).unwrap();
    let b = c.admit(body("b")).unwrap();
    let not_selected = c.admit(body("omitted")).unwrap();
    let missing = c.admit(body("selected-missing")).unwrap();
    let missing_edge = [9; 32];
    let pred = Predicate::Custom {
        namespace: "example".into(),
        name: "link".into(),
    };
    let ab1 = edge(&mut c, a, b, "alice", pred.clone());
    let ab2 = edge(&mut c, a, b, "bob", pred.clone());
    let ba = edge(&mut c, b, a, "carol", pred.clone());
    let ma = edge(&mut c, missing, a, "dave", pred.clone());
    let oa = edge(&mut c, not_selected, a, "eve", pred.clone());
    let am = edge(&mut c, a, missing, "helen", pred.clone());
    let ao = edge(&mut c, a, not_selected, "ivan", pred.clone());
    let ignored = edge(&mut c, a, b, "frank", Predicate::Supports);
    let selected = BTreeSet::from([a, b, missing]);
    let claims = selected
        .iter()
        .map(|id| {
            (
                *id,
                AvailabilityClaim {
                    archive: Availability::ReferenceOnly,
                    current: Availability::Missing,
                },
            )
        })
        .collect();
    let view = AsOfView::capture(
        &c,
        selected,
        BTreeSet::from([ab1, ab2, ba, ma, oa, am, ao, ignored, missing_edge]),
        BTreeSet::new(),
    )
    .unwrap();
    let (bytes, _) = PortableView::new(view, claims).unwrap().export().unwrap();
    let (bytes, digest) = with_missing(bytes, missing);
    let limits = Limits {
        input_bytes: bytes.len(),
        selected_relations: 9,
        links: 7,
        output_bytes: 2048,
    };
    let out = wiki::project(&bytes, digest, &pred, limits).unwrap();
    assert_eq!(out.predicate, pred);
    let support = wiki::project(&bytes, digest, &Predicate::Supports, limits).unwrap();
    assert_eq!(support.predicate, Predicate::Supports);
    assert_eq!(support.cost.links, 1);
    assert_ne!(support.predicate, out.predicate);
    assert_eq!(out.missing_relations, vec![missing_edge]);
    assert_eq!(out.cost.links, 7);
    assert_eq!(out.cost.selected_relations, 9);
    let links = &out.incoming[&b];
    assert_eq!(links.len(), 2);
    assert!(links.windows(2).all(|w| w[0].relation < w[1].relation));
    assert_eq!(
        links
            .iter()
            .map(|x| x.attributed_to.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["alice", "bob"])
    );
    assert!(links.iter().all(|x| x.subject == a
        && x.subject_state == Endpoint::Present
        && x.object_state == Endpoint::Present));
    let reverse = &out.incoming[&a];
    assert_eq!(reverse.len(), 3);
    assert!(
        reverse
            .iter()
            .any(|x| x.subject == b && x.subject_state == Endpoint::Present)
    );
    assert!(
        reverse
            .iter()
            .any(|x| x.subject == missing && x.subject_state == Endpoint::SelectedMissing)
    );
    assert!(
        reverse
            .iter()
            .any(|x| x.subject == not_selected && x.subject_state == Endpoint::Omitted)
    );
    assert_eq!(
        out.incoming[&missing][0].object_state,
        Endpoint::SelectedMissing
    );
    assert_eq!(
        out.incoming[&not_selected][0].object_state,
        Endpoint::Omitted
    );
    let restored = PortableView::import(&bytes, digest)
        .unwrap()
        .export()
        .unwrap();
    assert_eq!(
        wiki::project(&restored.0, restored.1, &pred, limits).unwrap(),
        out
    );
    let mut bad = bytes.clone();
    bad[0] ^= 1;
    assert!(matches!(
        wiki::project(&bad, digest, &pred, limits),
        Err(wiki::Error::Portable(_))
    ));
    let mut narrow = limits;
    narrow.links = 6;
    assert_eq!(
        wiki::project(&bytes, digest, &pred, narrow),
        Err(wiki::Error::Limit)
    );
    narrow = limits;
    narrow.output_bytes = out.cost.output_bytes - 1;
    assert_eq!(
        wiki::project(&bytes, digest, &pred, narrow),
        Err(wiki::Error::Limit)
    );
    narrow = limits;
    narrow.selected_relations = 8;
    assert_eq!(
        wiki::project(&bytes, digest, &pred, narrow),
        Err(wiki::Error::Limit)
    );
}
