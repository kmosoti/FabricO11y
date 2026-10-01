# Query engine direction

Status: **exploratory research**, tier 6 of the [source-of-truth order](../README.md#source-of-truth). This page reviews a proposal to give Fabric its own query engine (a logical algebra, a RowSet primitive, granule statistics, a vectorised executor, an evidence algebra, budgets, adaptive planning, a pipeline language) and sets out a plan. It decides nothing: a decision needs an ADR and a milestone with explicit scope ([AGENTS.md](../../AGENTS.md#scope-rule)). Every number here comes from a recorded run, and the two runs made for this review are exploratory, not registered.

## Verdict

The proposal's thesis is right, and it is already Fabric's thesis. "What is the smallest amount of evidence I must examine to answer this question correctly, and what can I truthfully claim about the answer" is what the [product contract](../PRODUCT-CONTRACT.md) promises with `complete`, `unavailable`, `freshness`, `gaps` and the retained window, and what the [retained-history contract](../architecture/retained-history.md) specifies. The part of the proposal worth building is the part that makes that claim formal: an evidence algebra with proved composition rules.

The proposal's diagnosis of where time goes is wrong for Fabric as it exists. It assumes the cost is "deciding which observations can matter" across thousands of segments and granules. The measured cost is elsewhere:

- With an unsealed journal tail, **every query pays the whole tail**, whatever it asks: 126 ms for a query that can match nothing, on a 40 MiB tail, against 1.1 ms once the same bytes are a Segment ([attribution run 01](../experiments/benchmarks/query-attribution-run-01.md)). The read path decodes every frame and every OTLP payload before it filters anything. A tail reaches 64 MiB before each seal, so this fixed cost reaches about 200 ms at the end of every sealing cycle.
- Inside a Segment, **the cost is the rows materialised, not the rows returned**: about 40 ms for a logs query over 63,950 rows whether it returns 50 or 638, and whether the filter keeps 1 % or half. Every column of every row in a surviving row group is built before the filter runs, and a `limit 50` query reads all eight row groups although the first one decides the answer.
- At the median, a line written on a host is queryable 517 ms later, and 85 % of that is the Spindle's one-second log poll ([collection-to-query run 01](../experiments/benchmarks/e2e-latency-run-01.md)). The query is not on the critical path of freshness unless the tail is large.

So the first engine work is not an engine. It is four mechanisms the existing path lacks, each removing a measured cost, each checked against the current path as an oracle, under the existing three query kinds and the existing Segment format. A planner, an IR and a vectorised executor are deferred behind a stop rule, because the registered query gate passes with four times headroom (p99 at most 481 ms against 2 s in [history run 01](../experiments/benchmarks/history-run-01.md)), and because the contract forbids indexes beyond row-group statistics until a registered gate fails.

## The proposal against what exists

The proposal argues against making DataFusion the primary engine. Fabric never made that choice: the server reads Parquet through `parquet` and `arrow-array` directly ([Cargo.toml](../../crates/fabric-server/Cargo.toml)) and holds its query decisions in a pure kernel ([fabric_core::query](../../crates/fabric-core/src/query.rs)). Point by point:

| Proposal | In Fabric today | Verdict |
| --- | --- | --- |
| 1. A query algebra, with syntax as adapters | Three query kinds as an enum (`Query::Logs`, `Metrics`, `Rate`), one JSON shape over HTTPS, `fabricctl` as the one client. No algebra, no second syntax | Defer. An algebra pays off at the second query shape that composes operators; there is no such shape in the contract |
| 2. A RowSet primitive and selectors | Present at one level: `segment::prune` returns the row groups whose time statistics overlap the window. Absent for the journal tail, which has no selection at all | Adopt the idea at the two granularities Fabric has: tail entries and row groups. No bitmaps are needed at these sizes |
| 3. A storage hierarchy visible to the planner, with granule statistics | Segment (manifest: group range, receive bounds, newest time per node, file hashes), row group (Parquet statistics on the time column, 8,192 rows), column. `History::sources` reads every Segment's manifest and `gaps.parquet` on every query and skips none by time or node, although the manifest holds both | Adopt what the manifest already allows. New statistics (dictionaries, Blooms) are forbidden by the contract until a registered gate fails |
| 4. A vectorised executor | Row at a time: `scan_logs` builds a `LogRow` per row with three heap allocations and a JSON parse of the attributes, then calls the filter. Measured 0.6 µs per log row and 0.24 µs per metric point; a 10,000-row answer spends at most 6 ms there | Defer. Reading fewer rows is worth ten times more than reading rows faster |
| 5. Temporal correlation as a primitive | Absent. Logs and metrics are queried separately; traces are a non-goal of the first profile | Out of contract |
| 6. Time as a native type | Present: every query carries a mandatory `Window`; the snapshot intersects retention; row groups are pruned by time. Absent at the Segment level | Adopt the Segment-level pruning; the rest exists |
| 7. Signal semantics first-class | Present: metric rows carry kind, sum, monotonic; rates follow the counter contract through `counter_step`, proved for all inputs by Kani (HIST-6) | Keep |
| 8. Evidence as part of execution, with composition rules worth proving | Present in every answer: `complete`, `unavailable`, `retained_from_ns`, `retained_to_ns`, `freshness`, `gaps`, `snapshot`; the kernel has `complete` and `page_snapshot_retained`. Absent: a rule that says what skipping a source may and may not change, and a proof of it. The earlier [coverage-receipt research](../architecture/query.md) is the same idea at block level | **Adopt first.** This is the one novel part, and it is the gate the other three mechanisms must pass |
| 9. Budgeted execution | Partial: `limit` at most 10,000, a bounded heap, queries on a blocking task. No deadline, memory or I/O budget; the tail is loaded whole | Partial adoption: the tail index bounds memory; a deadline is a separate decision |
| 10. Dynamic strategy changes | Absent | Defer: nothing to choose between yet |
| 11. A logical plan enum | Absent | Defer behind the stop rule |
| 12. Specialised physical operators that make predicates disappear into storage | The fixed pipeline per kind does this for time already | Adopt for node and key bounds (below) |
| 13. A canonical predicate representation | Predicates are three fixed fields: node, `contains`, metric name | Defer until predicates compose |
| 14. Selection separate from materialisation | Absent, and the central gap: the tail decodes everything, the Segment scan materialises every column of every candidate row | **Adopt** at both levels |
| 15. Crate boundaries: a pure IR crate, an optimiser crate, an executor crate, an index crate | The pure kernel already lives in `fabric-core` under the layer gate; the adapter code in `fabric-server`. Four new crates would be frameworks without consumers | Defer; a new crate needs a second consumer |
| 16. Morsel-driven parallelism | One blocking task per query; the registered profile gives the server two CPUs | Defer |
| 17. A pipeline query language | Absent | Out of scope: no consumer, and the contract requires one for every feature |
| 18. Do not optimise arbitrary relational queries | Agreed; the contract already limits the surface to three kinds | Keep |
| 19. The thesis: minimise evidence examined subject to semantic and evidence correctness | The product contract | Keep, and make it checkable |

The proposal's own ordering builds the infrastructure first (plan, catalog, RowSet, executor) and measures at step nine. Fabric's rule is the reverse: register the measurement, then build what it justifies ([ADR-0018](../decisions/ADR-0018-accept-work-on-executable-evidence.md)).

## What the measurements say

From [query attribution run 01](../experiments/benchmarks/query-attribution-run-01.md), one 40 MiB state, median of 12 queries, milliseconds:

| Shape | Unsealed tail | Sealed Segment | Rows |
| --- | ---: | ---: | ---: |
| empty window | 126.0 | 1.1 | 0 |
| host logs, limit 1,000 | 129.3 | 43.7 | 638 |
| logs, limit 50 | 139.5 | 39.4 | 50 |
| text search, rare | 136.7 | 40.0 | 33 |
| metric history | 124.6 | 17.0 | 22 |
| fleet metrics, limit 10,000 | 151.9 | 33.1 | 2,200 |

What each proposed optimisation would have done to these numbers:

| Mechanism | Effect on the tail column | Effect on the Segment column |
| --- | --- | --- |
| Segment and granule pruning (proposal 3) | none: the tail is not pruned by anything | none here: one Segment, whole window |
| A vectorised executor (4) | none: the cost is decoding, not the filter loop | at most a few times on the 40 ms, by making per-row work cheaper |
| Selection before materialisation (14), at the tail | **removes the fixed cost**: select entries by node and time from an index, decode only those | — |
| Selection before materialisation (14), inside a row group | — | **about 90 % of the bytes avoided** for the 1 % node query: the body column is about 90 % of `logs.parquet` by size |
| Early termination on the sort key (12, the Top-N idea) | the same selection, bounded | **1 of 8 row groups** for `limit 50` over the whole window; 1 of 14 at a 64 MiB file |

The last two rows are arithmetic over the Segment's row-group statistics, not measurements; the plan registers them as predictions. The sealer's output order makes them possible: row groups are sorted and disjoint in time, which [ADR-0022](../decisions/ADR-0022-build-segments-by-external-merge-sort.md) keeps.

## Where the proposal conflicts with the rules

- The [non-goals](../PRODUCT-CONTRACT.md#non-goals-of-the-first-profile) exclude traces, plugin boundaries and "any index beyond Parquet row-group statistics unless a registered gate fails". Blooms, token postings, dictionaries per granule and trace navigation are out until a gate fails; no gate has.
- [AGENTS.md](../../AGENTS.md#architecture-rule) forbids plugin systems and frameworks without a boundary to justify them, and [ADR-0016](../decisions/ADR-0016-keep-a-pure-semantic-core.md) keeps the core free of dependencies: a `no_std` IR crate would be allowed, but four crates for one consumer would not.
- Every feature needs a current consumer and a test ([contract](../PRODUCT-CONTRACT.md#non-goals-of-the-first-profile)); SQL, a pipeline language and a Grafana adapter have none.
- The oracles are the independent Python [query and rate oracles](../QUALIFICATION.md#tooling-classes). DataFusion, ClickHouse and DuckDB can be benchmark opponents in a research experiment with a registered workload; they cannot be oracles, because the contract's semantics (completeness, snapshot-bound pages, reset markers) are not theirs.

## The plan

Four milestones, in this order, under the existing format and query kinds. Each removes a measured cost, keeps the current read path as a differential oracle, registers its mutants before its implementation, and names what would falsify it. None adds a crate, a dependency, a persisted format or a configuration key. A measurement protocol is registered before the first one starts: the fixtures of the [history protocol](../experiments/benchmarks/alpha-phase4-history-protocol.md) in both modes, the five registered query shapes plus the empty window, `limit 50` and a node-selective query against a 64 MiB tail; per shape, wall p50 and p99 over 20 instances, rows materialised, bytes read, row groups opened, index memory and time to ready; the gates are oracle equality and no change to any evidence field against the full-decode path, with the speed figures recorded as predictions, not gates, until a baseline exists.

### Q1. The evidence kernel

Make the composition rule explicit and prove it, before anything is allowed to read less.

In `fabric_core::query`, with `Copy` types and no allocation, as the other kernels: the facts a source publishes (`SourceFacts`: group range, receive bounds, time bounds, and whether the queried node appears), a `Disposition` (read, skip because no time can overlap, skip because the node is absent, unavailable), a function from facts, window, snapshot and node filter to a disposition, and a `Claim` (retained bounds, completeness) folded over sources. The server keeps per-node freshness and gaps in its own maps; the kernel decides what a skip may change.

Invariants to prove with Kani over abstract rows, and to search with proptest:

- **E1, skipping is silent.** For any source and query, folding a *skip* disposition into the claim gives the same claim as folding a *read*. Reading less never changes the retained window, the freshness or `complete`.
- **E2, unavailability is sticky.** Once a source is unavailable, no later source makes the claim complete.
- **E3, a skip is sound.** A skip is returned only when no row the facts admit can satisfy the window and node filter. (The facts bound the rows; the differential test checks that the manifest facts are true of the Segment.)
- **E4, pages keep evidence.** Within one snapshot, the dispositions do not depend on the page.

The server then uses the disposition for Segments, which today are all read. Mutants: a skip that drops the retained window; a skip at a boundary (`time_max == from_ns`); a disposition that reads a Segment whose manifest lacks the node yet claims it skipped. Falsifiers: any oracle-graded answer changing; any Kani harness failing. The expected measurable effect is small (manifests are small); the point is that Q2 to Q4 are checked against E1 to E4.

### Q2. A selectable journal tail

The largest measured cost, and it falls on every query.

An index of the unsealed tail, held in `fabric-server` beside the journal adapter: for each committed entry, its group, its position in the frame log, the node, the receive time, the time bounds of its log rows and of its metric points, and whether it carries gaps. About 80 bytes per entry; a 64 MiB tail at the soak workload is about 50,000 entries, so about 4 MB, bounded by the journal file size. The index is extended on demand: a query reads `committed_group`, extends the index from the last indexed frame to that group under a lock, and drops entries for groups that a Segment now covers. No new thread, nothing on the commit thread, nothing persisted: a restart rebuilds it from the journal it already replays. Selection takes the entries whose node, time bounds and group pass the kernel's disposition; materialisation decodes only those entries' Batches. Freshness, receive bounds and gap presence come from the index without decoding.

The full-decode path stays as the test oracle. Tests: a differential over registered states and over generated tails with backlogs and interleavings; the crash-state test for HIST-2 extended to a tail that is sealed between indexing and reading (the existing "journal moved" retry covers it). Mutants: an entry the index misses; an index not extended to `committed_group` (stale); an entry kept after its Segment exists (double counting). Falsifiers: any differential mismatch; index memory above the bound; ACK p99 moving in the registered soak.

Predicted: the empty-window and node-selective shapes on a 64 MiB tail from about 200 ms to under 5 ms; the history protocol's freshness probe, whose journal-only queries took about 1.3 s each, to tens of milliseconds.

### Q3. Stop when the answer cannot change

Every logs and metrics answer is the `limit` smallest keys within a window, keys are `(time, node identity, sequence, index)`, and the store is time-ordered at row-group level. So: order the selected row groups and tail entries by their minimum time, scan ascending, and once the bounded heap is full with threshold `T`, skip every source whose minimum time is above `T` and stop. A page token's `after` key gives the symmetric lower bound: skip sources whose maximum key is at or below it, instead of reading and discarding them as today. Both bounds come from statistics the scan already reads.

The kernel gains a pure `can_skip(bounds, lower, upper)`; Kani proves it never skips a source that could hold a key in the open interval; a mutant skips at the boundary. The oracle is the full scan. Falsifier: any differential mismatch; the heap's contents differing from the full scan's on any registered shape.

Predicted: `limit 50` over the whole window from 39 ms to about 5 ms (one row group of eight); the protocol's host-logs query and its freshness probe similarly. Rate queries, which have no limit, are unchanged.

### Q4. Materialise only survivors

For the row groups that remain, decode the key columns first (time, node identity, sequence, index, and the metric name), compute the selection with the node filter, the window, the page bound and the Q3 threshold, then decode body and attributes (or value, unit and attributes) only for the selected rows, using the Parquet reader's projection and row selection. Text search still decodes every candidate body; attributes stay deferred. `LogRow` and `MetricRow` remain the output types.

This is the most code and the least certain gain, so it comes last and only if the registered measurement after Q3 still shows the node and name shapes bound by decoding. Predicted: host logs at 1 % selectivity from 44 ms to about 5 ms. Falsifier: rows not byte-identical to the full scan's.

### After Q4

Rerun the registered history protocol unchanged as run 02 and compare with run 01; register the collection-to-query harness as a protocol and run it; run soak run 02 after the sealer milestone. The predictions above are then results, or failures recorded as such.

### The stop rule

Build the proposal's larger engine only when one of these holds:

1. The contract gains a query shape that composes operators (an aggregation over windows, a boolean combination of predicates, a join of any kind). Then the algebra is a `Plan` type in `fabric_core::query` whose typing rule is the evidence kernel from Q1, and it gets its own crate only when a second consumer exists.
2. A registered gate fails after Q1 to Q4. Then the contract clause on indexes is the thing to change, in its own commit, before any index is built.
3. A profile after Q4 shows per-row work dominating a registered shape. Then a columnar batch between scan and filter is justified, for that shape.

Until then: no SQL, no pipeline language, no cost model, no adaptive planning, no morsels, no correlation, no traces, no Blooms, no postings.

## Why this is better than the proposal's order

- It is ordered by measured cost, not by architecture. The tail (every query, up to 200 ms) comes before row-group work (tens of milliseconds) which comes before per-row work (microseconds).
- Every step has an oracle it did not write (the current path, and the Python oracles), a mutant that must be caught, and a falsifier stated before the code.
- The novel part, the evidence algebra, is first and formal, because it is what makes "read less" safe to do at all. The proposal puts it eighth and proves it later.
- It adds nothing structural: no crate, dependency, format or key, so the layer, purity, dependency and documentation gates keep their meaning.
- It ends with a rule for when to build more, instead of a list of frameworks.

## Open questions

- Whether the tail index should be persisted with the journal checkpoint, so a restart does not rebuild it. Rebuilding costs about a second per 64 MiB on this host; the answer is a measurement on the target profile.
- Whether `Rate` should accept a limit, so that Q3 applies to it. That is a contract change.
- Whether the one-second log poll and the 50 ms group window, which together are 95 % of the median line-to-query time, should change. Neither is a query-engine question; each is a delivery or Spindle decision with its own ADR and measurement.
