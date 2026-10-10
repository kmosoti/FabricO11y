# Storage catalog and Rust: lab development

Status: **developed investigation plan; prototypes and comparisons not run**.
Scope added by the owner on 2026-10-05: query needs a map across pending and
published data, storage should own its read boundary, and Rust should be an
explicit performance and hardening dimension. This extends the
[frontier campaign](performance-frontier-lab-plan.md); it does not register new
acceptance criteria or change production behavior.

## What exists and what is proposed

`query::History` owns `Mutex<WalkState>`. Walk already uses journal-entry bounds,
compressed tail blocks, Segment metadata and optional filters. `query.rs` still
opens physical files, decodes journal frames and handles physical source types.
`tail::visit_block` already filters borrowed `ObservationRef` fields; it is not
an untouched zero-copy opportunity. `spill::write_row/read_row` allocate private
byte buffers per row. The [latest results](performance-frontier-run-01.md) record
spill amplification and an unresolved partial-table verifier-model mismatch.

Proposed boundary, initially an internal server module:

| Query owns | Storage read catalog owns |
| --- | --- |
| Predicate, ordering, rate and pagination semantics | Physical locations, bounded source readers and lifecycle tracking |
| Choosing which candidate to read next | Sound candidate bounds, validated optional filters and fallback |
| Constructing exact answers | Snapshot-relative coverage and explicit read/availability outcomes |

Keep semantic decisions in the pure core. Start with a concrete narrow API over
existing mechanisms; choose a port only if an effect boundary needs it. A new
crate or generic repository framework is not required for the first experiment.
Changing ownership alone is not assumed to improve speed.

Conceptual flow (proposal, not an implemented interface):

```text
query predicate + logical snapshot
          -> storage read catalog
          -> candidate descriptors + coverage evidence
          -> bounded readers
          -> exact selection and answer construction
```

Descriptors identify sources and expose conservative bounds, not complete decoded
data. An unavailable bound means unknown, not empty. Any type sketch must explain
who owns bytes, what invalidates a handle and what a dropped handle releases.
`Arc` keeping memory alive does not authorize keeping expired records queryable.

## Three standing labs

| Lab / PI | Packet | Experiments and decision |
| --- | --- | --- |
| Query and availability / `query_landscape` | [Query catalog](catalog-labs-query.md) | Isolate the read boundary; compare query-driven and storage-maintained map updates; integrate Q3 attribution |
| Capacity and lifecycle / `capacity_landscape` | [Rust memory and cost](catalog-labs-capacity.md) | Reuse bounded buffers, delay remaining owned materialization, measure snapshot memory retention |
| Recovery and operations / `operations_landscape` | [Coverage and typed handles](catalog-labs-operations.md) | Establish map soundness across append/publication/reclaim; test invalid handles and partial availability |

PIs use GPT-6.1-SOL medium. Root admits workloads and reviews cross-lab evidence.
Luna assistants receive one bounded task at a time: fixture inventory, allocation
ledger or mutation/receipt audit. They do not independently launch workloads.
Estimated preparation allowance is 12000 tokens plus 3000 contingency; actual
token telemetry is unavailable, so these are dispatch estimates, not usage claims.

Developed experiment queue:

| ID | Hypothesis under test | First artifact / dependency |
| --- | --- | --- |
| O1–O5 | The map preserves coverage, publication, reclaim, retention and sound pruning | Deterministic cut fixtures with independent rejection controls |
| CQ1 | A storage-owned read boundary preserves behavior without material overhead | Narrow module prototype after coverage fixtures; existing Walk baseline |
| CR1 | Bounded spill-buffer reuse reduces allocation traffic | Isolated encode/decode variants with identical bytes; independent of CQ1 |
| CQ2 | Proactive map maintenance reduces total cost under repeated demand | CQ1 plus query-off/lazy/eager schedules and refresh-failure fallback |
| CR2 | Borrowed Parquet inspection avoids constructing rejected rows | Q3 phase evidence; preserve validation and owned winning rows |
| CR3 | Immutable metadata views reduce cloning/lock cost within a retention bound | CQ1 and measured lock/clone cost; slow-reader and cancellation controls |

O1–O5 are small correctness cells, not five performance sweeps. CQ1 is a
preservation experiment; CQ2/CR1–CR3 need a measured benefit. Start with the
coverage design and CR1; do not implement all mechanisms before seeing results.

## Admission sequence and decisive comparisons

1. **Make the evidence usable.** Resolve formatting/Clippy in affected prepared
   harnesses, capture every oracle invocation losslessly, and design partial-table
   verification separately from the existing whole-record unavailability model.
   The old failed run stays failed. No missing-table failure may be silently
   removed or reclassified to clear a gate.
2. **Establish coverage before map optimizations.** Enumerate active journal,
   closed pending journal, published Segment and retention transitions; compare
   exact candidate coverage and full query chains. Add negative controls that
   demonstrate false-negative pruning and duplicate coverage are detected.
3. **Separate the boundary without changing the algorithm.** Compare existing
   Walk to the catalog-backed path using the same map/update policy and exact
   answers. Attribute any new overhead before adding a maintenance worker.
4. **Run independent small ablations.** Buffer reuse can proceed independently
   of catalog scheduling once its own correctness prerequisites pass. Compare
   lazy versus proactive maintenance with query-off controls to expose costs
   shifted into ingestion. Add immutable snapshots only after lock attribution.
5. **Combine only independently supported changes.** Compare baseline, catalog
   alone, allocation change alone and both on matched data/demand. Confirm the
   best feasible combination in fresh trials and deployment-limit cells.

Suggested experiment API names in packets are design sketches. Before execution,
each new protocol must have real commands, binary/source hashes, frozen fixtures,
seeds, expected output and its own registration commit. The existing runner's
completion-campaign accounting is insufficient by itself for new comparisons;
root also maintains the frontier aggregate budget. No invented CLI is executable.

## Common measurements and controls

Measure CPU per accepted MiB and per complete query, first-page and full-chain
wall time separately, allocations, peak live heap, process RSS/HWM, retained
buffer capacity, cgroup anon/file/kernel and pressure, IO, disk and catalog bytes.
Add source candidates/visits, decoded bytes, lock wait/hold time, rebuild time,
retired snapshot bytes and oldest live handle age. Record total service cost,
including map maintenance when queries are absent.

Keep Spool accepted Batches/s and bytes/s, attempts/refusals, durable ACK latency,
retransmits, oldest backlog age and recovery drain in native cells. A local trace
HTTP200 is Spool acceptance, not server ACK. Polling visibility is a bounded
observation; existing freshness targets remain fixed.

Use the same payloads/identities and offered query schedule within each pair.
Diagnostic probes and counted allocation builds are separate from plain timing.
Compare Scan for exact-reference context and existing Walk as the optimization
baseline; neither agreement alone replaces the independent oracle. Keep complete
pagination and metadata grading. Warm process and warm OS cache are different.

Screen one pair per declared cell; nominate at most one candidate per mechanism.
Confirmation uses three fresh alternating pairs. Before measurement, register
the primary metric and a practical threshold: target at least 10% reduction in
that cost in every pair, with at most 5% regression in paired CPU, peak memory
and latency guards, no accumulating backlog and no correctness failures.
If a small sample cannot support a latency quantile, report individual samples
and classify that guard inconclusive. A cleaner boundary may be accepted for
maintainability separately, but receives no performance-win claim without data.

## Bounds, cleanup and completion

Use the existing frontier four-hour total, not an additional four hours. Its
last recorded consumption is about 429.166 seconds; reconcile receipts before
admission. Allocate up to 45 minutes each for the three initial lab screens,
leaving the rest for prerequisites, independent confirmation and uncertainty.
Each command stays under the default 30-minute launcher deadline. Serialize
project builds, tests, validators and experiments through:

```sh
python3 -B tools/resource_group.py -- COMMAND
```

All descendants remain inside 16 GiB high / 20 GiB max / zero swap; scratch and
build caches use `/run/media/kmosoti/data/FabricO11y`. Enforce 8 GiB owned scratch
and 16 GiB free-space floor, 256 MiB evidence per lab, plus tighter application
limits in deployment cells. Verify actual placement and limits. Stop on OOM,
coverage/custody error, invalid clocks, exceeded budgets or unenforced bounds.
Archive failures before cleanup, clean successful owned temporary files and
processes, preserve shared caches and report remaining disk use.

Each PI delivers results including nulls, failure traces, resource/cleanup
receipts and one decision: reject, revise, independently confirm or adopt a
specific scoped change. Root reconciles CPU moved between components and memory
retained across snapshots. A compile-time guarantee covers API misuse only;
publication ordering, durability and filesystem races still need runtime evidence.

No persistent index format, wire change, sync-order change, default-plan change,
unsafe optimization, new dependency, release or remote workload is proposed in
this first screen. Persistent maps and adaptive scheduling remain later branches
only if rebuild or fixed-policy measurements justify their added responsibilities.
