use fabric_corpus::{
    Attestation, Corpus, Error, Kind, MAX_DEPTH, Predicate, Relation, RevisionBody, SourceClaim,
    Value,
};
use std::collections::{BTreeMap, BTreeSet};

fn body(id: &str) -> RevisionBody {
    RevisionBody {
        object_id: id.into(),
        kind: Kind::Decision,
        text: "hello".into(),
        parents: BTreeSet::new(),
        sources: vec![],
        extensions: BTreeMap::new(),
    }
}

#[test]
fn fixed_profile_and_digest() {
    // Independently constructed framing: domain, id, kind, text, empty sets/maps.
    let mut expected = b"fabric-corpus/revision\0\x01".to_vec();
    expected.extend_from_slice(&1u64.to_be_bytes());
    expected.extend_from_slice(b"x");
    expected.push(5);
    expected.extend_from_slice(&5u64.to_be_bytes());
    expected.extend_from_slice(b"hello");
    expected.extend_from_slice(&[0; 24]);
    assert_eq!(body("x").canonical_bytes().unwrap(), expected);
    assert_eq!(
        body("x").digest().unwrap(),
        [
            0xb8, 0x16, 0x74, 0x1f, 0x49, 0xf7, 0x4e, 0x36, 0xc5, 0x77, 0xdf, 0x18, 0x22, 0x1a,
            0x4f, 0xa9, 0xe8, 0x66, 0x4a, 0xd3, 0xcf, 0xd0, 0x5a, 0xf3, 0x0c, 0xd7, 0x60, 0xd6,
            0xcd, 0xb4, 0xd8, 0x4f
        ]
    );
}

#[test]
fn canonical_extensions_and_bounds() {
    let mut a = body("x");
    let mut b = body("x");
    a.extensions
        .insert("é".into(), Value::Text("9007199254740993".into()));
    a.extensions.insert("a".into(), Value::Bool(true));
    b.extensions.insert("a".into(), Value::Bool(true));
    b.extensions
        .insert("é".into(), Value::Text("9007199254740993".into()));
    assert_eq!(a.canonical_bytes(), b.canonical_bytes());
    assert_ne!(a.digest(), body("x").digest());
    let mut nested = Value::Null;
    for _ in 0..=MAX_DEPTH {
        nested = Value::Array(vec![nested]);
    }
    a.extensions.insert("deep".into(), nested);
    assert_eq!(a.digest(), Err(Error::TooDeep));
}

#[test]
fn append_dag_relations_and_postcommit_attestation() {
    let mut c = Corpus::default();
    let root = c.admit(body("x")).unwrap();
    let mut child = body("x");
    child.text = "corrected".into();
    child.parents.insert(root);
    let next = c.admit(child).unwrap();
    let mut other = body("other");
    other.parents.insert(root);
    assert_eq!(c.admit(other), Err(Error::ParentObjectMismatch(root)));
    let mut missing = body("x");
    missing.parents.insert([7; 32]);
    assert_eq!(c.admit(missing), Err(Error::MissingParent([7; 32])));
    for (subject, object) in [(root, next), (next, root)] {
        c.add_relation(Relation {
            subject,
            predicate: Predicate::Supports,
            object,
            attributed_to: "alice".into(),
            extensions: BTreeMap::new(),
        })
        .unwrap();
    }
    c.add_attestation(Attestation {
        revision: root,
        attributed_to: "alice".into(),
        statement: "receipt claim".into(),
    })
    .unwrap();
    assert_eq!(c.revision(&root).unwrap().digest().unwrap(), root);
    assert_eq!(c.revision(&root).unwrap().text, "hello");
    assert_eq!(c.relations().len(), 2);
}

#[test]
fn source_claims_are_distinct_and_require_qualified_locators() {
    let mut captured = body("x");
    captured.sources.push(SourceClaim::Captured {
        capture_id: "capture-1".into(),
        artifact_sha256: [4; 32],
        locator: "urn:artifact:1".into(),
        parser: "parser-v1".into(),
    });
    let mut external = body("x");
    external.sources.push(SourceClaim::ExternalOnly {
        locator: "urn:artifact:1".into(),
        parser: "parser-v1".into(),
    });
    assert_ne!(captured.digest().unwrap(), external.digest().unwrap());
    external.sources = vec![SourceClaim::ExternalOnly {
        locator: "relative".into(),
        parser: "parser-v1".into(),
    }];
    assert_eq!(external.digest(), Err(Error::InvalidLocator));
}

#[test]
fn missing_cycles_and_concurrent_heads_are_not_collapsed() {
    let mut corpus = Corpus::default();
    let mut first = body("x");
    first.parents.insert([1; 32]);
    assert_eq!(corpus.admit(first), Err(Error::MissingParent([1; 32])));
    let mut second = body("x");
    second.parents.insert([2; 32]);
    assert_eq!(corpus.admit(second), Err(Error::MissingParent([2; 32])));
    let root = corpus.admit(body("x")).unwrap();
    let mut left = body("x");
    left.text = "left".into();
    left.parents.insert(root);
    let mut right = body("x");
    right.text = "right".into();
    right.parents.insert(root);
    let l = corpus.admit(left).unwrap();
    let r = corpus.admit(right).unwrap();
    assert_ne!(l, r);
    assert!(corpus.revision(&l).is_some() && corpus.revision(&r).is_some());
}

#[test]
fn byte_and_item_limits_are_enforced() {
    let mut large = body("x");
    large.text = "x".repeat(fabric_corpus::MAX_CANONICAL_BYTES);
    assert_eq!(large.digest(), Err(Error::TooLarge));
    let mut many = body("x");
    many.extensions.insert(
        "items".into(),
        Value::Array(vec![Value::Null; fabric_corpus::MAX_ITEMS]),
    );
    assert_eq!(many.digest(), Err(Error::TooManyItems));
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn complete_revision_golden() {
    let mut revision = body("x");
    revision.parents.extend([[2; 32], [1; 32]]);
    revision.sources = vec![
        SourceClaim::Captured {
            capture_id: "c1".into(),
            artifact_sha256: [4; 32],
            locator: "urn:a".into(),
            parser: "p1".into(),
        },
        SourceClaim::ExternalOnly {
            locator: "https://x".into(),
            parser: "p2".into(),
        },
    ];
    revision
        .extensions
        .insert("é".into(), Value::Text("9007199254740993".into()));
    revision.extensions.insert("a".into(), Value::Bool(true));
    assert_eq!(
        hex(&revision.canonical_bytes().unwrap()),
        include_str!("fixtures/revision-complete.hex").trim()
    );
    assert_eq!(
        hex(&revision.digest().unwrap()),
        include_str!("fixtures/revision-complete.sha256").trim()
    );
}

#[test]
fn relation_golden_and_idempotence() {
    let relation = Relation {
        subject: [1; 32],
        predicate: Predicate::Custom {
            namespace: "research".into(),
            name: "supports".into(),
        },
        object: [2; 32],
        attributed_to: "alice".into(),
        extensions: BTreeMap::from([("note".into(), Value::Text("claim".into()))]),
    };
    assert_eq!(
        hex(&relation.canonical_bytes().unwrap()),
        include_str!("fixtures/relation.hex").trim()
    );
    assert_eq!(
        hex(&relation.digest().unwrap()),
        include_str!("fixtures/relation.sha256").trim()
    );
    let mut changed = relation.clone();
    changed.attributed_to = "bob".into();
    assert_ne!(relation.digest(), changed.digest());

    let mut c = Corpus::default();
    let a = c.admit(body("a")).unwrap();
    let b = c.admit(body("b")).unwrap();
    let mut actual = relation;
    actual.subject = a;
    actual.object = b;
    let digest = actual.digest().unwrap();
    assert_eq!(
        c.add_relation(actual.clone()),
        Ok(fabric_corpus::RelationInsertion::Inserted(digest))
    );
    assert_eq!(
        c.add_relation(actual.clone()),
        Ok(fabric_corpus::RelationInsertion::Duplicate(digest))
    );
    actual.attributed_to = "bob".into();
    let bob = actual.digest().unwrap();
    assert_eq!(
        c.add_relation(actual.clone()),
        Ok(fabric_corpus::RelationInsertion::Inserted(bob))
    );
    actual.subject = b;
    actual.object = a;
    let reverse = actual.digest().unwrap();
    assert_eq!(
        c.add_relation(actual),
        Ok(fabric_corpus::RelationInsertion::Inserted(reverse))
    );
    assert_eq!(c.relations().len(), 3);
}
