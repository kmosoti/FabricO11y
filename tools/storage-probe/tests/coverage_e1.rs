//! Frozen E1R finite corpus. All expectations are computed from original rows by
//! this scalar scan; neither the candidate summary nor receipt supplies an oracle.
use fabric_o11y::generator::{EventGenerator, WorkloadConfig};
use fabric_o11y::{Event, Payload, TenantId};
use std::num::NonZeroUsize;
use storage_probe::Query;
use storage_probe::coverage::{self, CoverageStatus, Disposition, SealedSnapshot};

const ROWS: u32 = 2048;
const BLOCK: usize = 64;
const QUERIES: usize = 128;

// S1 shape and payload mutation, pinned here before either E1 implementation.
fn workload(name: &str, seed: u64) -> (Vec<Event>, i64) {
    let mut events: Vec<_> = EventGenerator::new(WorkloadConfig { seed, events: ROWS }).collect();
    let base = events[0].event_time.0;
    for (index, event) in events.iter_mut().enumerate() {
        if name == "clustered_logs" {
            event.tenant = TenantId(index as u64 / 256 + 1);
        } else if name == "shuffled_logs" || name == "mixed" {
            event.tenant = TenantId((index as u64 * 17 + seed) % 1024 + 1);
        }
        if name == "shuffled_logs" {
            event.event_time.0 =
                base + ((index as u64 * 109 + seed) % ROWS as u64) as i64 * 1_000_000;
        }
        if name == "clustered_logs"
            || name == "shuffled_logs"
            || (name == "mixed" && index % 2 == 0)
        {
            let term = if index % 257 == 0 { "rare" } else { "normal" };
            event.payload = Payload::Log {
                body: format!(
                    "common service{} request{index} {term} {}",
                    index % 16,
                    "x".repeat(128)
                ),
            };
        }
    }
    (events, base)
}

// SplitMix64, initialized with the registered seed 7 once per snapshot. Fixed
// eight-case cycle keeps all registered predicate shapes in every 128-query set.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut x = *state;
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}
fn queries(base: i64) -> Vec<Query> {
    let mut rng = 7_u64;
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
fn reference(rows: &[Event], q: &Query) -> Vec<usize> {
    let mut positions = Vec::new();
    for (pos, event) in rows.iter().enumerate() {
        if event.event_time.0 < q.start_ns || event.event_time.0 > q.end_ns {
            continue;
        }
        if q.tenant.is_some_and(|tenant| tenant != event.tenant.0) {
            continue;
        }
        if let Some(token) = &q.token {
            let Payload::Log { body } = &event.payload else {
                continue;
            };
            if !body.split_whitespace().any(|word| word == token) {
                continue;
            }
        }
        positions.push(pos);
    }
    positions
}
// This predicate uses independently traversed rows to derive an exact-set block
// summary, then applies the protocol's conservative exclusion rule.
fn may_match(rows: &[Event], q: &Query) -> bool {
    if q.start_ns > q.end_ns {
        return false;
    }
    let min = rows.iter().map(|r| r.event_time.0).min().unwrap();
    let max = rows.iter().map(|r| r.event_time.0).max().unwrap();
    if max < q.start_ns || min > q.end_ns {
        return false;
    }
    if let Some(t) = q.tenant {
        if !rows.iter().any(|r| r.tenant.0 == t) {
            return false;
        }
    }
    if let Some(token) = &q.token {
        if !rows.iter().any(|r| matches!(&r.payload, Payload::Log { body } if body.split_whitespace().any(|word| word == token))) { return false; }
    }
    true
}

#[test]
fn registered_e1_corpus_and_fault_matrix() {
    for name in ["gauge", "clustered_logs", "shuffled_logs", "mixed"] {
        for seed in [101_u64, 102, 103] {
            let (rows, base) = workload(name, seed);
            let corpus = queries(base);
            let expected: Vec<_> = corpus.iter().map(|q| reference(&rows, q)).collect();
            let candidate_ordinals: Vec<Vec<_>> = corpus
                .iter()
                .map(|q| {
                    rows.chunks(BLOCK)
                        .enumerate()
                        .filter_map(|(ord, block)| may_match(block, q).then_some(ord))
                        .collect()
                })
                .collect();
            let snapshot =
                SealedSnapshot::new(rows, NonZeroUsize::new(BLOCK).unwrap(), seed, None).unwrap();
            assert_eq!(snapshot.anchor().row_count, ROWS as usize);
            assert_eq!(snapshot.anchor().block_count, ROWS as usize / BLOCK);
            let mut clean = 0_usize;
            let mut unavailable = 0_usize;
            let mut faults = [0_usize; 6]; // omission, stale summary, order, root, snapshot, count
            for (index, q) in corpus.iter().enumerate() {
                let answer = snapshot.query(q, &[true; ROWS as usize / BLOCK]).unwrap();
                assert_eq!(
                    answer.positions, expected[index],
                    "{name} seed={seed} query={index}"
                );
                assert_eq!(
                    coverage::verify(snapshot.anchor(), q, &answer.receipt),
                    Ok(CoverageStatus::Complete)
                );
                clean += 1;

                let unavailable_answer =
                    snapshot.query(q, &[false; ROWS as usize / BLOCK]).unwrap();
                assert!(unavailable_answer.positions.is_empty());
                let expected_status = if candidate_ordinals[index].is_empty() {
                    CoverageStatus::Complete
                } else {
                    CoverageStatus::Incomplete {
                        unavailable: candidate_ordinals[index].clone(),
                    }
                };
                assert_eq!(
                    coverage::verify(snapshot.anchor(), q, &unavailable_answer.receipt),
                    Ok(expected_status),
                    "availability {name} seed={seed} query={index}"
                );
                unavailable += 1;

                let mut bad = answer.receipt.clone();
                bad.blocks.remove(0);
                assert!(coverage::verify(snapshot.anchor(), q, &bad).is_err());
                faults[0] += 1;
                if let Some(ord) = answer
                    .receipt
                    .blocks
                    .iter()
                    .position(|b| matches!(b.disposition, Disposition::Excluded(_)))
                {
                    let mut bad = answer.receipt.clone();
                    if let Disposition::Excluded(ref mut s) = bad.blocks[ord].disposition {
                        s.tokens.insert("tampered-summary".into());
                    }
                    assert!(coverage::verify(snapshot.anchor(), q, &bad).is_err());
                    faults[1] += 1;
                }
                let mut bad = answer.receipt.clone();
                bad.blocks.swap(0, 1);
                assert!(coverage::verify(snapshot.anchor(), q, &bad).is_err());
                faults[2] += 1;
                let mut bad_anchor = snapshot.anchor().clone();
                bad_anchor.root[0] ^= 1;
                assert!(coverage::verify(&bad_anchor, q, &answer.receipt).is_err());
                faults[3] += 1;
                let mut bad = answer.receipt.clone();
                bad.snapshot_id ^= 1;
                assert!(coverage::verify(snapshot.anchor(), q, &bad).is_err());
                faults[4] += 1;
                let mut bad_anchor = snapshot.anchor().clone();
                bad_anchor.block_count -= 1;
                assert!(coverage::verify(&bad_anchor, q, &answer.receipt).is_err());
                faults[5] += 1;
            }
            assert_eq!(clean, QUERIES);
            assert_eq!(unavailable, QUERIES);
            assert_eq!(
                [faults[0], faults[2], faults[3], faults[4], faults[5]],
                [QUERIES; 5]
            );
            // One compact, stable, machine-readable line per corpus.
            println!(
                "E1_JSON {{\"workload\":\"{name}\",\"seed\":{seed},\"rows\":{ROWS},\"block_size\":{BLOCK},\"queries\":{clean},\"availability\":{unavailable},\"fault_omission\":{},\"fault_stale_summary\":{},\"fault_order\":{},\"fault_root\":{},\"fault_snapshot\":{},\"fault_block_count\":{},\"contract_violations\":0}}",
                faults[0], faults[1], faults[2], faults[3], faults[4], faults[5]
            );
        }
    }
}
