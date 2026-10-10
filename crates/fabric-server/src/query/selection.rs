//! Bounded query selection. Heap entries carry keys, never row payloads.
use super::Key;
use std::collections::BinaryHeap;

/// Keeps the capacity smallest rows. Slots are reused only after eviction;
/// admission ordinals preserve the legacy stable order independently of slots.
/// Empty selectors allocate no slots. Geometric Vec growth can round allocated
/// capacity above the logical bound; the row-slot length never exceeds capacity.
pub(super) struct Smallest<T> {
    heap: BinaryHeap<(Key, usize, usize)>,
    rows: Vec<Option<T>>,
    next: usize,
    capacity: usize,
    weights: Vec<usize>,
    weight: Option<fn(&T) -> usize>,
    retained: usize,
    byte_cap: usize,
    exceeded: bool,
}

impl<T> Smallest<T> {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            heap: BinaryHeap::new(),
            rows: Vec::new(),
            next: 0,
            capacity,
            weights: Vec::new(),
            weight: None,
            retained: 0,
            byte_cap: usize::MAX,
            exceeded: false,
        }
    }
    pub(super) fn with_byte_budget(mut self, cap: usize, weight: fn(&T) -> usize) -> Self {
        self.byte_cap = cap;
        self.weight = Some(weight);
        self
    }
    pub(super) fn budget_exceeded(&self) -> bool {
        self.exceeded
    }
    pub(super) fn offer(&mut self, key: Key, row: T) {
        if self.exceeded || self.capacity == 0 {
            return;
        }
        if self.heap.len() == self.capacity
            && self.heap.peek().is_some_and(|(top, _, _)| key >= *top)
        {
            return;
        }
        let weight = self.weight.map_or(0, |f| f(&row));
        let removed = if self.heap.len() == self.capacity {
            self.heap
                .peek()
                .map_or(0, |(_, _, slot)| self.weights[*slot])
        } else {
            0
        };
        let Some(retained) = self
            .retained
            .checked_sub(removed)
            .and_then(|r| r.checked_add(weight))
            .filter(|r| *r <= self.byte_cap)
        else {
            self.exceeded = true;
            self.heap.clear();
            self.rows.clear();
            self.weights.clear();
            self.retained = 0;
            return;
        };
        self.retained = retained;
        let slot = if self.heap.len() == self.capacity {
            if self.heap.peek().is_some_and(|(top, _, _)| key >= *top) {
                return;
            }
            let (_, _, slot) = self.heap.pop().unwrap();
            self.rows[slot] = Some(row);
            self.weights[slot] = weight;
            slot
        } else {
            let slot = self.rows.len();
            self.rows.push(Some(row));
            self.weights.push(weight);
            slot
        };
        self.heap.push((key, self.next, slot));
        self.next += 1;
    }
    pub(super) fn threshold(&self) -> Option<Key> {
        if self.heap.len() == self.capacity {
            self.heap.peek().map(|(key, _, _)| *key)
        } else {
            None
        }
    }
    pub(super) fn sorted(self) -> Vec<(Key, T)> {
        let mut keyed = self.heap.into_vec();
        keyed.sort_unstable_by_key(|(key, admission, _)| (*key, *admission));
        let mut rows = self.rows;
        keyed
            .into_iter()
            .map(|(key, _, slot)| (key, rows[slot].take().unwrap()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rows::LogRow;
    use std::collections::BTreeMap;
    use std::hint::black_box;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Instant;

    #[test]
    fn byte_budget_releases_evictions_and_fails_closed_on_overflow() {
        let key = |n| (n, [0; 16], 1, 0);
        let mut selected = Smallest::new(2).with_byte_budget(10, Vec::<u8>::len);
        selected.offer(key(3), vec![0; 6]);
        selected.offer(key(2), vec![0; 4]);
        selected.offer(key(4), vec![0; 100]); // A rejected row is not retained.
        assert!(!selected.budget_exceeded());
        selected.offer(key(1), vec![0; 6]); // Replaces the six-byte row, not the four-byte one.
        assert_eq!(selected.retained, 10);
        selected.offer(key(0), vec![0; 7]);
        assert!(selected.budget_exceeded());
        assert!(selected.rows.is_empty());
        selected.offer(key(0), vec![0; 1]);
        assert!(
            selected.sorted().is_empty(),
            "pressure must not turn into a partial successful page"
        );
        let mut empty = Smallest::new(0).with_byte_budget(0, Vec::<u8>::len);
        empty.offer(key(1), vec![0; 1]);
        assert!(!empty.budget_exceeded());
        assert!(empty.sorted().is_empty());
    }

    // Frozen pre-extraction selector, query.rs Smallest. This is a differential
    // comparator, not the independent stable-sort specification below.
    struct Legacy<T> {
        heap: BinaryHeap<(Key, usize)>,
        rows: BTreeMap<usize, T>,
        next: usize,
        capacity: usize,
    }
    impl<T> Legacy<T> {
        fn new(capacity: usize) -> Self {
            Self {
                heap: BinaryHeap::new(),
                rows: BTreeMap::new(),
                next: 0,
                capacity,
            }
        }
        fn offer(&mut self, key: Key, row: T) {
            if self.heap.len() == self.capacity {
                if self.heap.peek().is_some_and(|(top, _)| key >= *top) {
                    return;
                }
                let (_, id) = self.heap.pop().unwrap();
                self.rows.remove(&id);
            }
            self.heap.push((key, self.next));
            self.rows.insert(self.next, row);
            self.next += 1;
        }
        fn threshold(&self) -> Option<Key> {
            if self.heap.len() == self.capacity {
                self.heap.peek().map(|(key, _)| *key)
            } else {
                None
            }
        }
        fn sorted(self) -> Vec<(Key, T)> {
            let mut keyed = self.heap.into_vec();
            keyed.sort();
            let mut rows = self.rows;
            keyed
                .into_iter()
                .map(|(key, id)| (key, rows.remove(&id).unwrap()))
                .collect()
        }
    }

    fn keys(order: &str, count: usize) -> Vec<Key> {
        let mut seed = 42_u64;
        (0..count)
            .map(|index| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                match order {
                    "ascending" => (index as u64, [0; 16], 1, 0),
                    "descending" => ((count - index) as u64, [0; 16], 1, 0),
                    "duplicates" => ((index % 7) as u64, [0; 16], 1, 0),
                    // Equal time, distinct tie-break fields, including exact-key
                    // duplicates: selection must not order only by time.
                    "equaltime" => (
                        7,
                        [(index % 3) as u8; 16],
                        (index % 11) as u64,
                        (index % 5) as u32,
                    ),
                    "mixed" => (
                        seed % 257,
                        [(seed % 3) as u8; 16],
                        seed % 17,
                        (seed % 13) as u32,
                    ),
                    _ => unreachable!(),
                }
            })
            .collect()
    }

    fn oracle<T: Clone>(input: &[(Key, T)], capacity: usize) -> Vec<(Key, T)> {
        let mut ordered = input.to_vec();
        ordered.sort_by_key(|(key, _)| *key);
        ordered.truncate(capacity);
        ordered
    }

    #[test]
    fn dense_matches_legacy_and_stable_sort_at_every_offer() {
        for order in [
            "ascending",
            "descending",
            "duplicates",
            "equaltime",
            "mixed",
        ] {
            let input: Vec<_> = keys(order, 4096)
                .into_iter()
                .enumerate()
                .map(|(id, key)| (key, (id, format!("payload-{id}"))))
                .collect();
            for capacity in [1, 21, 1001] {
                let empty = Smallest::<usize>::new(capacity);
                assert_eq!(empty.threshold(), None);
                assert_eq!(empty.rows.capacity(), 0);
                assert_eq!(empty.heap.capacity(), 0);
                assert!(empty.sorted().is_empty());
                let mut dense = Smallest::new(capacity);
                let mut old = Legacy::new(capacity);
                for (index, (key, row)) in input.iter().enumerate() {
                    dense.offer(*key, row.clone());
                    old.offer(*key, row.clone());
                    // Reference-only stable sorting avoids cloning every payload
                    // for each prefix. It shares no heap/slot implementation.
                    let mut selected: Vec<_> = input[..=index].iter().collect();
                    selected.sort_by_key(|(key, _)| *key);
                    let threshold = if index + 1 >= capacity {
                        Some(selected[capacity - 1].0)
                    } else {
                        None
                    };
                    assert_eq!(dense.threshold(), threshold, "{order}/{capacity}/{index}");
                    assert_eq!(old.threshold(), threshold);
                    assert!(dense.rows.len() <= capacity);
                    assert_eq!(
                        dense.heap.len(),
                        dense.rows.iter().filter(|r| r.is_some()).count()
                    );
                }
                let expected = oracle(&input, capacity);
                assert_eq!(dense.sorted(), expected);
                assert_eq!(old.sorted(), expected);
            }
        }
    }

    struct Tracked {
        id: usize,
        drops: Arc<Vec<AtomicUsize>>,
    }
    impl Drop for Tracked {
        fn drop(&mut self) {
            self.drops[self.id].fetch_add(1, Ordering::SeqCst);
        }
    }
    #[test]
    fn dense_releases_rejected_evicted_sorted_and_cancelled_rows_once() {
        for sorted in [false, true] {
            for capacity in [1, 21, 1001] {
                let input = keys("mixed", 4096);
                let drops = Arc::new(
                    (0..input.len())
                        .map(|_| AtomicUsize::new(0))
                        .collect::<Vec<_>>(),
                );
                let mut selector = Smallest::new(capacity);
                for (id, key) in input.iter().enumerate() {
                    selector.offer(
                        *key,
                        Tracked {
                            id,
                            drops: drops.clone(),
                        },
                    );
                }
                let held: Vec<_> = selector.rows.iter().flatten().map(|r| r.id).collect();
                for (id, count) in drops.iter().enumerate() {
                    assert_eq!(
                        count.load(Ordering::SeqCst),
                        usize::from(!held.contains(&id))
                    );
                }
                if sorted {
                    let output = selector.sorted();
                    assert_eq!(output.len(), capacity);
                    drop(output);
                } else {
                    drop(selector);
                }
                assert!(drops.iter().all(|count| count.load(Ordering::SeqCst) == 1));
            }
        }
    }

    #[test]
    fn independent_oracle_rejects_threshold_and_tie_mutations() {
        let input = vec![
            ((7, [0; 16], 1, 0), "first"),
            ((7, [0; 16], 1, 0), "second"),
            ((8, [0; 16], 1, 0), "third"),
        ];
        let expected = oracle(&input, 2);
        let check_rows = |actual: &[(Key, &str)]| actual == oracle(&input, 2).as_slice();
        assert!(check_rows(&expected));
        let mut altered = expected.clone();
        altered.swap(0, 1);
        assert!(
            !check_rows(&altered),
            "tie reversal must be rejected by the independent sort oracle"
        );
        let correct = expected.last().unwrap().0;
        let check_threshold =
            |actual: Option<Key>| actual == Some(oracle(&input, 2).last().unwrap().0);
        assert!(check_threshold(Some(correct)));
        let incorrect = (correct.0 + 1, correct.1, correct.2, correct.3);
        assert!(
            !check_threshold(Some(incorrect)),
            "changed threshold must be rejected by the independent sort oracle"
        );
        // <= instead of < would replace the first retained equal-key row.
        assert_ne!(oracle(&input, 1), vec![(input[1].0, input[1].1)]);
    }

    fn payloads(order: &str, size: usize) -> Vec<(Key, LogRow)> {
        keys(order, 4096)
            .into_iter()
            .enumerate()
            .map(|(id, key)| {
                let prefix = format!("{id:04}:");
                let body = prefix + &"R".repeat(size - 5);
                (
                    key,
                    LogRow {
                        group: 1,
                        node: "selection-fixture".into(),
                        node_id: key.1,
                        sequence: key.2,
                        index: key.3,
                        observed_ns: key.0,
                        body,
                        attributes: BTreeMap::from([("fixture".into(), format!("{id}"))]),
                    },
                )
            })
            .collect()
    }
    fn measured(input: &[(Key, LogRow)], capacity: usize, dense: bool) -> (Vec<u128>, Vec<usize>) {
        let expected = oracle(input, capacity);
        let mut times = Vec::new();
        let mut admissions = Vec::new();
        for _ in 0..5 {
            // Payload allocations/clones are deliberately outside Instant.
            let fixture = input.to_vec();
            let (elapsed, output, accepted) = if dense {
                let start = Instant::now();
                let mut selector = Smallest::new(capacity);
                for (key, row) in fixture {
                    selector.offer(black_box(key), black_box(row));
                }
                let accepted = selector.next;
                let output = black_box(selector.sorted());
                (start.elapsed().as_nanos(), output, accepted)
            } else {
                let start = Instant::now();
                let mut selector = Legacy::new(capacity);
                for (key, row) in fixture {
                    selector.offer(black_box(key), black_box(row));
                }
                let accepted = selector.next;
                let output = black_box(selector.sorted());
                (start.elapsed().as_nanos(), output, accepted)
            };
            assert_eq!(output, expected);
            times.push(elapsed);
            admissions.push(accepted);
        }
        (times, admissions)
    }
    #[test]
    #[ignore = "registered root-only bounded selection timing screen"]
    fn dense_selection_timing_probe() {
        for size in [16, 1024] {
            for order in ["ascending", "descending", "equaltime", "mixed"] {
                let input = payloads(order, size);
                for capacity in [21, 1001] {
                    for pair in 1..=3 {
                        for dense in if pair % 2 == 1 {
                            [false, true]
                        } else {
                            [true, false]
                        } {
                            let (times, admissions) = measured(&input, capacity, dense);
                            println!(
                                "{}",
                                serde_json::json!({"probe":"dense_selection","seed":42,"records":4096,
                                "body_bytes":size,"order":order,"capacity":capacity,"pair":pair,"dense":dense,
                                "offer_and_sorted_wall_ns":times,"accepted_offers":admissions,"loops":5,
                                "exact_rows_and_order":true,"fixture_clone_outside_timing":true,
                                "selector_construction_in_timing":true,
                                "scope":"selector kernel only; no HTTP, storage or allocator attribution"})
                            );
                        }
                    }
                }
            }
        }
    }
}
