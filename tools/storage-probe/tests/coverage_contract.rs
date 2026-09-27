//! Black-box E1R contract probes. Expected rows are computed here, never by coverage.
use std::collections::BTreeSet;
use std::num::NonZeroUsize;

use fabric_o11y::{
    Attribute, Event, EventId, EventTime, ObservedTime, Payload, ResourceId, Scalar, SourceId,
    TenantId,
};
use storage_probe::Query;
use storage_probe::coverage::{
    self, BlockCommitment, BlockReceipt, CoverageError, CoverageStatus, Disposition, Receipt,
    SealedSnapshot, Summary,
};

fn event(id: u64, tenant: u64, time: i64, body: &str) -> Event {
    Event {
        id: EventId(id),
        tenant: TenantId(tenant),
        source: SourceId(2),
        resource: ResourceId(3),
        event_time: EventTime(time),
        observed_time: ObservedTime(time),
        attributes: vec![],
        payload: Payload::Log { body: body.into() },
    }
}
fn query(start_ns: i64, end_ns: i64, tenant: Option<u64>, token: Option<&str>) -> Query {
    Query {
        start_ns,
        end_ns,
        tenant,
        token: token.map(str::to_owned),
    }
}
fn scan(rows: &[Event], q: &Query) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter_map(|(pos, row)| {
            if row.event_time.0 < q.start_ns || row.event_time.0 > q.end_ns {
                return None;
            }
            if q.tenant.is_some_and(|t| t != row.tenant.0) {
                return None;
            }
            if let Some(token) = &q.token {
                match &row.payload {
                    Payload::Log { body } if body.split_whitespace().any(|part| part == token) => {}
                    _ => return None,
                }
            }
            Some(pos)
        })
        .collect()
}
fn seal(rows: Vec<Event>, size: usize) -> SealedSnapshot {
    SealedSnapshot::new(rows, NonZeroUsize::new(size).unwrap(), 17, None).unwrap()
}

#[test]
fn exact_positional_scan_unicode_and_empty_tokens() {
    let rows = vec![
        event(7, 1, i64::MIN, "rare\u{2003}β"),
        event(7, 1, 0, "rarely rare"),
        event(7, 2, i64::MAX, "β\tcommon"),
        event(8, 1, 0, ""),
    ];
    let qs = [
        query(i64::MIN, i64::MAX, None, None),
        query(i64::MIN, i64::MIN, None, Some("β")),
        query(0, 0, Some(1), Some("rare")),
        query(0, 0, None, Some("rarely")),
        query(i64::MIN, i64::MAX, None, Some("rare β")),
        query(i64::MIN, i64::MAX, None, Some("")),
        query(i64::MAX, i64::MIN, None, None),
        query(i64::MAX, i64::MAX, Some(2), Some("common")),
    ];
    let expected: Vec<_> = qs.iter().map(|q| scan(&rows, q)).collect();
    let snapshot = seal(rows, 2);
    for (q, want) in qs.iter().zip(expected) {
        let answer = snapshot.query(q, &[true, true]).unwrap();
        assert_eq!(answer.positions, want, "query {q:?}");
        assert_eq!(
            coverage::verify(snapshot.anchor(), q, &answer.receipt),
            Ok(CoverageStatus::Complete)
        );
    }
}

#[test]
fn empty_single_and_odd_trees() {
    for count in [0, 1, 3, 5] {
        let rows: Vec<_> = (0..count).map(|i| event(1, 1, i as i64, "word")).collect();
        let snapshot = seal(rows, 1);
        let q = query(i64::MIN, i64::MAX, None, None);
        let answer = snapshot.query(&q, &vec![true; count]).unwrap();
        assert_eq!(answer.positions, (0..count).collect::<Vec<_>>());
        assert_eq!(
            coverage::verify(snapshot.anchor(), &q, &answer.receipt),
            Ok(CoverageStatus::Complete)
        );
        assert_eq!(snapshot.anchor().block_count, count);
        assert_eq!(snapshot.anchor().row_count, count);
        assert!(snapshot.query(&q, &vec![true; count + 1]).is_err());
    }
    assert!(matches!(
        coverage::summarize(&[]),
        Err(CoverageError::InvalidSummary)
    ));
}

#[test]
fn builder_rejects_omissions_and_accepts_conservative_summaries() {
    let rows = vec![event(1, 1, -10, "rare β"), event(2, 2, 10, "common")];
    let exact = coverage::summarize(&rows).unwrap();
    let mut variants = Vec::new();
    let mut x = exact.clone();
    x.min_time = -9;
    variants.push(x);
    let mut x = exact.clone();
    x.max_time = 9;
    variants.push(x);
    let mut x = exact.clone();
    x.tenants.remove(&2);
    variants.push(x);
    let mut x = exact.clone();
    x.tokens.remove("β");
    variants.push(x);
    for bad in variants {
        assert_eq!(
            coverage::validate_summary(&rows, &bad),
            Err(CoverageError::InvalidSummary)
        );
        let owned = vec![event(1, 1, -10, "rare β"), event(2, 2, 10, "common")];
        assert!(matches!(
            SealedSnapshot::new(owned, NonZeroUsize::new(2).unwrap(), 17, Some(vec![bad])),
            Err(CoverageError::InvalidSummary)
        ));
    }
    let mut conservative = exact;
    conservative.min_time = i64::MIN;
    conservative.max_time = i64::MAX;
    conservative.tenants.insert(999);
    conservative.tokens.insert("absent".into());
    assert_eq!(coverage::validate_summary(&rows, &conservative), Ok(()));
    let owned = vec![event(1, 1, -10, "rare β"), event(2, 2, 10, "common")];
    assert!(
        SealedSnapshot::new(
            owned,
            NonZeroUsize::new(2).unwrap(),
            17,
            Some(vec![conservative])
        )
        .is_ok()
    );
    assert!(matches!(
        SealedSnapshot::new(
            vec![event(1, 1, 0, "x")],
            NonZeroUsize::new(1).unwrap(),
            17,
            Some(vec![])
        ),
        Err(CoverageError::InvalidSummary)
    ));
    assert!(SealedSnapshot::new(vec![], NonZeroUsize::new(1).unwrap(), 17, Some(vec![])).is_ok());
}

#[test]
fn row_digest_binds_every_event_field_and_float_bits() {
    let base = || Event {
        id: EventId(1),
        tenant: TenantId(2),
        source: SourceId(3),
        resource: ResourceId(4),
        event_time: EventTime(5),
        observed_time: ObservedTime(6),
        attributes: vec![Attribute {
            key: "key".into(),
            value: Scalar::Bool(true),
        }],
        payload: Payload::Gauge {
            name: "cpu".into(),
            value: f64::from_bits(0x7ff8_0000_0000_0001),
            unit: "ms".into(),
        },
    };
    let first = coverage::rows_digest(&[base()]);
    let mut variants = Vec::new();
    let mut x = base();
    x.id.0 += 1;
    variants.push(x);
    let mut x = base();
    x.tenant.0 += 1;
    variants.push(x);
    let mut x = base();
    x.source.0 += 1;
    variants.push(x);
    let mut x = base();
    x.resource.0 += 1;
    variants.push(x);
    let mut x = base();
    x.event_time.0 += 1;
    variants.push(x);
    let mut x = base();
    x.observed_time.0 += 1;
    variants.push(x);
    let mut x = base();
    x.attributes[0].key.push('!');
    variants.push(x);
    for value in [
        Scalar::Bool(false),
        Scalar::I64(-1),
        Scalar::U64(1),
        Scalar::F64(-0.0),
        Scalar::String("x".into()),
    ] {
        let mut x = base();
        x.attributes[0].value = value;
        variants.push(x);
    }
    let mut x = base();
    x.attributes.push(Attribute {
        key: "key".into(),
        value: Scalar::Bool(true),
    });
    variants.push(x);
    let mut x = base();
    x.payload = Payload::Gauge {
        name: "cpu!".into(),
        value: f64::from_bits(0x7ff8_0000_0000_0001),
        unit: "ms".into(),
    };
    variants.push(x);
    let mut x = base();
    x.payload = Payload::Gauge {
        name: "cpu".into(),
        value: f64::from_bits(0x7ff8_0000_0000_0002),
        unit: "ms".into(),
    };
    variants.push(x);
    let mut x = base();
    x.payload = Payload::Gauge {
        name: "cpu".into(),
        value: f64::from_bits(0x7ff8_0000_0000_0001),
        unit: "ns".into(),
    };
    variants.push(x);
    let mut x = base();
    x.payload = Payload::Log { body: "cpu".into() };
    variants.push(x);
    for (index, x) in variants.iter().enumerate() {
        assert_ne!(
            coverage::rows_digest(std::slice::from_ref(x)),
            first,
            "field mutation {index}"
        );
    }
    assert_ne!(coverage::rows_digest(&[base(), base()]), first);
    assert_ne!(
        coverage::rows_digest(&[event(1, 1, 0, "ab"), event(2, 1, 0, "c")]),
        coverage::rows_digest(&[event(1, 1, 0, "a"), event(2, 1, 0, "bc")])
    );
}

#[test]
fn receipt_authentication_binding_and_layout_errors() {
    let snapshot = seal((0..5).map(|i| event(i, 1, i as i64, "rare")).collect(), 1);
    let q = query(i64::MIN, i64::MAX, None, None);
    let receipt = snapshot.query(&q, &[true; 5]).unwrap().receipt;
    assert_eq!(
        coverage::verify(snapshot.anchor(), &q, &receipt),
        Ok(CoverageStatus::Complete)
    );
    let mut wrong = receipt.clone();
    wrong.query.token = Some("rare".into());
    assert_eq!(
        coverage::verify(snapshot.anchor(), &q, &wrong),
        Err(CoverageError::WrongQuery)
    );
    let mut wrong = receipt.clone();
    wrong.snapshot_id += 1;
    assert_eq!(
        coverage::verify(snapshot.anchor(), &q, &wrong),
        Err(CoverageError::WrongSnapshot)
    );
    let mut anchor = snapshot.anchor().clone();
    anchor.root[0] ^= 1;
    assert!(
        coverage::verify(&anchor, &q, &receipt).is_err(),
        "must compare proof to independently retained root"
    );
    let mut bad = receipt.clone();
    bad.blocks.remove(2);
    assert!(coverage::verify(snapshot.anchor(), &q, &bad).is_err());
    let mut bad = receipt.clone();
    bad.blocks.swap(1, 2);
    assert!(coverage::verify(snapshot.anchor(), &q, &bad).is_err());
    let mut bad = receipt.clone();
    bad.blocks[1].proof.pop();
    assert!(coverage::verify(snapshot.anchor(), &q, &bad).is_err());
    let mut bad = receipt.clone();
    bad.blocks[1].proof[0][0] ^= 1;
    assert!(coverage::verify(snapshot.anchor(), &q, &bad).is_err());
    let mut bad = receipt.clone();
    bad.blocks[1].commitment.start += 1;
    assert!(coverage::verify(snapshot.anchor(), &q, &bad).is_err());
    let mut bad = receipt.clone();
    bad.blocks[1].commitment.rows_digest[0] ^= 1;
    assert!(coverage::verify(snapshot.anchor(), &q, &bad).is_err());
    let mut bad = receipt.clone();
    bad.blocks[1].commitment.len = 0;
    assert!(coverage::verify(snapshot.anchor(), &q, &bad).is_err());
    let mut anchor = snapshot.anchor().clone();
    anchor.block_count -= 1;
    assert!(coverage::verify(&anchor, &q, &receipt).is_err());
    let mut anchor = snapshot.anchor().clone();
    anchor.row_count = usize::MAX;
    assert!(coverage::verify(&anchor, &q, &receipt).is_err());
    let mut bad = receipt.clone();
    bad.blocks[4].commitment.start = usize::MAX;
    bad.blocks[4].commitment.len = 2;
    assert!(
        coverage::verify(snapshot.anchor(), &q, &bad).is_err(),
        "range addition must be checked"
    );
}

#[test]
fn authenticated_malformed_layouts_still_fail_verification() {
    let q = query(i64::MIN, i64::MAX, None, None);
    let commitment = |ordinal, start, len| BlockCommitment {
        ordinal,
        start,
        len,
        rows_digest: [3; 32],
        summary_digest: [4; 32],
    };
    let cases = [
        (2, vec![commitment(0, 0, 0), commitment(1, 0, 2)]),
        (2, vec![commitment(0, 0, 1), commitment(1, 2, 1)]),
        (2, vec![commitment(0, 0, 1), commitment(0, 1, 1)]),
        (2, vec![commitment(1, 0, 1), commitment(0, 1, 1)]),
        (usize::MAX, vec![commitment(0, usize::MAX, 2)]),
    ];
    for (index, (row_count, commitments)) in cases.into_iter().enumerate() {
        let (anchor, proofs) = coverage::authenticate(5, row_count, &commitments);
        let receipt = Receipt {
            snapshot_id: 5,
            query: q.clone(),
            blocks: commitments
                .into_iter()
                .zip(proofs)
                .map(|(commitment, proof)| BlockReceipt {
                    commitment,
                    proof,
                    disposition: Disposition::Scanned,
                })
                .collect(),
        };
        assert!(
            coverage::verify(&anchor, &q, &receipt).is_err(),
            "authenticated malformed layout {index}"
        );
    }
    let false_summary = Summary {
        min_time: 0,
        max_time: 0,
        tenants: BTreeSet::from([1]),
        tokens: BTreeSet::from(["rare".into()]),
    };
    let matching_commitment = BlockCommitment {
        summary_digest: coverage::summary_digest(&false_summary),
        ..commitment(0, 0, 1)
    };
    let (anchor, proofs) = coverage::authenticate(5, 1, &[matching_commitment.clone()]);
    let receipt = Receipt {
        snapshot_id: 5,
        query: query(i64::MIN, i64::MAX, None, Some("rare")),
        blocks: vec![BlockReceipt {
            commitment: matching_commitment,
            proof: proofs[0].clone(),
            disposition: Disposition::Excluded(false_summary),
        }],
    };
    assert!(
        coverage::verify(&anchor, &receipt.query, &receipt).is_err(),
        "matching summary cannot justify exclusion"
    );
}

#[test]
fn dispositions_and_unavailability() {
    let rows = vec![
        event(1, 1, 0, "rare"),
        event(2, 1, 1, "rare"),
        event(3, 2, 10, "common"),
        event(4, 2, 11, "common"),
    ];
    let snapshot = seal(rows, 2);
    let q = query(0, 11, None, Some("rare"));
    let answer = snapshot.query(&q, &[false, false]).unwrap();
    assert!(answer.positions.is_empty());
    assert_eq!(
        coverage::verify(snapshot.anchor(), &q, &answer.receipt),
        Ok(CoverageStatus::Incomplete {
            unavailable: vec![0]
        })
    );
    assert!(matches!(
        answer.receipt.blocks[1].disposition,
        Disposition::Excluded(_)
    ));
    let mut bad = answer.receipt.clone();
    bad.blocks[0].disposition = Disposition::Excluded(
        coverage::summarize(&[event(1, 1, 0, "rare"), event(2, 1, 1, "rare")]).unwrap(),
    );
    assert!(coverage::verify(snapshot.anchor(), &q, &bad).is_err());
    let mut bad = answer.receipt.clone();
    bad.blocks[1].disposition = Disposition::Unavailable;
    assert_eq!(
        coverage::verify(snapshot.anchor(), &q, &bad),
        Ok(CoverageStatus::Incomplete {
            unavailable: vec![0, 1]
        })
    );
    let mut bad = answer.receipt.clone();
    if let Disposition::Excluded(ref mut summary) = bad.blocks[1].disposition {
        summary.tokens.insert("tamper".into());
    }
    assert!(coverage::verify(snapshot.anchor(), &q, &bad).is_err());
}

#[test]
fn authenticated_dishonest_builder_is_visible_only_to_row_oracle() {
    let rows = vec![event(1, 1, 0, "rare")];
    let false_summary = Summary {
        min_time: 0,
        max_time: 0,
        tenants: BTreeSet::from([1]),
        tokens: BTreeSet::from(["common".into()]),
    };
    assert_eq!(
        coverage::validate_summary(&rows, &false_summary),
        Err(CoverageError::InvalidSummary)
    );
    let commitment = BlockCommitment {
        ordinal: 0,
        start: 0,
        len: 1,
        rows_digest: coverage::rows_digest(&rows),
        summary_digest: coverage::summary_digest(&false_summary),
    };
    let (anchor, proofs) = coverage::authenticate(77, 1, &[commitment.clone()]);
    let q = query(i64::MIN, i64::MAX, None, Some("rare"));
    let receipt = Receipt {
        snapshot_id: 77,
        query: q.clone(),
        blocks: vec![BlockReceipt {
            commitment,
            proof: proofs[0].clone(),
            disposition: Disposition::Excluded(false_summary),
        }],
    };
    assert_eq!(
        coverage::verify(&anchor, &q, &receipt),
        Ok(CoverageStatus::Complete)
    );
    assert_eq!(
        scan(&rows, &q),
        vec![0],
        "semantic omission survives cryptographic verification"
    );
}
