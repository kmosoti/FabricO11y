# Query-plan comparison and allocation protocol

Registered follow-up to [lab run 02](dev-small-labs-run-02.md), authorized by the
owner's “Proceed”. Scope: a focused Q2a screen/confirmation on logs and native
host metrics, plus a separate allocation investigation. R2a spans, Q2b retention,
soak, target-host qualification and changing the default plan are excluded.

## Questions and fixed decisions

Native hypothesis: Walk reduces balanced server CPU at the same offered ingestion
and HTTP demand while preserving exact custody/query answers and resource guards.
Null: median paired CPU saving is below 15%, any pair does not improve, or another
declared guard regresses. Primary: equal-weight mean of normal/burst/recovery
server CPU cores; report CPU seconds per completed request as a contextual metric
that includes ingestion/sealing, not pure query CPU. Do not choose another primary
metric after results. Three pairs provide finite repeatability, not significance.

Use the prior 900-byte repetitive/entropy source and 60/60/60-second offers,
five-second settling and twenty-second drain. Fresh H0, 64 MiB journals, one
worker, 1 GiB journal/Segment limits and 24-hour retention; no intended eviction.
Server first two allowed CPUs, client/nodes next two; all native release binaries
frozen. Source content is identical within profile; live wall timestamps, host
metrics and physical layout are not identical across native runs. Exact identical
stored input is tested separately in the allocation study; do not conflate these.

Run order: development Scan, development Walk (one node, 10/30/10 logs/s), then
small Scan-1, Walk-1, Walk-2, Scan-2, Scan-3, Walk-3 (20 nodes, 1000/3000/1000).
Development is diagnostic and excluded from primary paired aggregates. Historical
C1 cells are context only. Do not pool them with the new fixed-demand consumer.

## Demand and correctness

Ordinary requests: exactly 200 scheduled at offer epoch + integer seconds 0..199,
one serial client, rotating recent ten-second logs, absent text and CPU metrics.
Recent windows bind scheduled wall time. Never reset the due time after a slow
request. Preserve scheduled/start/end times, counts by shape/phase, late starts
and observed overlap. Require all 200 requests, 20 per shape per offered phase,
maximum start lateness <=100 ms and at most two concurrent requests in total.
A missed/late demand guard makes performance comparison ineligible.

Visibility: retain the existing 36 source targets but issue exactly one request
per target at source write +3 seconds, in a second serial client. Preserve exact
returned body and original timestamps; all targets must be returned within the
existing 30-second diagnostic horizon. These deliberately delayed probes are
availability checks, not nearest-visibility estimates. No adaptive retry traffic
may make one plan appear cheaper. Require 36 scheduled/completed visibility
requests; record their lateness under the same 100 ms demand guard.

Retain all ordinary pilot guards from the [prior protocol](dev-small-labs-screen-protocol.md):
exact live source bodies/counts and encoded Batch custody, sequences, no unexpected
gaps/retries/exits, independent quiescent oracle answers and negative controls,
valid integer clock/phase/rate accounting, server HWM <=2 GiB and node <=64 MiB,
collection-to-ingest/ACK-observed p99 <=1 second and producer lag p99 <=100 ms.
Live returned rows have fidelity checks; complete concurrent snapshot answers are
not established by that alone. Fixed config, campaign hash, binaries and consumer
source must be bound in the receipt. Supplemental audit independently reconstructs
request counts/schedule/concurrency and rejects missing, late and plan-mismatch
controls before timing. Existing query/delivery oracles remain unchanged.

For a favorable native decision require all correctness/demand guards, each small
pair CPU ratio <1 and median Walk/Scan CPU ratio <=0.85; median phase-peak RSS ratio
<=1.10. For every ordinary shape/phase, median-of-pairs latency must be no more
than max(1.10 times Scan, Scan +2 ms). Median ACK p99 must be <=1.05 times Scan
(declared diagnostic jitter tolerance); no retries, no final pending/file backlog,
and median peak pending/file backlog no higher. Report every individual result,
including failures. A failed guard stops dependent native cells. A performance
regression is a trade-off, never suppressed by a good primary metric.

## Allocation study

Use the existing private `responsibility_probe` query mode and unchanged Python
`grade_query` oracle. Seed42, 65,536 shuffled 1,024-byte log bodies (~64 MiB), four
shapes (empty/selective/common/broad), tail and Segment layouts, Scan and Walk,
first call plus three warm calls. Both plans use the exact recovered records.
Grade every actual answer and changed/missing/duplicate controls. First means a
fresh History/cache object, with buffered OS pages; it is not a cold-disk query.

Build plain and counted+phase-probe executables separately and copy each before
the next build. Counted uses existing `responsibility-alloc-probe,phase-probe`
features and `BENCH_PHASES=1`; no new production instrumentation. Report baseline,
peak, incremental live heap, cumulative requested bytes and existing nested phase
snapshots, alongside the uninstrumented counterpart. Nested/global peaks cannot
be summed as exclusive ownership. Instrumentation timing is descriptive and its
perturbation is reported; it does not enter native speed ratios. Fixed Scan-before-
Walk and tail-before-Segment order is a limitation. Run tiny controls first.

Preserve all distinct actual answers and their complete hash/mapping ledger;
lossless deduplication is allowed only after equality verification. Retain exact
recovered input, oracle verdicts, commands, source and binary hashes, clocks and
resources. Stop if evidence cannot fit, rather than discard distinct answers.

## Budget, labs and completion

This is a new finite allocation, not reuse of the previous queue: <=3600 seconds
measured command time, split native2400, profiling/build900, other checks/cleanup300.
Admit native cells using observed estimates (initially215s development/300s small),
with <=900s individual deadline, and defer before launch if an estimate cannot fit.
Never shorten a trial. All work is serialized through `tools/resource_group.py`
and `tools/bench/labs/query_compare/run_job.py` under the existing20 GiB maximum,
16 GiB high/no swap/30-minute outer deadline. Data-drive scratch only, <=4 GiB,
>=4 GiB free reserve; clean successful fixtures, retain/review failures first.

Evidence root: `data/query-plan-run-01/`; capacity owns allocation evidence, query
owns native cells, recovery owns independent audit/verification. <=50 MiB retained
per lab. Estimated delegated budget12,000 tokens (9,000 initial work,3,000 reserve),
actual usage only where exposed. Inline coordinator acts as query/recovery PI;
capacity PI prepares profiling, Luna provides bounded review. No extra agents or
workloads merely to fill slots. Register this protocol separately before timing;
preserve old protocols/results. Final report records all exits, cleanup, budget
stops, source differences, uncertainty and remaining release prerequisites.
