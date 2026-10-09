# RowSet representation screen with pinned Roaring

Prospective finite supplement to the [matched workload protocol](cross-system-sweep-protocol.md).
This is standalone data-structure research, with no Fabric workspace dependency,
production representation, storage format or query semantic change. No outcome
is claimed before execution.

## Sources and mechanism

Compile actual roaring-rs source at
`0439d576fb3d36acba4444f5a5bc0d4173ecd932`, the revision already inventoried in
the prior continuation. Its immutable codeload archive has historical SHA-256
`7239e8bec111af1f49bcc736628966622a7bb5103a9bfb71a45714f47aa48e36` and
145182 compressed bytes. The pinned [package manifest](https://github.com/RoaringBitmap/roaring-rs/blob/0439d576fb3d36acba4444f5a5bc0d4173ecd932/roaring/Cargo.toml)
declares version 0.11.5, Rust 1.90 and the std feature; its
[workspace manifest](https://github.com/RoaringBitmap/roaring-rs/blob/0439d576fb3d36acba4444f5a5bc0d4173ecd932/Cargo.toml)
declares bytemuck 1.25.2 and byteorder 1.5.0. Serde and derive/simd features
are not enabled. No upstream build scripts, tests or benchmarks run.

Direct rustc rlibs compile these unchanged source trees without introducing a
Cargo workspace or resolving optional/development dependencies. Bytemuck has
no enabled features; byteorder and Roaring receive `feature="std"`. Upstream
package checksums from the official
[bytemuck registry index](https://github.com/rust-lang/crates.io-index/blob/master/by/te/bytemuck)
and [byteorder registry index](https://github.com/rust-lang/crates.io-index/blob/master/by/te/byteorder)
are respectively `95832e849adfb21180ccb6826a99da14e5d266ae5c2e668e1602cf234f153797`
and `1fd0f2584146f6f2ef48085050886acf353beff7305ebd1ae69500e27c67f64b`.
The URLs select exact versions; recorded checksums provide immutable verification.

The Roaring construction path uses `from_sorted_iter` then the upstream
`optimize()` call; its construction timing and allocator costs include
optimization. The pinned implementation selects array/bitmap/run storage and
serializes those containers in the standard Roaring format. Sorted vectors use
copied sorted inputs and two-pointer AND/OR with a growing result Vec. Dense
sets use an explicitly universe-sized `Vec<u64>`, bitwise word operations and
descending bit enumeration. These are concrete baseline implementations,
not proofs of optimal implementations for each representation.

H1: container adaptation improves some sparse/clustered operation or memory
costs after accounting for construction and conversion. H0: no useful benefit,
construction dominates a one-query lifetime, or dense/common cases erase it.
Counterexamples include sparse intersections shorter than 64, clustered runs,
randomly spread membership, and approximately 75% membership density. No
representation is assumed to win across these shapes.

## Registered finite grid

Exactly 48 fresh native cells = two universes × four shapes × two seeds × three
representations. Each cell constructs two sets and computes AND, OR and the
largest 64 intersection IDs in descending order. Universe sizes are 32768 and
262144. Seeds are 2703204353 and 2703204354. Representations are sorted Vec,
dense bitset and pinned Roaring. Rotate representation order by shape index
plus repeat. Universe is an index domain, not a Fabric payload-byte claim.

Python defines and retains the exact sorted input IDs. Sparse and spread select
approximately 1/1024 and 1/16 of the universe through a fixed 32-bit mixer;
common selects approximately 3/4. Operand B shares half the hash buckets with
A and samples the other half independently at the same density, giving both
overlap and misses. Clustered membership takes 512-ID runs in every 4096-ID
window, shifted by seed and 97 IDs between operands. Record actual cardinalities
and input bytes; these are finite deterministic generators, not uniformity claims.

Each operation has one-shot timing and a separate batch of 128 operations with
black-boxed inputs/results, including result allocation and destruction.
Top64 operates on the already constructed intersection; do not call its timing
a fused AND/top query. Measure construction, AND, OR, top64, serialization and
full-result conversion separately. Requested allocation, incremental peak live
and retained live bytes are System allocator observations for each scope, not
resident/physical memory or declared reservations. Repeated operation batches
reuse constructed inputs; they do not replace the two fresh seeded constructions.

Native whole-process CPU/wall is recorded through wait4. Read native
`/proc/self/status` VmHWM inside the executable: Python oracle residency can
dominate inherited pre-exec wait4 RSS for these small workers, so wait4 RSS is
retained but not interpreted as native-only memory. Native VmHWM includes
fixture decoding, conversion, serialization and correctness export. Report
those boundaries and construction amortization before claiming an operation win.

Native serialization sizes are actual input-pair artifacts: u32 little-endian
for vectors, u64 little-endian universe words for dense bitsets, and upstream
Roaring serialization. Serialization time and requested allocation are recorded;
round trips use the corresponding native implementation. Independent Python
verifies complete ordered canonical AND/OR membership and descending top64
bytes against set algebra, rather than trusting native hashes or cardinality.
Missing, extra and reordered result controls must fail before measurements.
This does not claim independent proof of the Roaring wire-format decoder.

## Execution, provenance and preservation

The [runner](../../../tools/bench/labs/cross_system/rowset_sweep.py) downloads,
authenticates and safely extracts only regular source files into wholly owned
data-drive scratch; links, traversal, duplicate/noncanonical paths and mixed
archive roots reject. Record full member SHA-256/size inventories and license
paths. Each source package is at most 2 MiB compressed and 16 MiB decoded,
with 45 seconds network time. Retain complete compressed source archives and
per-package receipts. A checksum mismatch stops without an inexact substitute.

Compile [rowset_probe.rs](../../../tools/bench/labs/cross_system/rowset_probe.rs)
with explicit rustc edition/optimization/features/extern arguments. Record
rustc version, command/exit, native binary and probe/runner/protocol SHA-256.
The whole job receives at most 600 seconds, with a 540-second inner deadline,
four 90-second compile ceilings and a 20-second native-child ceiling. Apply
the unchanged 20 GiB/no-swap resource group, 512 MiB total scratch, 16 GiB free
reserve and the round's 8 GiB active scratch envelope. No project or upstream
work runs outside that launcher.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/cross_system/run_sweep_job.py --id rowset-01 --lab query --seconds 600 --reserve-mib 64 -- python3 -B tools/bench/labs/cross_system/rowset_sweep.py --destination docs/experiments/benchmarks/data/cross-system-sweep-01/query/rowset-01 --protocol docs/experiments/benchmarks/cross-system-rowset-sweep-proposal.md
```

Goal: at most 32 MiB evidence; hard cap 64 MiB inside the existing query category.
Keep exact input vectors, raw metrics/process receipts and compressed exact native
outputs with member maps. Read every archived payload back before deleting its
source output. On success record the complete scratch source/build member map,
then remove all owned extracted sources, rlibs, binaries and temporary outputs.
On failure preserve complete remaining scratch in a readback-verified archive
within 32 MiB and the 64 MiB evidence cap; if it cannot fit, retain complete
scratch and record any partial archive's exact bytes/hash, then stop. Historical
failed state is never deleted to admit another cell.

Any membership/control/round-trip error rejects the screen. Report all null and
adverse regimes, both seeds and build-plus-operation cost. A shape-specific
research vector needs a repeatable operation or memory advantage in both seeds,
an explicit construction/serialization tradeoff and a source-based causal
explanation. There is no production nomination threshold in this exploratory
screen: a separately registered end-to-end Fabric query comparison and fresh
confirmation are required before considering a representation change.
