# Source mechanisms to test in Fabric

Research synthesis, 2026-10-08. The owner requested downloading and dissecting
the supplied FOSS sources, then using their engineering to generate and test
new trajectories. This is implementation research, not a storage migration or
an upstream benchmark. The [finite continuation](../experiments/benchmarks/cross-system-continuation-protocol.md)
registers containment, provenance, workloads and evidence limits.

## What was downloaded and inspected

Twenty-four additional repositories now have complete regular-member source
inventories. The [matched sweep](../experiments/benchmarks/cross-system-sweep-protocol.md)
repaired the three earlier incomplete downloads: ClickHouse at its existing pin,
and Vector/FoundationDB at the explicit replacement revisions already used for
selected-file inspection. ClickHouse required a 512 MiB compressed-archive
allowance and a 16 MiB compressed-inventory allowance. Its complete 361264396-byte
archive was reused exactly after the first inventory attempt failed; it was not
downloaded twice. The historical 128 MiB cap and HTTP 404 failures remain recorded.
New pins do not turn the historical revisions into successful retrievals.
Turso's earlier archive and 24 matching reviewed blobs remain separate evidence.
The Feldera storage-design proposal is counted separately from its implementation
repository. Complete retrieval does not imply a recursive submodule checkout.

Selected implementation files, licenses, immutable revisions, complete archive
member inventories where available, requested-path failures, archive hashes and
cleanup receipts are retained under
[`cross-system-run-01/source`](../experiments/benchmarks/data/cross-system-run-01/source).
The earlier full successful archives were removed after verified inventory and
selected-source preservation. The three repaired complete archives remain under
[`cross-system-sweep-01/source`](../experiments/benchmarks/data/cross-system-sweep-01/source).
The new independent checker reread every regular member of the three repaired
archives and bound each selected excerpt to its full inventory. Its self-consistent excerpt-substitution
control catches a provenance gap that separate archive and excerpt checks missed.
The bounded historical failed ClickHouse prefix remains. Retrieval executed no
upstream application or build script; later registered workload execution is
separate from this download claim.

| Dissection | Systems | Mechanisms actually inspected |
| --- | --- | --- |
| [Storage](cross-system-storage-dissection.md) | Comet, Feldera implementation and proposal, Vortex, Lance, redb, Fjall | Reservation ownership; cancellation drain; retained versus construction memory; merge priority and fuel; compressed predicates; snapshot owners; oldest journal obstruction |
| [Query](cross-system-query-dissection.md) | Tantivy, Quickwit, DuckDB, GreptimeDB, Tempo, Roaring, ClickHouse | Candidate IDs versus payloads; index maintenance; sound early termination; serialization overlap; trace-specific lookup; container dispatch; saturated-index fallback |
| [Operations](cross-system-operations-dissection.md) | Vector, OTLP Arrow receiver, FoundationDB, Shuttle, Loom, fail-rs, proptest, Prometheus, SlateDB, differential-dataflow | Admission lifetime; decode before reservation; modeled persistence loss; exploration stop reasons; failpoint activation; replay/shrinking; numerical codecs; publication/reclamation; input versus output work |

These are selected-function dissections. An archive inventory does not imply a
whole-engine audit, transitive dependency review, license opinion or measured
performance advantage. Exact paths, source links and transfer limits are in the
three dissection records.

## Patterns that survived comparison

**Account for the lifetime that consumes the resource.** Comet's explicit drain
and Fabric's existing completion-owned query permit address the same ownership
boundary. Lance constructs ranges before deciding whether to retain them;
OTLP Arrow converts input before queue-byte admission. Those are concrete
counterexamples to equating a retained cache or queue allowance with the whole
process's peak memory. Fabric should observe construction, retained owners,
worker completion, allocator state and cgroup memory separately. Independent
peak measurements cannot be added to reconstruct simultaneous pressure.

**Relieve the oldest real obstruction.** Fjall's journal manager inspects what
prevents reclaiming the oldest journal; Feldera also considers batch fanout
before resident memory. Fabric's ordered-prefix repair removes one unnecessary
wait, but checkpoint work still shares the commit thread with ingest. The
experiment must measure both retained journal byte-time and ACK tails.

**An acceleration structure needs permission to stop helping.** ClickHouse's
saturated set becomes non-pruning. DuckDB disables a cardinality shortcut when
filters invalidate its premise. Tempo's trace lookup depends on ID ordering;
its general search takes another path. Fabric should preserve exact fallback
and choose a candidate representation from predicate semantics and measured
shape, rather than mandate one index for every field. Tokenization does not
implement arbitrary literal substring matching.

**Budget consumed work, including work with no output.** Differential-dataflow's
inspected merge can consolidate many inputs to zero while its output-based fuel
charge remains unchanged; the Spine also supplies fuel independently to layers.
Fabric should measure input bytes, decoded rows and comparisons before adopting
a scheduling quantum. Weighted consolidation is not custody deduplication.

**Model the state that survives a failure.** FoundationDB distinguishes pending
writes, sync and simulated persistence loss. Vector distinguishes reader
progress, durable checkpoints and asynchronous physical deletion. A successful
function return, visibility to a local reader and durable custody are separate
events. Fabric's recovery experiments should preserve requested/applied fault
cuts and resulting file images, not count every injected error as equivalent
coverage. This does not establish real-device power-loss behavior.

## Derived models and competing trajectories

These are deductions and hypotheses, not calibrated controllers or novelty
claims. They refine the existing D1–D5 cards rather than add unlimited prototypes.

For maintenance, measure `J = integral(retained_journal_bytes(t), dt)` together
with ACK latency and live memory. A candidate action's useful relief is the
eligible prefix it actually releases, not the size of its own output. Compare
oldest-obstruction, largest-resident and FIFO policies independently before
combining them. The null is that checkpoint contention or delayed input
reclamation cancels the apparent benefit.

For optional indexes, let `C_build` include construction, publication and future
maintenance; let `delta_C_query` be the measured saving per applicable query.
Only when `delta_C_query > 0` is the simple amortization threshold
`q_break_even = C_build / delta_C_query` meaningful. Expiry, repeated conversions,
snapshot ownership and memory occupancy are additional constraints. Test zero,
one and repeated queries, clustered and shuffled rows, and common predicates.
The null is that reuse never pays for the extra representation before expiry.

For bounded scheduling, a target quantum `Q` requires counting consumed work.
Even with correct accounting, work can exceed `Q` by the largest indivisible
operation; there is no fixed latency bound without bounding that operation and
its cost. Measure cancellation-to-worker-completion and oldest-prefix progress
alongside throughput. The null is that more yielding retains inputs longer and
increases total byte-time without improving responsiveness.

For compressed predicates, Vortex gives a concrete mechanism, not a general
permission to skip decoding. Charge conversion, dictionary construction,
candidate verification, exact literal fallback and final materialization.
Compare with Fabric's existing Walk and integrity checks on the same output.
The null is that preparation and verification dominate the saved payload work.

The census and [matched workload sweep](../experiments/benchmarks/cross-system-sweep-findings.md)
have now exercised these mechanisms. The sweep includes native sort-cap and
phase-attribution measurements, Arrow/DuckDB/Vortex query comparisons, RowSet
representation/locality controls, prepared scan reuse, and real Store admission
cadence. Its [research vectors](cross-system-sweep-vectors.md) incorporate adverse
regimes and rejected predictions. Nomination remains constrained by measured
costs and the existing two-prototype ceiling. No production format, durability
ordering, query semantics or dependency changed in this source-research round.
