// Additional integrator probes after candidate source inspection.
// Dataset/query helpers copied from the independently frozen E3R oracle.
use fabric_o11y::generator::{EventGenerator, WorkloadConfig};
use fabric_o11y::{Event, Payload, TenantId};
use std::num::NonZeroUsize;
use storage_probe::Query;
use storage_probe::coverage::{Disposition, SealedSnapshot};
use storage_probe::resume::{self, Accumulator, Binding, MatchedRow};

const ROWS: usize = 2048;
const BLOCK: usize = 64;
const BLOCKS: usize = ROWS / BLOCK;
const QUERIES: usize = 128;

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
fn binding(s: &SealedSnapshot, query: Query) -> Binding {
    Binding {
        anchor: s.anchor().clone(),
        query,
        tokenizer_version: resume::TOKENIZER_VERSION,
        order_version: resume::ORDER_VERSION,
    }
}

#[test]
fn each_registered_binding_rejects_faults_atomically() {
    let mut cases = 0;
    for shape in ["mixed", "shuffled_logs"] {
        for seed in [201, 202, 203] {
            let (rows, base) = workload(shape, seed);
            let snap =
                SealedSnapshot::new(rows, NonZeroUsize::new(BLOCK).unwrap(), seed, None).unwrap();
            for q in queries(base) {
                let b = binding(&snap, q);
                let p = resume::execute(&snap, &b, &[true; BLOCKS]).unwrap();
                let mut acc = Accumulator::new(b, p.clone()).unwrap();
                let before = (acc.rows(), acc.status(), acc.residual());
                let mut faults = Vec::new();
                let mut v = p.clone();
                v.binding.query.token = Some("changed-predicate".into());
                v.receipt.query = v.binding.query.clone();
                faults.push(v);
                let mut v = p.clone();
                v.binding.tokenizer_version += 1;
                faults.push(v);
                let mut v = p.clone();
                v.binding.order_version += 1;
                faults.push(v);
                let mut v = p.clone();
                v.receipt.blocks[0].commitment.rows_digest[0] ^= 1;
                faults.push(v);
                let mut v = p.clone();
                if let Some(row) = v.rows.first_mut() {
                    row.digest[0] ^= 1;
                } else {
                    // Even an empty query has a resolved empty block result. An
                    // otherwise well-formed added row must conflict with it.
                    v.receipt.blocks[0].disposition = Disposition::Scanned;
                    v.rows.push(MatchedRow {
                        block: 0,
                        offset: 0,
                        digest: snap.row_digest_at(0).unwrap(),
                    });
                }
                faults.push(v);
                for v in faults {
                    assert!(acc.merge(v).is_err());
                    assert_eq!((acc.rows(), acc.status(), acc.residual()), before);
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 3840);
}

#[test]
fn genuinely_different_snapshot_with_identical_layout_is_rejected() {
    let a = SealedSnapshot::new(
        workload("mixed", 201).0,
        NonZeroUsize::new(BLOCK).unwrap(),
        1,
        None,
    )
    .unwrap();
    let b = SealedSnapshot::new(
        workload("mixed", 201).0,
        NonZeroUsize::new(BLOCK).unwrap(),
        2,
        None,
    )
    .unwrap();
    let q = queries(0).remove(0);
    let first = resume::execute(&a, &binding(&a, q.clone()), &[true; BLOCKS]).unwrap();
    let second = resume::execute(&b, &binding(&b, q), &[true; BLOCKS]).unwrap();
    assert_eq!(first.receipt.blocks, second.receipt.blocks);
    let mut acc = Accumulator::new(first.binding.clone(), first).unwrap();
    let before = (acc.rows(), acc.status(), acc.residual());
    assert!(acc.merge(second).is_err());
    assert_eq!((acc.rows(), acc.status(), acc.residual()), before);
}

#[test]
fn conflict_after_new_work_does_not_partially_update_state() {
    let snap = SealedSnapshot::new(
        workload("mixed", 201).0,
        NonZeroUsize::new(BLOCK).unwrap(),
        1,
        None,
    )
    .unwrap();
    let b = binding(&snap, queries(0).remove(0));
    let mut mask = [true; BLOCKS];
    mask[0] = false;
    let initial = resume::execute(&snap, &b, &mask).unwrap();
    let mut acc = Accumulator::new(b.clone(), initial).unwrap();
    let before = (acc.rows(), acc.status(), acc.residual());
    let mut later = resume::execute(&snap, &b, &[true; BLOCKS]).unwrap();
    later.rows.last_mut().unwrap().digest[0] ^= 1;
    assert!(acc.merge(later).is_err());
    assert_eq!((acc.rows(), acc.status(), acc.residual()), before);
}
