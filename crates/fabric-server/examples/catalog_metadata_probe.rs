//! CR3 precursor: immutable Manifest ownership only; no production catalog/lease.
use fabric_server::segment::{FileEntry, Manifest};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    hint::black_box,
    sync::{Arc, Barrier, mpsc},
    time::Instant,
};

#[cfg(feature = "responsibility-alloc-probe")]
mod allocation {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    struct Counted;
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);
    static TOTAL: AtomicUsize = AtomicUsize::new(0);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn add(n: usize) {
        let live = LIVE.fetch_add(n, Relaxed) + n;
        PEAK.fetch_max(live, Relaxed);
        TOTAL.fetch_add(n, Relaxed);
        CALLS.fetch_add(1, Relaxed);
    }
    // Delegate pointer/layout contracts to System; observation allocates nothing.
    unsafe impl GlobalAlloc for Counted {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { System.alloc(layout) };
            if !p.is_null() {
                add(layout.size());
            }
            p
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { System.alloc_zeroed(layout) };
            if !p.is_null() {
                add(layout.size());
            }
            p
        }
        unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
            unsafe { System.dealloc(p, layout) };
            LIVE.fetch_sub(layout.size(), Relaxed);
        }
        unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            let q = unsafe { System.realloc(p, layout, size) };
            if !q.is_null() {
                if size >= layout.size() {
                    let live = LIVE.fetch_add(size - layout.size(), Relaxed) + size - layout.size();
                    PEAK.fetch_max(live, Relaxed);
                } else {
                    LIVE.fetch_sub(layout.size() - size, Relaxed);
                }
                TOTAL.fetch_add(size, Relaxed);
                CALLS.fetch_add(1, Relaxed);
            }
            q
        }
    }
    #[global_allocator]
    static ALLOCATOR: Counted = Counted;
    pub fn facts() -> [usize; 4] {
        [
            LIVE.load(Relaxed),
            PEAK.load(Relaxed),
            TOTAL.load(Relaxed),
            CALLS.load(Relaxed),
        ]
    }
    pub fn reset() -> [usize; 4] {
        PEAK.store(LIVE.load(Relaxed), Relaxed);
        TOTAL.store(0, Relaxed);
        CALLS.store(0, Relaxed);
        facts()
    }
}
#[cfg(not(feature = "responsibility-alloc-probe"))]
mod allocation {
    pub fn facts() -> [usize; 4] {
        [0; 4]
    }
    pub fn reset() -> [usize; 4] {
        [0; 4]
    }
}

const SEED: u64 = 42;
const SEGMENTS: usize = 64;
const NODES: usize = 20;
const OPERATIONS: usize = 4096;
const GENERATIONS: usize = 8;
type Catalog = BTreeMap<u64, Arc<Manifest>>;

fn fixture(generation: usize) -> Catalog {
    (0..SEGMENTS)
        .map(|i| {
            let label = (generation * SEGMENTS + i + 1) as u64;
            let first = (label - 1) * 512 + 1;
            let received = 1_600_000_000_000_000_000 + first;
            let files = [
                "batches.parquet",
                "logs.parquet",
                "metrics.parquet",
                "spans.parquet",
                "gaps.parquet",
                "text_filter.bin",
                "spans_filter.bin",
            ]
            .into_iter()
            .enumerate()
            .map(|(j, name)| {
                let sha256 = format!("{:x}", Sha256::digest(format!("{SEED}:{label}:{name}")));
                (
                    name.to_owned(),
                    FileEntry {
                        sha256,
                        bytes: 65_536 + j as u64 * 4096,
                        rows: if name.ends_with(".bin") {
                            0
                        } else {
                            65_536 / (j as u64 + 1)
                        },
                    },
                )
            })
            .collect();
            (
                label,
                Arc::new(Manifest {
                    version: 1,
                    journal_label: label,
                    first_group: first,
                    last_group: first + 511,
                    records: 65_536,
                    received_min_ns: received,
                    received_max_ns: received + 511,
                    freshness: (0..NODES)
                        .map(|n| (format!("node-{n:03}"), received + n as u64))
                        .collect(),
                    files,
                }),
            )
        })
        .collect()
}

fn bytes(m: &Manifest) -> Vec<u8> {
    serde_json::to_vec(m).unwrap()
}
fn fold_bytes(mut sum: u64, data: &[u8]) -> u64 {
    for b in data {
        sum = sum.wrapping_mul(31).wrapping_add(*b as u64);
    }
    sum
}
fn consume(m: &Manifest) -> u64 {
    let mut sum = m.version as u64
        ^ m.journal_label
        ^ m.first_group
        ^ m.last_group
        ^ m.records
        ^ m.received_min_ns
        ^ m.received_max_ns;
    for (node, time) in &m.freshness {
        sum = fold_bytes(sum ^ time, node.as_bytes());
    }
    for (name, entry) in &m.files {
        sum = fold_bytes(sum ^ entry.bytes ^ entry.rows, name.as_bytes());
        sum = fold_bytes(sum, entry.sha256.as_bytes());
    }
    black_box(sum)
}
fn acquire_and_consume(catalog: &Catalog, mode: &str) -> u64 {
    if mode == "clone" {
        let view: Vec<Manifest> = catalog.values().map(|m| m.as_ref().clone()).collect();
        black_box(view.iter().map(consume).fold(0u64, u64::wrapping_add))
    } else {
        let view: Vec<Arc<Manifest>> = catalog.values().cloned().collect();
        black_box(
            view.iter()
                .map(|m| consume(m))
                .fold(0u64, u64::wrapping_add),
        )
    }
}

fn controls(catalog: &Catalog) -> serde_json::Value {
    let owned: Vec<Manifest> = catalog.values().map(|m| m.as_ref().clone()).collect();
    let handles: Vec<Arc<Manifest>> = catalog.values().cloned().collect();
    for (left, right) in owned.iter().zip(&handles) {
        assert_eq!(bytes(left), bytes(right));
    }
    assert_eq!(
        acquire_and_consume(catalog, "clone"),
        acquire_and_consume(catalog, "arc")
    );
    let original = owned[0].clone();
    let mut rejected = Vec::new();
    for name in [
        "group_bound",
        "freshness",
        "file_digest",
        "file_rows",
        "missing_file",
    ] {
        let mut bad = original.clone();
        match name {
            "group_bound" => bad.last_group += 1,
            "freshness" => *bad.freshness.values_mut().next().unwrap() += 1,
            "file_digest" => bad.files.values_mut().next().unwrap().sha256.push('0'),
            "file_rows" => bad.files.values_mut().next().unwrap().rows += 1,
            "missing_file" => {
                bad.files.pop_first();
            }
            _ => unreachable!(),
        }
        assert_ne!(bytes(&original), bytes(&bad), "accepted {name}");
        rejected.push(name);
    }
    json!({"exact_manifest_equality": true, "rejected_mutations": rejected})
}

fn lifecycle() -> serde_json::Value {
    let before = allocation::reset();
    let mut current = fixture(0);
    let snapshot_floor = current.values().next().unwrap().first_group;
    let held: Vec<Arc<Manifest>> = current.values().cloned().collect();
    let weak: Vec<_> = held.iter().map(Arc::downgrade).collect();
    let mut paused = vec![held];
    let mut logical_payload_bytes = vec![paused[0].iter().map(|m| bytes(m).len()).sum::<usize>()];
    for generation in 1..GENERATIONS {
        current = fixture(generation);
        paused.push(current.values().cloned().collect());
        logical_payload_bytes.push(paused.last().unwrap().iter().map(|m| bytes(m).len()).sum());
    }
    let retained_labels: BTreeSet<_> = current.keys().copied().collect();
    let retained_floor = current.values().next().unwrap().first_group;
    let expired = !fabric_core::query::page_snapshot_retained(snapshot_floor, retained_floor);
    assert!(expired);
    assert!(!retained_labels.contains(&paused[0][0].journal_label));
    assert!(weak.iter().all(|w| w.upgrade().is_some()));
    // Negative control: handle existence cannot serve as snapshot authorization.
    let stale_handle_would_authorize = weak[0].upgrade().is_some();
    assert!(stale_handle_would_authorize && expired);
    let during = allocation::facts();
    let all_weak: Vec<_> = paused.iter().flatten().map(Arc::downgrade).collect();
    drop(current);
    assert!(all_weak.iter().all(|w| w.upgrade().is_some()));
    // Cancellation is modeled by dropping all paused-reader views, not a task API.
    drop(paused);
    assert!(all_weak.iter().all(|w| w.upgrade().is_none()));
    drop(all_weak);
    drop(weak);
    drop(retained_labels);
    let after = allocation::facts();
    json!({"generations": GENERATIONS, "paused_handles": GENERATIONS * SEGMENTS,
        "logical_serialized_bytes_by_generation": logical_payload_bytes,
        "logical_expiration_control": expired, "stale_handle_authorization_rejected": true,
        "all_manifests_released_after_cancel": true, "before": before, "during": during,
        "after": after, "interpretation": "logical model; Arc liveness is not a storage lease"})
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 3, "usage: catalog_metadata_probe clone|arc 1|4");
    let mode = &args[1];
    assert!(matches!(mode.as_str(), "clone" | "arc"));
    let readers: usize = args[2].parse().unwrap();
    assert!(matches!(readers, 1 | 4));
    let catalog = fixture(0);
    let control = controls(&catalog);
    let expected = catalog
        .values()
        .map(|m| consume(m))
        .fold(0u64, u64::wrapping_add)
        .wrapping_mul(OPERATIONS as u64);
    let fixture_bytes: Vec<_> = catalog.values().flat_map(|m| bytes(m)).collect();
    let fixture_hash = format!("{:x}", Sha256::digest(&fixture_bytes));
    let logical_serialized_bytes = fixture_bytes.len();
    drop(fixture_bytes);
    let lifecycle_control = lifecycle();
    let ready = Arc::new(Barrier::new(readers + 1));
    let start = Arc::new(Barrier::new(readers + 1));
    let finished = Arc::new(Barrier::new(readers + 1));
    let (send, receive) = mpsc::channel();
    let (wall_ns, before, after, checksum) = std::thread::scope(|scope| {
        for _ in 0..readers {
            let (ready, start, finished, send) =
                (ready.clone(), start.clone(), finished.clone(), send.clone());
            let catalog = &catalog;
            scope.spawn(move || {
                ready.wait();
                start.wait();
                let mut sum = 0u64;
                for _ in 0..OPERATIONS / readers {
                    sum = sum.wrapping_add(acquire_and_consume(catalog, mode));
                }
                send.send(sum).unwrap();
                finished.wait();
            });
        }
        ready.wait();
        let before = allocation::reset();
        let began = Instant::now();
        start.wait();
        let checksum = (0..readers)
            .map(|_| receive.recv().unwrap())
            .fold(0u64, u64::wrapping_add);
        let wall_ns = began.elapsed().as_nanos();
        let after = allocation::facts();
        finished.wait();
        (wall_ns, before, after, checksum)
    });
    assert_eq!(checksum, expected, "lost/mutated operations");
    println!(
        "{}",
        json!({"stage": "complete", "mode": mode, "readers": readers,
        "seed": SEED, "segments": SEGMENTS, "nodes_per_manifest": NODES,
        "total_operations": OPERATIONS, "manifest_acquisitions": OPERATIONS * SEGMENTS,
        "allocator_counted": cfg!(feature = "responsibility-alloc-probe"),
        "fixture_sha256": fixture_hash, "logical_serialized_fixture_bytes": logical_serialized_bytes,
        "wall_ns": wall_ns, "checksum": checksum, "allocation_before": before,
        "allocation_after": after, "controls": control, "lifecycle": lifecycle_control,
        "allocation_fields": ["requested_live_bytes", "requested_peak_bytes", "cumulative_requested_bytes", "allocation_calls"],
        "limits": {"scratch_bytes": 0, "operations": OPERATIONS, "generations": GENERATIONS},
        "interpretation": "fixed-work ownership microbenchmark; no storage IO, catalog adoption or lease guarantee; counted wall time diagnostic only"})
    );
}
