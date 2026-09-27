//! E3R black-box contract probes, written from RESUME_API.md before resume exists.
use fabric_o11y::{
    Event, EventId, EventTime, ObservedTime, Payload, ResourceId, SourceId, TenantId,
};
use std::num::NonZeroUsize;
use storage_probe::Query;
use storage_probe::coverage::{self, CoverageStatus, Disposition, SealedSnapshot};
use storage_probe::resume::{self, Accumulator, Binding, MatchedRow, Page, Residual, ResumeError};

fn event(id: u64, time: i64, body: &str) -> Event {
    Event {
        id: EventId(id),
        tenant: TenantId(1),
        source: SourceId(2),
        resource: ResourceId(3),
        event_time: EventTime(time),
        observed_time: ObservedTime(time),
        attributes: vec![],
        payload: Payload::Log { body: body.into() },
    }
}
fn q(token: Option<&str>) -> Query {
    Query {
        start_ns: i64::MIN,
        end_ns: i64::MAX,
        tenant: None,
        token: token.map(str::to_owned),
    }
}
fn seal(rows: Vec<Event>, id: u64, size: usize) -> SealedSnapshot {
    SealedSnapshot::new(rows, NonZeroUsize::new(size).unwrap(), id, None).unwrap()
}
fn binding(s: &SealedSnapshot, query: Query) -> Binding {
    Binding {
        anchor: s.anchor().clone(),
        query,
        tokenizer_version: resume::TOKENIZER_VERSION,
        order_version: resume::ORDER_VERSION,
    }
}
fn expected(rows: &[Event], query: &Query, size: usize) -> Vec<MatchedRow> {
    rows.iter().enumerate().filter(|(_,e)| e.event_time.0 >= query.start_ns && e.event_time.0 <= query.end_ns
        && query.tenant.is_none_or(|t| t == e.tenant.0)
        && query.token.as_ref().is_none_or(|t| matches!(&e.payload, Payload::Log { body } if body.split_whitespace().any(|w| w == t))))
        .map(|(p,e)| MatchedRow { block: p/size, offset: p%size, digest: coverage::rows_digest(std::slice::from_ref(e)) }).collect()
}
fn assert_unchanged(
    acc: &Accumulator,
    rows: &[MatchedRow],
    status: &CoverageStatus,
    residual: &Residual,
) {
    assert_eq!(acc.rows(), rows);
    assert_eq!(&acc.status(), status);
    assert_eq!(&acc.residual(), residual);
}
fn reject_atomic(acc: &mut Accumulator, page: Page) {
    let rows = acc.rows();
    let status = acc.status();
    let residual = acc.residual();
    assert!(acc.merge(page).is_err());
    assert_unchanged(acc, &rows, &status, &residual);
}

#[test]
fn exact_adapters_resume_and_permanent_outstanding() {
    let rows = vec![
        event(1, i64::MIN, "rare\u{2003}β"),
        event(1, 0, "rare β"),
        event(2, 1, "ordinary"),
        event(2, i64::MAX, "β\tcommon"),
    ];
    let query = q(Some("β"));
    let want = expected(&rows, &query, 1);
    let snap = seal(rows, 11, 1);
    let b = binding(&snap, query);
    let first = resume::execute(&snap, &b, &[true, false, false, false]).unwrap();
    assert_eq!(first.rows, want[..1]);
    let mut acc = Accumulator::new(b.clone(), first).unwrap();
    assert_eq!(
        acc.status(),
        CoverageStatus::Incomplete {
            unavailable: vec![1, 3]
        }
    );
    assert_eq!(
        acc.residual()
            .blocks
            .iter()
            .map(|c| c.ordinal)
            .collect::<Vec<_>>(),
        vec![1, 3]
    );
    let subset = Residual {
        binding: b.clone(),
        blocks: vec![acc.residual().blocks[0].clone()],
    };
    let page = resume::resume(&snap, &subset, &[false, true, false, true]).unwrap();
    assert_eq!(page.rows, want[1..2]);
    acc.merge(page).unwrap();
    assert_eq!(
        acc.status(),
        CoverageStatus::Incomplete {
            unavailable: vec![3]
        }
    );
    assert_eq!(
        acc.residual()
            .blocks
            .iter()
            .map(|c| c.ordinal)
            .collect::<Vec<_>>(),
        vec![3]
    );
    let last = resume::resume(&snap, &acc.residual(), &[false, false, false, true]).unwrap();
    acc.merge(last).unwrap();
    assert_eq!(acc.status(), CoverageStatus::Complete);
    assert_eq!(acc.rows(), want);
    assert_eq!(acc.positions(), vec![0, 1, 3]);
    assert!(acc.residual().blocks.is_empty());
    let empty = resume::resume(&snap, &acc.residual(), &[false; 4]).unwrap();
    acc.merge(empty).unwrap();
    assert_eq!(acc.rows(), want);
}

#[test]
fn distinct_identical_rows_and_entire_block_conflicts() {
    let rows = vec![event(7, 0, "β"), event(7, 0, "β"), event(8, 1, "β")];
    let query = q(None);
    let want = expected(&rows, &query, 2);
    let snap = seal(rows, 17, 2);
    let b = binding(&snap, query);
    let full = resume::execute(&snap, &b, &[true, true]).unwrap();
    let mut acc = Accumulator::new(b.clone(), full.clone()).unwrap();
    assert_eq!(acc.rows(), want);
    assert_eq!(acc.positions(), vec![0, 1, 2]);
    assert_eq!(want[0].digest, want[1].digest);
    acc.merge(full.clone()).unwrap();
    assert_eq!(acc.rows(), want);
    for row_index in [0, 1] {
        let mut changed = full.clone();
        changed.rows[row_index].digest[0] ^= 1;
        reject_atomic(&mut acc, changed);
    }
    let mut omitted = full.clone();
    omitted.rows.remove(0);
    reject_atomic(&mut acc, omitted);
    let mut added = full.clone();
    added.rows.push(full.rows[0].clone());
    reject_atomic(&mut acc, added);
    let mut duplicate = full.clone();
    duplicate.rows.insert(1, duplicate.rows[0].clone());
    reject_atomic(&mut acc, duplicate);
    let mut out_of_range = full.clone();
    out_of_range.rows[0].offset = 2;
    reject_atomic(&mut acc, out_of_range);
    let mut wrong_block = full.clone();
    wrong_block.rows[0].block = 2;
    reject_atomic(&mut acc, wrong_block);
    let mut unsorted = full.clone();
    unsorted.rows.swap(0, 1);
    reject_atomic(&mut acc, unsorted);
    let mut unavailable_with_row = full.clone();
    unavailable_with_row.receipt.blocks[0].disposition = Disposition::Unavailable;
    reject_atomic(&mut acc, unavailable_with_row);
    let mut excluded_with_row = full.clone();
    excluded_with_row.receipt.blocks[0].disposition = Disposition::Unavailable;
    reject_atomic(&mut acc, excluded_with_row);
}

#[test]
fn bindings_layout_and_residual_validation() {
    let rows = vec![event(1, 0, "a"), event(2, 1, "b"), event(3, 2, "c")];
    let snap = seal(
        vec![event(1, 0, "a"), event(2, 1, "b"), event(3, 2, "c")],
        23,
        1,
    );
    let b = binding(&snap, q(None));
    let page = resume::execute(&snap, &b, &[false; 3]).unwrap();
    let mut acc = Accumulator::new(b.clone(), page.clone()).unwrap();
    let mut variants = Vec::new();
    let mut p = page.clone();
    p.binding.anchor.snapshot_id += 1;
    variants.push(p);
    let mut p = page.clone();
    p.binding.anchor.root[0] ^= 1;
    variants.push(p);
    let mut p = page.clone();
    p.binding.anchor.row_count += 1;
    variants.push(p);
    let mut p = page.clone();
    p.binding.query.token = Some("x".into());
    variants.push(p);
    let mut p = page.clone();
    p.binding.tokenizer_version += 1;
    variants.push(p);
    let mut p = page.clone();
    p.binding.order_version += 1;
    variants.push(p);
    let mut p = page.clone();
    p.receipt.blocks.remove(0);
    variants.push(p);
    let mut p = page.clone();
    p.receipt.blocks.swap(0, 1);
    variants.push(p);
    let mut p = page.clone();
    p.receipt.blocks[0].commitment.rows_digest[0] ^= 1;
    variants.push(p);
    for p in variants {
        reject_atomic(&mut acc, p);
    }
    let mut bad = b.clone();
    bad.tokenizer_version += 1;
    assert!(resume::execute(&snap, &bad, &[true; 3]).is_err());
    assert!(resume::execute(&snap, &b, &[true; 2]).is_err());
    assert!(resume::execute(&snap, &b, &[true; 4]).is_err());
    let other = seal(rows, 24, 1);
    assert!(resume::execute(&other, &b, &[true; 3]).is_err());
    let residual = acc.residual();
    let mut invalid = Vec::new();
    let mut r = residual.clone();
    r.blocks.swap(0, 1);
    invalid.push(r);
    let mut r = residual.clone();
    r.blocks.push(r.blocks[0].clone());
    invalid.push(r);
    let mut r = residual.clone();
    r.blocks[0].rows_digest[0] ^= 1;
    invalid.push(r);
    let mut r = residual.clone();
    r.blocks[0].start += 1;
    invalid.push(r);
    let mut r = residual.clone();
    r.binding.order_version += 1;
    invalid.push(r);
    for r in invalid {
        assert!(resume::resume(&snap, &r, &[true; 3]).is_err());
    }
    assert!(resume::resume(&snap, &residual, &[true; 2]).is_err());
    assert!(resume::resume(&other, &residual, &[true; 3]).is_err());
    assert!(matches!(
        Accumulator::new(b.clone(), {
            let mut p = page;
            p.rows.push(MatchedRow {
                block: 0,
                offset: 0,
                digest: [0; 32],
            });
            p
        }),
        Err(ResumeError::InvalidRows)
    ));
}

#[test]
fn empty_snapshot_and_inverted_query() {
    let empty = seal(vec![], 90, 1);
    let b = binding(&empty, q(None));
    let p = resume::execute(&empty, &b, &[]).unwrap();
    let acc = Accumulator::new(b, p).unwrap();
    assert_eq!(acc.status(), CoverageStatus::Complete);
    assert!(acc.rows().is_empty());
    assert!(acc.residual().blocks.is_empty());
    let one = seal(vec![event(1, i64::MIN, "a")], 91, 1);
    let inverted = Query {
        start_ns: i64::MAX,
        end_ns: i64::MIN,
        tenant: None,
        token: None,
    };
    let b = binding(&one, inverted);
    let p = resume::execute(&one, &b, &[false]).unwrap();
    let acc = Accumulator::new(b, p).unwrap();
    assert_eq!(acc.status(), CoverageStatus::Complete);
    assert!(acc.rows().is_empty());
}

#[test]
fn added_row_conflicts_even_when_coordinates_are_well_formed() {
    let rows = vec![event(1, 0, "match"), event(2, 1, "other")];
    let query = q(Some("match"));
    let digest_other = coverage::rows_digest(std::slice::from_ref(&rows[1]));
    let snap = seal(rows, 101, 2);
    let b = binding(&snap, query);
    let full = resume::execute(&snap, &b, &[true]).unwrap();
    assert_eq!(full.rows.len(), 1);
    let mut acc = Accumulator::new(b, full.clone()).unwrap();
    let mut added = full;
    added.rows.push(MatchedRow {
        block: 0,
        offset: 1,
        digest: digest_other,
    });
    reject_atomic(&mut acc, added);
}

#[test]
fn safe_empty_scanned_and_excluded_agree_but_faulty_summary_trust_remains_limited() {
    use std::collections::BTreeSet;
    use storage_probe::coverage::{BlockCommitment, BlockReceipt, Receipt, Summary};
    let original = vec![event(1, 0, "rare")];
    let summary = Summary {
        min_time: 0,
        max_time: 0,
        tenants: BTreeSet::from([1]),
        tokens: BTreeSet::from(["common".to_owned()]),
    };
    assert!(coverage::validate_summary(&original, &summary).is_err());
    let commitment = BlockCommitment {
        ordinal: 0,
        start: 0,
        len: 1,
        rows_digest: coverage::rows_digest(&original),
        summary_digest: coverage::summary_digest(&summary),
    };
    let (anchor, proofs) = coverage::authenticate(777, 1, &[commitment.clone()]);
    let query = q(Some("rare"));
    let b = Binding {
        anchor,
        query: query.clone(),
        tokenizer_version: resume::TOKENIZER_VERSION,
        order_version: resume::ORDER_VERSION,
    };
    let receipt = Receipt {
        snapshot_id: 777,
        query,
        blocks: vec![BlockReceipt {
            commitment,
            proof: proofs[0].clone(),
            disposition: Disposition::Excluded(summary),
        }],
    };
    // The authenticated false summary says "common" only. Metadata verification
    // accepts its exclusion even though the original row matches "rare".
    assert_eq!(
        coverage::verify(&b.anchor, &b.query, &receipt),
        Ok(CoverageStatus::Complete)
    );
    let excluded = Page {
        binding: b.clone(),
        receipt: receipt.clone(),
        rows: vec![],
    };
    let mut acc = Accumulator::new(b.clone(), excluded).unwrap();
    assert_eq!(acc.status(), CoverageStatus::Complete);
    assert!(acc.rows().is_empty());
    let mut scanned = receipt;
    scanned.blocks[0].disposition = Disposition::Scanned;
    acc.merge(Page {
        binding: b,
        receipt: scanned,
        rows: vec![],
    })
    .unwrap();
    assert!(acc.rows().is_empty());
}
