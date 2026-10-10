//! Finite generated SEAL-2 checks. The oracle spells out the contract key
//! independently of `Row::key`; unique payloads make stable ties observable.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "sealer-merge-property-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

struct Generator(u64);

impl Generator {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn rows(&mut self, count: usize) -> Vec<LogRow> {
        (0..count)
            .map(|index| {
                // Small domains produce repeated complete keys and ties at each
                // component. Extremes exercise the full integer key domains.
                let choices = [0, 1, 2, u64::MAX];
                let mut node_id = [0; 16];
                node_id[0] = (self.next() % 3) as u8;
                node_id[15] = (self.next() % 3) as u8;
                let mut row = LogRow {
                    group: index as u64,
                    node: format!("label-{}", self.next() % 5),
                    node_id,
                    sequence: choices[(self.next() % 4) as usize],
                    index: [0, 1, u32::MAX][(self.next() % 3) as usize],
                    observed_ns: choices[(self.next() % 4) as usize],
                    body: format!(
                        "input-{index}: λ\0\"\\{}",
                        "x".repeat((self.next() % 257) as usize)
                    ),
                    attributes: BTreeMap::from([(
                        format!("key-{}\0λ", self.next() % 7),
                        format!("value-{index}\\\""),
                    )]),
                };
                // These distinguishable rows have exactly equal contract keys,
                // across many runs, while retaining their input position.
                if index % 3 == 0 {
                    row.observed_ns = 1;
                    row.node_id = [7; 16];
                    row.sequence = 2;
                    row.index = 1;
                }
                row
            })
            .collect()
    }
}

fn contract_key(row: &LogRow) -> (u64, [u8; 16], u64, u32) {
    (row.observed_ns, row.node_id, row.sequence, row.index)
}

fn check(rows: Vec<LogRow>, byte_limit: usize, seed: u64) {
    let scratch = Scratch::new();
    let mut expected = rows.clone();
    expected.sort_by_key(contract_key);
    let mut runs = Runs::new(&scratch.0, "generated-logs", byte_limit);
    for row in rows {
        runs.push(row).unwrap();
    }
    let mut actual = Vec::new();
    runs.finish(|row| {
        actual.push(row);
        Ok(())
    })
    .unwrap();
    assert_eq!(actual, expected, "seed={seed:#x}, byte_limit={byte_limit}");
    assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 0);
}

#[test]
fn generated_merge_matches_independent_stable_sort_across_run_sizes() {
    // Reproducible seeds and bounded populations; this is finite generated
    // evidence, not a claim over every possible row or byte limit.
    for case in 0..64_u64 {
        let seed = 0xA11F_A001 ^ (case + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut generator = Generator(seed);
        let count = (generator.next() % 200) as usize;
        let rows = generator.rows(count);
        for limit in [0, 1, 512, 4096, usize::MAX] {
            check(rows.clone(), limit, seed);
        }
    }
}

#[test]
fn generated_stable_ties_survive_full_and_partial_multipass_merges() {
    // One row per run forces actual paths on both sides of FAN_IN and
    // FAN_IN², including the partial rewrite/untouched-run rename path.
    for count in [0, 1, 15, 16, 17, 18, 31, 32, 255, 256, 257, 273, 513] {
        let seed = 0xA11F_A001 ^ count as u64;
        check(Generator(seed).rows(count), 1, seed);
    }
}

#[test]
fn stable_sort_oracle_rejects_changed_keys_payloads_and_tie_order() {
    let mut expected = Generator(0xA11F_A001).rows(40);
    expected.sort_by_key(contract_key);
    let mut wrong_key = expected.clone();
    wrong_key[0].observed_ns ^= 1;
    assert_ne!(wrong_key, expected);
    let mut wrong_payload = expected.clone();
    wrong_payload[0].body.push('!');
    assert_ne!(wrong_payload, expected);
    let pair = expected
        .windows(2)
        .position(|pair| contract_key(&pair[0]) == contract_key(&pair[1]))
        .expect("fixture includes distinct payloads with identical keys");
    let mut wrong_ties = expected.clone();
    wrong_ties.swap(pair, pair + 1);
    assert_ne!(wrong_ties, expected);
}
