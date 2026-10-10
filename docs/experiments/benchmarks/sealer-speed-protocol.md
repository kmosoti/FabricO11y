# Bounded sealer speed investigation

Registered 2026-10-04 before new measurements. The owner asks whether the
observed 97% heap reduction can coexist with near-instant builds, and requests
exploration from multiple representations. This is a finite local optimization
investigation, not qualification or a change to the bounded-sealer acceptance.

## Invariants and candidate space

Preserve raw Batch bytes, exact row content/order, stable ties, schemas, filters,
row-group boundaries, durable publication and checkpoint/reclaim ordering.
Keep run/chunk/fan-in bounds, current worker defaults and the independent query
oracle. Private disposable spill representation can change; journal, Segment
and wire formats cannot. No new dependency, CPU pool or installed setting.

| Representation | Candidate | Decisive question / limitation |
| --- | --- | --- |
| CPU work per byte | Replace JSON scratch encoding with a compact private codec; avoid repeated parsing in intermediate merges | How much time belongs to scratch serialization and merge rather than final compression? |
| Data movement | Merge keys and opaque payloads; avoid decoding and re-encoding intermediate runs | Extra indirection/random reads must not replace cheap sequential IO with costly seeks |
| Ordering | Exploit already sorted inputs or non-overlapping runs | Shuffled times and repeated keys must remain exact; no weaker global order or query pruning |
| Pipeline timing | Prepare bounded runs while an active journal fills; publish after rotation | Can shorten post-rotation latency but moves CPU earlier; needs separate recovery/scheduling design and does not remove total work |
| Parallel work | Overlap independent tables/stages | Adds live buffers and CPU contention; reject without evidence that the same memory budget holds |
| Information/output floor | Account for required read, encode, hash, compress, write and sync | Materializing hundreds of MiB has a nonzero cost; fast publication and fast complete rebuild are distinct metrics |

Inspect native phase costs first. Add opt-in coarse spans for spill sort/write,
intermediate merge, table/filter processing and finalization. Attribute exclusive
wall/CPU by subtracting immediate children on the same thread; do not sum nested
inclusive spans. Record probe-enabled and probe-disabled samples and ledger size;
profiling numbers guide selection, final timing comparisons disable observers.

## Finite measurement plan

Reuse ingestion-memory run 01's fixed seed 42, shuffled timestamps, one node,
empty attributes, 128 logs per Batch, 1,024-byte bodies. Screen 65,536 and 262,144
logs (actual encoded journal sizes recorded). First collect one profile at each
size plus an observer-off control at 65,536. Freeze baseline and each candidate
binary with SHA-256 and actual source snapshots before comparisons.

For the selected local mechanism: three alternating baseline/candidate pairs at
each size in fresh processes, one builder, sequential workloads. Report wall and
CPU time, incremental live Rust heap, retained-fixture baseline, process RSS,
logical IO counters, output hashes, exact rows/raw custody and cleanup. Reuse
the existing independent first-page query grader for a three-pair pending-query
screen if a candidate is retained. Fixture/grade time stays outside build timing.

Success means exact output hashes against the bounded baseline, at least 10%
lower median build wall time, no more than 10% growth above the bounded baseline's
heap at each size, and largest-fixture heap at most 3% of run 01's whole-file
reference (1,242,865,595 bytes). All samples/failures stay in the record. A smaller
gain is reported as such; do not rename it near-instant. There is no universal
latency claim. Report residual stage costs and what a subsecond target would
require after the measurement. An additional candidate requires an addendum
before its measurements, without changing these success rules.

Correctness includes the existing mixed-signal byte differential, float-bit
preservation, multi-pass stable ties, large-row byte admission and cleanup after
corruption. A new private codec must reject malformed lengths, invalid data and
truncation, with deterministic negative controls. Run required fast checks after
the final implementation. Four-shape acceptance, full faults and soak remain
separate unfinished work.

All builds, tests, profiling, comparisons and validators use the existing
20 GiB cgroup launcher, no swap, mounted data-drive build/scratch paths, and
30-minute per-command deadline. Archive small evidence and failure observations;
delete owned trial scratch and temporary frozen binaries once complete. Keep
source fixtures as identified evidence. Do not run remote workloads here.

## Selected mechanism, after attribution and before comparisons

The phase profile found 18 runs at the largest fixture. The old pass rewrites
all 18 into two runs even though reducing to sixteen needs only three runs
merged into one. Select two compatible removals: compact binary private spill
encoding (preserving scalar bits and UTF-8), and a partial final merge pass that
rewrites only enough contiguous runs to meet fan-in while renaming untouched
runs into the next level. Full passes remain when reduction to sixteen is
impossible in one pass. Stable ties follow original contiguous run order.

Freeze three variants: baseline JSON/full-pass, binary-only, and candidate
binary/partial-pass. Run three alternating baseline/candidate pairs at both
sizes, and three alternating baseline/binary-only pairs at 262,144 rows to
separate representation from merge scheduling. Report the binary-only cell
even if it loses. The original success rules are unchanged. Final exact file
hash equality is required for every measured pair.
