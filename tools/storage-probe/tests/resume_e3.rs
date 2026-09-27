//! Frozen E3R corpus. The scalar oracle and fixture plan use original events and E1
//! authenticated metadata only; no resume output supplies an expected answer.
use fabric_o11y::generator::{EventGenerator, WorkloadConfig};
use fabric_o11y::{Event, Payload, TenantId};
use std::num::NonZeroUsize;
use storage_probe::Query;
use storage_probe::coverage::{self, CoverageStatus, Disposition, Receipt, SealedSnapshot};
use storage_probe::resume::{self, Accumulator, Binding, MatchedRow, Page, Residual};

const ROWS: usize = 2048;
const BLOCK: usize = 64;
const BLOCKS: usize = ROWS / BLOCK;
const QUERIES: usize = 128;
const MASKS: usize = 1000;
const SUCCESSORS: usize = 50;

fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut x = *state;
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}
fn workload(shape: &str, seed: u64) -> (Vec<Event>, i64) {
    let mut events: Vec<_> = EventGenerator::new(WorkloadConfig {
        seed,
        events: ROWS as u32,
    })
    .collect();
    let base = events[0].event_time.0;
    for (i, e) in events.iter_mut().enumerate() {
        e.tenant = TenantId((i as u64 * 17 + seed) % 1024 + 1);
        if shape == "shuffled_logs" {
            e.event_time.0 = base + ((i as u64 * 109 + seed) % ROWS as u64) as i64 * 1_000_000;
        }
        if shape == "shuffled_logs" || i % 2 == 0 {
            let term = if i % 257 == 0 { "rare" } else { "normal" };
            e.payload = Payload::Log {
                body: format!(
                    "common service{} request{i} {term} {}",
                    i % 16,
                    "x".repeat(128)
                ),
            };
        }
    }
    // Every tenth position, beginning at 10, copies its predecessor's EventId.
    // Exactly 204 of 2048 physical positions have a duplicate ID; rows remain distinct.
    for i in (10..ROWS).step_by(10) {
        events[i].id.0 = events[i - 1].id.0;
    }
    (events, base)
}
fn queries(base: i64) -> Vec<Query> {
    let mut rng = 9_u64;
    (0..QUERIES)
        .map(|i| {
            let offset = next(&mut rng) % ROWS as u64;
            let tenant = next(&mut rng) % 1024 + 1;
            let mut q = Query {
                start_ns: i64::MIN,
                end_ns: i64::MAX,
                tenant: None,
                token: None,
            };
            match i % 8 {
                1 | 6 => {
                    q.start_ns = base + offset as i64 * 1_000_000;
                    q.end_ns = q.start_ns + 31_000_000;
                    if i % 8 == 6 {
                        q.tenant = Some(tenant);
                        q.token = Some("rare".into());
                    }
                }
                2 => q.tenant = Some(tenant),
                3 => q.token = Some("rare".into()),
                4 => q.token = Some("absent".into()),
                5 => q.token = Some("common".into()),
                7 => {
                    q.start_ns = 1;
                    q.end_ns = 0;
                }
                _ => {}
            }
            q
        })
        .collect()
}
fn matches(e: &Event, q: &Query) -> bool {
    e.event_time.0 >= q.start_ns
        && e.event_time.0 <= q.end_ns
        && q.tenant.is_none_or(|t| t == e.tenant.0)
        && q.token.as_ref().is_none_or(
            |t| matches!(&e.payload, Payload::Log{body} if body.split_whitespace().any(|w|w==t)),
        )
}
fn scalar(rows: &[Event], q: &Query) -> Vec<MatchedRow> {
    rows.iter()
        .enumerate()
        .filter(|(_, e)| matches(e, q))
        .map(|(p, e)| MatchedRow {
            block: p / BLOCK,
            offset: p % BLOCK,
            digest: coverage::rows_digest(std::slice::from_ref(e)),
        })
        .collect()
}
fn binding(s: &SealedSnapshot, query: Query) -> Binding {
    Binding {
        anchor: s.anchor().clone(),
        query,
        tokenizer_version: resume::TOKENIZER_VERSION,
        order_version: resume::ORDER_VERSION,
    }
}
fn page_from_verified_layout(
    b: &Binding,
    complete: &Receipt,
    truth: &[MatchedRow],
    available: &[bool],
) -> Page {
    let mut receipt = complete.clone();
    for (i, block) in receipt.blocks.iter_mut().enumerate() {
        if !available[i] && matches!(block.disposition, Disposition::Scanned) {
            block.disposition = Disposition::Unavailable;
        }
    }
    let rows = truth
        .iter()
        .filter(|r| matches!(receipt.blocks[r.block].disposition, Disposition::Scanned))
        .cloned()
        .collect();
    Page {
        binding: b.clone(),
        receipt,
        rows,
    }
}
fn resolved_expected(
    truth: &[MatchedRow],
    complete: &Receipt,
    permanently_missing: Option<usize>,
) -> Vec<MatchedRow> {
    truth
        .iter()
        .filter(|r| {
            permanently_missing != Some(r.block)
                || matches!(
                    complete.blocks[r.block].disposition,
                    Disposition::Excluded(_)
                )
        })
        .cloned()
        .collect()
}
fn assert_acc(
    acc: &Accumulator,
    truth: &[MatchedRow],
    complete: &Receipt,
    permanent: Option<usize>,
) {
    let want = resolved_expected(truth, complete, permanent);
    assert_eq!(acc.rows(), want);
    assert_eq!(
        acc.positions(),
        want.iter()
            .map(|r| r.block * BLOCK + r.offset)
            .collect::<Vec<_>>()
    );
    let outstanding =
        permanent.filter(|&i| matches!(complete.blocks[i].disposition, Disposition::Scanned));
    let status = outstanding.map_or(CoverageStatus::Complete, |i| CoverageStatus::Incomplete {
        unavailable: vec![i],
    });
    assert_eq!(acc.status(), status);
    assert_eq!(
        acc.residual()
            .blocks
            .iter()
            .map(|c| c.ordinal)
            .collect::<Vec<_>>(),
        outstanding.into_iter().collect::<Vec<_>>()
    );
}
fn snapshot(rows: Vec<Event>, id: u64) -> SealedSnapshot {
    SealedSnapshot::new(rows, NonZeroUsize::new(BLOCK).unwrap(), id, None).unwrap()
}

#[test]
fn registered_e3_full_mask_query_product() {
    for shape in ["mixed", "shuffled_logs"] {
        for seed in [201_u64, 202, 203] {
            let (rows, base) = workload(shape, seed);
            let snap = snapshot(
                workload(shape, seed).0,
                seed + if shape == "mixed" { 0 } else { 1000 },
            );
            let qs = queries(base);
            let mut mask_rng = 7_u64;
            let masks: Vec<Vec<bool>> = (0..MASKS)
                .map(|_| (0..BLOCKS).map(|_| next(&mut mask_rng) & 1 == 1).collect())
                .collect();
            let mut cases = 0usize;
            let mut retries = 0usize;
            let mut permanent_cases = 0usize;
            let mut conflicts = 0usize;
            let mut successors = 0usize;
            let mut adapter_cases = 0usize;
            for (qi, q) in qs.iter().enumerate() {
                let truth = scalar(&rows, q);
                let b = binding(&snap, q.clone());
                let complete = snap.query(q, &[true; BLOCKS]).unwrap().receipt;
                assert_eq!(
                    coverage::verify(snap.anchor(), q, &complete),
                    Ok(CoverageStatus::Complete)
                );
                let all = page_from_verified_layout(&b, &complete, &truth, &[true; BLOCKS]);
                // The adapter must independently reproduce scalar coordinates and row digests.
                let actual = resume::execute(&snap, &b, &[true; BLOCKS]).unwrap();
                assert_eq!(actual.rows, truth);
                assert_eq!(actual.receipt, complete);
                adapter_cases += 1;
                if let Some(first) = truth.first() {
                    let mut bad = all.clone();
                    bad.rows
                        .iter_mut()
                        .find(|r| r.block == first.block && r.offset == first.offset)
                        .unwrap()
                        .digest[0] ^= 1;
                    let mut acc = Accumulator::new(b.clone(), all.clone()).unwrap();
                    let before = (acc.rows(), acc.status(), acc.residual());
                    assert!(acc.merge(bad).is_err());
                    assert_eq!((acc.rows(), acc.status(), acc.residual()), before);
                    conflicts += 1;
                }
                for (mi, mask) in masks.iter().enumerate() {
                    let permanent = if mi % 20 == 0 {
                        complete
                            .blocks
                            .iter()
                            .position(|x| matches!(x.disposition, Disposition::Scanned))
                    } else {
                        None
                    };
                    let mut first_mask = mask.clone();
                    if let Some(i) = permanent {
                        first_mask[i] = false;
                    }
                    let mut second_mask: Vec<bool> = first_mask.iter().map(|v| !*v).collect();
                    if let Some(i) = permanent {
                        second_mask[i] = false;
                        permanent_cases += 1;
                    }
                    let first = page_from_verified_layout(&b, &complete, &truth, &first_mask);
                    let second = page_from_verified_layout(&b, &complete, &truth, &second_mask);
                    let mut pages = [first.clone(), second.clone()];
                    if (mi + qi) % 2 == 1 {
                        pages.swap(0, 1);
                    }
                    let mut acc = Accumulator::new(b.clone(), pages[0].clone()).unwrap();
                    // 1..3 deterministic retries, with the order permuted across cases.
                    let retry_count = 1 + (mi + qi) % 3;
                    for ri in 0..retry_count {
                        acc.merge(pages[(ri + mi) % 2].clone()).unwrap();
                        retries += 1;
                    }
                    acc.merge(pages[1].clone()).unwrap();
                    assert_acc(&acc, &truth, &complete, permanent);
                    acc.merge(pages[0].clone()).unwrap();
                    assert_acc(&acc, &truth, &complete, permanent);
                    cases += 1;
                }
                // Position reuse: a predecessor residual cannot be replayed against any
                // of 50 compacted successors, even when ordinal positions coincide.
                if qi == 0 {
                    let incomplete =
                        page_from_verified_layout(&b, &complete, &truth, &[false; BLOCKS]);
                    let acc = Accumulator::new(b.clone(), incomplete).unwrap();
                    let residual = acc.residual();
                    for n in 0..SUCCESSORS {
                        let rotate = (n + 1) % ROWS;
                        let mut changed = workload(shape, seed).0;
                        changed.rotate_left(rotate);
                        let successor = snapshot(changed, 10_000 + seed * 100 + n as u64);
                        assert!(resume::resume(&successor, &residual, &[true; BLOCKS]).is_err());
                        successors += 1;
                    }
                }
            }
            assert_eq!(cases, QUERIES * MASKS);
            assert_eq!(adapter_cases, QUERIES);
            assert_eq!(successors, SUCCESSORS);
            assert_eq!(
                permanent_cases,
                qs.iter()
                    .filter(|q| {
                        let r = snap.query(q, &[true; BLOCKS]).unwrap().receipt;
                        r.blocks
                            .iter()
                            .any(|b| matches!(b.disposition, Disposition::Scanned))
                    })
                    .count()
                    * MASKS
                    / 20
            );
            assert!(conflicts > 0);
            println!(
                "E3_JSON {{\"shape\":\"{shape}\",\"seed\":{seed},\"rows\":{ROWS},\"blocks\":{BLOCKS},\"queries\":{QUERIES},\"masks_per_query\":{MASKS},\"cases\":{cases},\"retries\":{retries},\"permanent_cases\":{permanent_cases},\"conflicts\":{conflicts},\"successors\":{successors},\"adapter_cases\":{adapter_cases},\"contract_violations\":0}}"
            );
        }
    }
}
