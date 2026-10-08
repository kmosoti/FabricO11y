# Cross-system source dissection and finite lab continuation

Prospective registration, 2026-10-08. The owner explicitly requests continued
investigation and downloading/dissecting the other supplied FOSS projects.
This extends the [research program](../../research/cross-system-performance-program.md)
after the verified ordered-prefix correction; it does not reset old receipts.

## Resource allocation and execution

Allocate 7200 additional seconds of serialized project execution to this round.
Keep the old campaign ledger, including failures and approximately 14384 seconds
of frontier consumption. The combined frontier ceiling becomes 21600 seconds;
the original 86400-second overall campaign ceiling remains. This is prospective
continuation scope, not retroactive success or a new per-process memory limit.
Each command still runs through `python3 -B tools/resource_group.py -- ...`,
with 16/20 GiB memory high/max, zero swap and at most 1800 seconds per service.
Use the existing coordinator lock, data-drive scratch/cache and 16 GiB free
reserve. Aggregate active scratch must remain within 8 GiB.

New evidence root: `data/cross-system-run-01`. Its 1 GiB allocation is additive
to the historical retained evidence, which remains inventoried separately.
Initial allocation was capacity/query/operations 256 MiB each, source retrieval
192 MiB and coordination/build/verification 64 MiB. Prospectively reallocate
128 MiB from query to operations after `prefix-load-03`: capacity 256 MiB,
query 128 MiB, operations 384 MiB, source 192 MiB, coordination 64 MiB. The
1 GiB aggregate is unchanged. A subsequent measurement repair reallocates
32 MiB from completed source retrieval to capacity: capacity 288 MiB, query
128 MiB, operations 384 MiB, source 160 MiB, coordination 64 MiB. Existing source
evidence, including the failed download prefix, fits the reduced allowance.
That successful diagnostic used a 2 ms observer
instead of the registered 10 ms; preserve it and repeat the same 18 cells at
10 ms. Its approximately 179 MiB retained evidence remains charged. Query must
fit the smaller allowance or stop; do not truncate evidence. Record allocated and logical bytes;
reserve the entire failed-case state before native admission. Do not silently
delete failure evidence to fit. Existing C5 has its own unresolved preservation
scope and is not admitted by this protocol. Preserve exact archive readback
before removing owned temporary state. Shared build cache remains on the data
drive. Record every command/exit, source/protocol identity, resource observations,
cleanup and limitation. No remote host or privileged installation is included.

Use `tools/bench/labs/cross_system/run_job.py` inside the resource launcher for
all following jobs. It accounts actual wall time in both historical and new
ledgers, serializes commands and contains their whole descendant tree. Maximum
job allowances: source retrieval 1700 s per invocation, builds 1200 s, memory
900 s, query 1200 s, operations 1200 s, verification 600 s. Their actual sum must
fit the finite 7200-second continuation; these are maxima, not additive grants.

## Download and source review

Retrieve source for Comet, Feldera and its storage-design proposal, Vortex,
Lance, redb, Fjall, Tantivy, Quickwit, ClickHouse, DuckDB, GreptimeDB, Tempo,
Roaring, Vector, OTLP Arrow's collector receiver, FoundationDB, Shuttle, Loom,
fail-rs, proptest, Prometheus, SlateDB and differential-dataflow. Turso is already
downloaded/verified. Resolve any unpinned repository HEAD before downloading,
record its exact SHA and use immutable URLs thereafter. Existing program pins
remain available for matching previous source observations.

For each upstream: at most 128 MiB compressed download, 2 GiB decoded inventory,
16 MiB selected source and 120 seconds network time. Stream member hashing;
do not execute upstream code or extract symlinks. Save complete regular-member
inventory, archive digest, licenses and selected source with exact readback.
Remove the temporary archive after those records are durable. An oversized or
unavailable archive remains a recorded retrieval failure; bounded retrieval of
selected files at the same SHA may follow and must be labeled partial coverage.
Repository names and source-path selection are recorded by the fetcher. A
download is not a whole-engine audit, performance comparison or license opinion.

Dissect actual implementation mechanisms: ownership and fallbacks, work/byte
accounting, publication/recovery, encoding-preserving execution, pruning and
candidate sets, feedback/backpressure, and deterministic failure schedules.
Record the inspected paths and counterexamples to naive transfer. Derive new
candidate trajectories only after inspection; retain distinctions between source
facts, deductions, hypotheses and measured Fabric results.

## Capacity lab: offline memory census

Build existing `readiness_memory_lab` offline, locked, release. Run paired
reference/bounded builders at 16 MiB for steady, adversarial and bigrows, seed
2703204353; alternate treatment order by shape. Child limit 120 seconds,
campaign 900 seconds. Run controls before measurements.

H1: bounded building changes requested-live build peak on identical input;
H0 includes no improvement or a shift to another memory domain. Require exact
input hashes, ordered row/custody ledgers, manifest semantics, applicable
existing file-byte comparisons and no build/run leftovers. This differential
reference is not the independent query oracle. Record build allocation counters,
whole-process RSS/CPU/wall, cgroup timeline and disk bytes separately. Declared
reservations are unsupported by this probe: do not invent a reservation metric
or claim complete M1/service qualification. Preserve full cell state under
128 MiB decoded/64 MiB archive, within the 256 MiB lab total; stop on overage.

Prospective checker correction after `memory/census-01` failed: filter manifest
`rows` counts physical groups, which ADR-0022 permits to differ under the byte
cap. The failed comparison remains failed. Before retry, authenticate each
file, compare its filter count with its own Parquet row groups, and reconstruct
the exact filter bits independently from the actual row strings. Only after
those checks may physical filter counts differ; logical rows, ordered custody,
other manifest semantics and applicable file-byte comparisons stay mandatory.
Run missing, misaligned and corrupted-filter rejection controls first. Use
PyArrow 22.0.0 solely as an independent Parquet decoder, installed as a binary
wheel into owned data-drive scratch and removed after the run; no production
dependency changes. Record its version/metadata and separate Python validation
time from native process costs. Installation and retry share the existing
900-second job allowance and unchanged evidence allocation.

After `memory/census-02`, the parent decoder's resident memory confounded
`wait4` peak RSS despite smaller post-exec `/proc` high-water samples. Replay
the same six cells with PyArrow import and validation confined to a reaped
subprocess and a lean native-spawning parent. Record parent RSS/high-water before
each native spawn. Retain native CPU, wall, allocator and post-exec observations
separately. The historical run remains valid for its allocator/semantic checks;
its wait4 RSS is not a native-only peak claim. Reuse an existing fixture archive
only after exact regular-file path, size and SHA-256 equality with the newly
generated complete state. Preserve a reference receipt and leave the original
archive untouched. A changed, missing or extra file must reject reuse and trigger
full failed-state preservation. Reserve 48 MiB evidence for this replay under
the amended capacity allowance; retain oversized failures in owned scratch and
stop. No new Rust probe, production code or filter predicate is introduced.

## Query lab: bounded work census

Build the existing `completion_query_probe` plain and with its existing
allocation/phase features sequentially in the shared data-drive Cargo cache.
Copy each executable to a distinct owned path before the next build; record
both build commands, features, source identity and binary SHA-256, and execute
only those frozen copies. This prospective clarification preserves dependency
cache reuse without letting the second build replace the first measured binary.
Use shuffled
seed 42, 128 and 2048 records, exact 16 and 1024-byte bodies, limit 64 and three
repeats. Alternate variant order. Each native variant emits 64 first-page
measurements and 64 complete chains over Scan/Walk and tail/Segment layouts.
Reuse the unchanged Python oracle, map reconstruction and rejection controls.
Controls must reject dropped/altered results before accepting timings.

H1: whole-probe and measured first-page costs reveal significant work outside
selected payload processing; H0 is no separable avoidable cost. Capture CPU,
RSS, allocation, inclusive spans and artifact sizes; do not sum nested spans or
label OS cache state cold/warm without control. Complete-chain correctness is
measured; plain query-only full-chain latency is unsupported. Per-case full
state must fit 128 MiB, lab retained evidence 128 MiB. The 8192-row cell is queued
but requires a separate admission decision after this bounded batch.

Prospective follow-up after `query/census-01`: compare the existing compile-time
borrowed-log experiment switch, explicitly 0 versus 1, crossed with plain versus
counted binaries. Keep the registered 128/2048 records, 16/1024-byte bodies,
seed 42, limit 64 and three repeats; balance the four-arm order across cells.
Copy and hash all four executables; verify the reported switch and identical
input provenance. This is 16 variants and 1024 complete oracle-graded chains,
inside the existing 128 MiB query evidence cap and 1200-second job maximum.
H1 predicts at least 1 MiB fewer requested allocation bytes in each matched
2048-row/wide-body sealed selective measured query; H0 includes smaller savings
or unchanged dominant decode work. Report plain CPU/time and adverse broad/tail
cases without a prespecified speedup claim. Preserve all unchanged oracle and
rejection controls, and run existing rejected-row corruption/type controls.
This is an exploratory ablation of existing code, not a production-default
change, fresh held-out confirmation or qualification.

## Operations lab: prefix reclamation under ingest

Use actual Store/intake and production `sealer::pass` against an example-local
historical group-join baseline that calls the same builders and reclaim effect.
Use three paired seeds 2703204353, 2703204354, 2703204355, alternating arm order,
128 stream identities, and three shapes: balanced three workers, naturally
skewed three workers, one-worker mechanism-negative case. Maximum 18 cells,
20 seconds each excluding build. Freeze executable fixture parameters before
measurement in the runner's source snapshot. No production sync/format change.

H1: earlier reclaim reduces journal byte-time without unacceptable ACK cost;
H0: no benefit, missing natural skew or increased shared-resource contention.
Record actual row/byte skew, offered/accepted bytes and rates, Answer latency,
CPU/RSS/cgroup counters, and publication/deletion intervals. A 10 ms filesystem
poll gives interval bounds, not exact checkpoint service times. Preserve exact
Batch replay and idempotent retry; grade pre/post complete Scan/Walk query chains
using the unchanged oracle. A separately selected artificial-delay diagnostic
can establish scheduling only and cannot substitute for the natural-load result.
State ≤64 MiB/cell, retained ≤16 MiB/cell and ≤256 MiB per invocation, within
the amended 384 MiB aggregate operations allocation; stop on divergence
or overage and preserve the full failed state before cleanup.

## Decisions and continuation

Report results even when null, adverse or interrupted. Source inspiration alone
does not nominate production changes. No metric, wire, storage-format, query
semantic or oracle migration is included. Repeated effect and unchanged semantic
checks are required before optimization claims. Fresh confirmation follows any
selected prototype; discarded directions retain their evidence. Continue with
the next admitted cell or source dissection after a result, rather than treating
one successful patch as completion of the research goal.
