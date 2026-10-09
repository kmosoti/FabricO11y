# Small storage peak-stage attribution diagnostic

Prospective supplement to the [storage scaling challenge](cross-system-storage-scaling-prediction-proposal.md)
and [storage sweep](cross-system-storage-sweep-proposal.md). The first completed
64 MiB cap8 steady holdout exceeds the frozen 16 MiB peak prediction while
retaining semantic gates. This diagnostic does not refit either model or change
its 5% rule. Its purpose is causal discrimination, without a speed nomination,
production phase-hook edit or default change. Results remain unmeasured here.

## Competing mechanisms

H1: the small-input model misses concurrent resident log/metric/span run buffers
during input ingestion. At 16 MiB, metrics have 14304 rows, below the independent
32768-row cap; the first 64 MiB steady case has 57120. Its sampled initial spill
counts change from at least (logs4, metrics1, spans1) to (logs13, metrics2,
spans1). No intermediate merge was sampled in that first cell. These observations
motivate inspection; they do not establish the exact allocation owner.

H0/alternatives: the larger peak arises during stable sorting/spill serialization,
batch/gap writer conversion, later table/filter writing, manifest/codec growth,
or an intermediate merge missed by filesystem sampling. Source explicitly spills
all three final resident runs before sorted table merging, so that stage should
not overlap still-resident sort buffers. Input extraction still interleaves three
independently capped resident runs with the Batches/Gaps writers. End-of-build
retained heap is unchanged in the first small/large comparison, which disfavors
a persistent leak as its explanation.

## Finite procedure and observation boundaries

The new example
[storage_attribution_probe.rs](../../../crates/fabric-server/examples/storage_attribution_probe.rs)
copies the existing readiness fixture, allocator and five ordered ledgers.
Its only additions install/take the existing phase observer, report its fixed
allocation and write a bounded phase trace **after** measured build and semantic
verification. Production source, original probe, oracle and compiler defaults
remain unchanged. Build two frozen copies with `phase-probe` and the existing
run selector 8 or 16 MiB, two Cargo jobs, release/offline/locked. Unset spill
workspace and borrowed-log experiment selectors.

Exactly four matched diagnostic pairs / eight native cells: encoded target
16/64 MiB × run cap8/16 MiB × plain/observed, steady shape, seed2703204353.
Both arms use the same feature-enabled binary; the plain arm never installs the
observer. Alternate arm order by target/cap. This isolates enabled capture from
an uninstalled observer, but does not remove compile-feature span-call overhead
from the plain control. Compare its incremental peak with the earlier frozen
unfeatured sweep only as an observer-perturbation observation.

The existing observer preallocates 262144 Records (tens of MiB). Record exact
LIVE increase across installation as `observer_live_bytes`, include that fixed
allocation in `heap_start_bytes`, and report whole peak alongside incremental
`peak_live_heap_bytes - heap_start_bytes`. Never subtract it from RSS or claim
physical-memory equivalence. Install after fixture generation, before resetting
the counted build peak. Take the trace immediately after build, before scanner
verification. Limit retained trace to 16384 Records; exceeding that envelope
rejects the diagnostic rather than silently dropping events.

Samples are `[0, LIVE, PEAK, 0]`: CPU/requested phase counters are unsupported.
Keep thread, nesting depth, start/end wall and before/after samples. A record
whose after-PEAK exceeds before-PEAK establishes an **enclosing interval** where
global high water increased. Nested intervals overlap, and uninstrumented
extraction/run pushes can raise high water between fine-grained records. Do not
sum nested wall times or label an enclosing interval an exclusive allocation
owner. Discrimination can remain inconclusive. Inspect live snapshots around
frame reads/spills versus table writes, sampled run-file presence, and the
small/large incremental peak changes to distinguish source-consistent regimes.

Reuse unchanged independent FTF1/PyArrow authentication and the existing
manifest, exact fixture, ordered-row/custody, applicable byte and leftover gates
between plain and observed arms. The same Rust ledgers remain a differential,
not an independent full-row oracle. Any semantic/control failure rejects this
diagnostic regardless of attribution. Preserve exact raw phase events, native
JSON, process costs, complete regular-file maps and source/protocol/binary hashes.

## Resource admission and command

The [runner](../../../tools/bench/labs/cross_system/storage_attribution.py) performs
two bounded builds, pinned ephemeral decoder installation and eight native cells
inside one 600-second job, with a 540-second inner deadline, 120 seconds per
build/native child and 45 seconds decoder installation. Scratch is entirely
owned data-drive state, at most 512 MiB; keep the registered 8 GiB aggregate
scratch and 16 GiB free reserve under unchanged 20 GiB/no-swap containment.
Retained diagnostic evidence is at most 32 MiB, including a 16 MiB failure
archive reserve. The outer coordinator independently enforces aggregate memory
category capacity. Builds/tools remain disposable only after their hashes and
configuration have been recorded.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/cross_system/run_sweep_job.py --id storage-attribution --lab memory --seconds 600 --reserve-mib 32 -- python3 -B tools/bench/labs/cross_system/storage_attribution.py --destination docs/experiments/benchmarks/data/cross-system-sweep-01/memory/attribution-01 --protocol docs/experiments/benchmarks/cross-system-storage-attribution-proposal.md
```

Passing pairs retain both native receipts and compressed phase-event bytes with
exact readback before fixture removal. Stop on first failure/deadline/overage;
archive all complete remaining owned state and verify every regular payload
before cleanup, within 16 MiB failure/32 MiB diagnostic evidence. Remove the
replaceable decoder first, never semantic fixtures. If preservation cannot fit,
record partial archive bytes/hash, retain complete remaining owned scratch and
stop. No silently discarded failure or new acceptance claim is included.
