# CR3 precursor: Manifest copies versus immutable handles

Status: **prepared; not built or run**. This is an isolated fixed-work Rust
ownership microbenchmark, not a catalog implementation or adoption decision.
[Catalog query packet](catalog-labs-query.md), [Q3](query-frontier-protocol.md)
and [frontier controls](performance-frontier-lab-plan.md) supply the larger scope.
Production query/defaults, crate graph, oracles, bytes and durability stay fixed.

## Mechanism, hypotheses and frozen fixture

Current `WalkState::segments` clones cached `segment::Manifest` values into a
query view. Those real types contain freshness and file-entry `BTreeMap`s with
owned strings. The [isolated example](../../../crates/fabric-server/examples/catalog_metadata_probe.rs)
compares deep Manifest clones with `Arc::clone` handles over identical immutable
content. Both baseline and candidate use the same backing
`BTreeMap<u64, Arc<Manifest>>`, so the baseline has an Arc-backed fixture too;
the changed operation is copying its Manifest versus acquiring its handle.
No mutex, directory listing, IO, query filtering or real Segment lease is modeled.

H1: handles reduce acquisition allocations and wall cost when the same metadata
is read, while paused views retain old-generation memory until released.
H0: consumed metadata/coordination dominates or reference-count contention erases
the benefit; retained generations impose unacceptable costs. A faster 4-reader
result must not be confused with less total work.

Freeze seed 42; 64 manifests; 20 node freshness entries each; seven files per
manifest (`batches/logs/metrics/spans/gaps.parquet`, `text_filter.bin`,
`spans_filter.bin`). File digests are SHA256 of `42:label:filename`; sizes and
counts use the exact deterministic formula in the frozen source. Groups cover
512 contiguous group IDs per label; fixed receive epoch is
1600000000000000000 ns. These are generated typed metadata, not measured disk
contents. Preserve source/hash receipts rather than claim real file verification.

Each trial performs 4096 total full-map acquisitions (262144 manifests), split
equally between 1 or 4 readers. Every path consumes all scalar fields, map keys,
timestamps, file digests and counts; both produce the same wrapping checksum.
Each acquisition releases its view before the next. Threads rendezvous before
the counter reset and timing; wall timing includes start barrier and result
handoff, excludes setup, thread creation/join and lifecycle controls. No warmup
or population changes after seeing timings. Process/OS scheduling remains a limit.

## Exactness, expiration and cancellation controls

Before timing, compare exact serialized real Manifest values from clone and
handle views. Inject changed group bound, freshness, digest, file rows and a
missing file; each must differ from the original exact bytes. This is structural
metadata equality, not the independent telemetry/query oracle.

Hold views across eight generations (512 handles). Publish each generated
generation in a local model and hold old views as paused readers. After the
final generation, the old snapshot floor fails the existing pure
`page_snapshot_retained` predicate despite live old Arc handles. Prove the stale
handle alone would authorize access incorrectly. Dropping current state must
leave paused handles alive; dropping all paused views models cancellation, and
every tracked Weak handle must then fail to upgrade. No filesystem lease,
concurrent publication protocol or asynchronous task cancellation is established.

The [driver](../../../tools/bench/labs/catalog/metadata.py) checks fixed work,
fixture hash/checksum, exact/mutation controls and expiration/drop accounting in
every trial. Its own injected wrong-mode, missing-operation/mutation,
stale-snapshot and leaked-generation receipts must be rejected. Do not normalize
a failure into success or substitute plan agreement for these controls.

## Matrix, observations and decision

Run plain first, then counted; each variant has 1/4 readers and three fresh
alternating pairs: clone/arc, arc/clone, clone/arc. Total: 24 subprocess trials.
Copy and hash each frozen binary before building the other variant. Plain lacks
`responsibility-alloc-probe`; counted enables it. Plain wall time is primary;
counted wall time is diagnostic because allocator atomics perturb contention.

Retain every exact JSON output and stderr, command/exit, source/protocol/harness
and binary hashes, CPU affinity/build environment and cleanup receipts. Compare
Arc/clone wall ratios separately for each reader count in each plain pair. H1
timing support requires at least 10% lower wall cost in all three pairs; a
regression or mixed result remains reported. No percentile/envelope claim follows
from three pairs or these synthetic maps.

Counted snapshots report global requested live/peak bytes, cumulative requested
allocation bytes and allocation-call counts; include coordination overhead.
Subtract trial baseline live bytes only for incremental peak. These are allocator
requests, not RSS or OS memory. Lifecycle snapshots are separate and include
tracking structures; serialized bytes by generation are logical content size,
not physical heap size. Weak-handle expiry establishes object release; remaining
live tracking/output allocations do not imply leaked Manifest objects. Report
the retention trade-off alongside acquisition savings, not as a lease guarantee.

## Executable admission and limits

Root must register this new protocol separately before measurement. No new
verification policy/oracle is changed. The existing example feature is reused;
no production refactor or new crate is needed. Root serializes the following
command through the enforcing launcher, preserving its resource receipt:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-metadata-01 --lab query --stage query --seconds 1500 -- python3 -B tools/bench/labs/catalog/launch.py metadata --out docs/experiments/benchmarks/data/catalog-metadata-run-01
```

The shared launcher builds, freezes/copies plain and counted binaries, invokes
`metadata.py --plain FROZEN_PLAIN --counted FROZEN_COUNTED --out EVIDENCE`, and
archives compressed frozen binaries. Root owns that launcher and its accounting.
The protocol/source snapshot must be frozen before timing; no retrospective
registration. The standalone driver also supports building with
`cargo build --offline --locked --release -p fabric-server
--example catalog_metadata_probe`, adding `--features responsibility-alloc-probe`
only for counted. Frozen native commands are `PLAIN_OR_COUNTED_BINARY clone 1`,
`arc 1`, `clone 4`, `arc 4`, in the above pair order. Those placeholders identify
the driver's hashed data-drive binary copies, not invented native CLI flags.

Shared-launcher builds have 600-second timeouts and its driver has 900 seconds;
the standalone driver's optional builds have 240 seconds. Each native trial has
60 seconds. The enclosing coordinator stops the whole job at 1500 seconds.
The driver stops admission after 24 elapsed minutes, allowing at most one final
60-second trial plus cleanup; the outer default 30-minute deadline remains.
All descendants use existing 16/20 GiB high/max and zero swap containment.
Probe-owned scratch and raw evidence each cap at 64 MiB, with 16 GiB mounted
drive free reserve. These exclude the shared launcher's frozen binary staging
and later compressed binary archives; the whole job retains the outer 8 GiB
scratch and 256 MiB evidence bounds. Shared data-drive build caches are outside
experiment scratch accounting. Local driver binary copies count toward its cap.
Driver output must be fresh under `data/catalog-metadata-run-01` or the original
`data/catalog-labs-run-01/query` root. The coordinator's 1500-second limit includes
shared build/freeze work and takes precedence over the driver's local allowance.
Failures/timeouts kill the owned process group and retain scratch; only fully
completed runs remove owned scratch. Preserve failures before later cleanup.

Completion means the microbenchmark matrix/controls finished, not performance
acceptance. CQ1 facade/lifecycle equivalence, CQ2 eager/lazy maintenance and Q3
source/output attribution remain pending. Real corruption/retention claims also
depend on resolving the recorded operations oracle/fixture mismatch separately.
