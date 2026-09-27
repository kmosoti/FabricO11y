//! Additional selection probes, authored independently of either candidate.
use fabric_o11y::generator::{EventGenerator, WorkloadConfig};
use fabric_o11y::{Attribute, Payload, Scalar};
use std::num::NonZeroUsize;
use storage_probe::Query;
use storage_probe::coverage::{self, CoverageStatus, SealedSnapshot};

fn query() -> Query {
    Query {
        start_ns: i64::MIN,
        end_ns: i64::MAX,
        tenant: None,
        token: None,
    }
}

fn snapshot(count: u32) -> SealedSnapshot {
    let rows = EventGenerator::new(WorkloadConfig {
        seed: 17,
        events: count,
    })
    .collect();
    SealedSnapshot::new(rows, NonZeroUsize::new(1).unwrap(), 9, None).unwrap()
}

#[test]
fn empty_anchor_and_excess_proof_nodes_are_checked() {
    for count in [0, 1, 3, 6, 9] {
        let snapshot = snapshot(count);
        let answer = snapshot
            .query(&query(), &vec![true; count as usize])
            .unwrap();
        let mut anchor = snapshot.anchor().clone();
        anchor.root[31] ^= 1;
        assert!(coverage::verify(&anchor, &query(), &answer.receipt).is_err());
        if count > 0 {
            let mut bad = answer.receipt;
            bad.blocks.last_mut().unwrap().proof.push([0; 32]);
            assert!(coverage::verify(snapshot.anchor(), &query(), &bad).is_err());
        }
    }
}

#[test]
fn canonical_digest_preserves_field_boundaries_order_and_signed_zero() {
    let row = || {
        EventGenerator::new(WorkloadConfig {
            seed: 17,
            events: 1,
        })
        .next()
        .unwrap()
    };
    let mut a = row();
    let mut b = row();
    a.payload = Payload::Gauge {
        name: "a".into(),
        value: 0.0,
        unit: "bc".into(),
    };
    b.payload = Payload::Gauge {
        name: "ab".into(),
        value: 0.0,
        unit: "c".into(),
    };
    assert_ne!(coverage::rows_digest(&[a]), coverage::rows_digest(&[b]));
    let mut a = row();
    let mut b = row();
    a.attributes = vec![Attribute {
        key: "x".into(),
        value: Scalar::F64(0.0),
    }];
    b.attributes = vec![Attribute {
        key: "x".into(),
        value: Scalar::F64(-0.0),
    }];
    assert_ne!(coverage::rows_digest(&[a]), coverage::rows_digest(&[b]));
    let mut a = row();
    let mut b = row();
    let attrs = || {
        vec![
            Attribute {
                key: "a".into(),
                value: Scalar::Bool(true),
            },
            Attribute {
                key: "b".into(),
                value: Scalar::Bool(false),
            },
        ]
    };
    a.attributes = attrs();
    b.attributes = attrs();
    b.attributes.reverse();
    assert_ne!(coverage::rows_digest(&[a]), coverage::rows_digest(&[b]));
}

#[test]
fn self_authentication_cannot_replace_an_independent_anchor() {
    let snapshot = snapshot(3);
    let answer = snapshot.query(&query(), &[true; 3]).unwrap();
    let mut trusted = snapshot.anchor().clone();
    trusted.root[0] ^= 1;
    assert!(coverage::verify(&trusted, &query(), &answer.receipt).is_err());
    // Deliberately faulty baseline: derive authority from the response itself.
    // It ignores the independently retained `trusted` root and accepts substitution.
    let commitments: Vec<_> = answer
        .receipt
        .blocks
        .iter()
        .map(|b| b.commitment.clone())
        .collect();
    let (self_supplied, _) = coverage::authenticate(answer.receipt.snapshot_id, 3, &commitments);
    assert_eq!(
        coverage::verify(&self_supplied, &query(), &answer.receipt),
        Ok(CoverageStatus::Complete)
    );
}
