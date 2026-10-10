# CR3: many-Segment real-query metadata screen

Status: draft; not registered, built or executed during preparation.
H1: shared immutable catalog metadata reduces absent-query cumulative requested
bytes >=10% in every64-Segment counted pair at fixed completed demand, with
<=5% CPU, elapsed, p95 latency and incremental allocation-peak cost.
H0: decoding/discovery dominates, clone cost is negligible, or sharing violates
these guards. One-Segment layout and broad queries are dominated-cost controls.
No default, unsafe candidate, persistent format, ACK or durability changes.

## Frozen fixture and work

Separate `catalog_many_segment_probe` uses real `Store`, `segment::build` and
Walk `History.run`. Runtime clone/default versus `with_shared_catalog` shares
one identical binary per plain/count population. Compile borrowed-log mode0;
no spill experiment. No query.rs/read_catalog.rs edits for this probe.
Exactly65536 shuffled1024-byte log bodies, seed42 (same deterministic body/rank
helpers as corrected completion probe);512 real groups,128 rows each. One
layout places512 groups in one Segment; the other has64 Segments of8 groups.
Exact raw Batch ledger hashes must match all variants/layouts/reader counts.
Empty journal; no Intake/SDK/Spool workload and no custody/throughput claim.

Queries cover all timestamps. Absent text is `__CR3_ABSENT__`; broad contains
no predicate. BOTH have limit1000, honestly a medium page (not a20-row page).
One warm-up per shape establishes derived caches outside timing. Then1 or4
bounded OS reader threads share Arc<History>, issue32 completed first-page
queries per shape total (32 or8 each), and join before results are inspected.
No concurrent publication/retention in this screen; CQ2 studies maintenance.
Every query includes real source discovery, file reads, filtering/page logic
and answer serialization; timed batch includes barrier release/join overhead.
Per-call latency covers History.run+serialization, excluding thread creation.
Fixed queries/demand make elapsed per32 completions comparable across variants.

After each timed shape, a complete canonical chain is independently graded
against original raw Batches using the unchanged Python query oracle: absent1
page, broad66pages. Every actual timed first-page byte string must exactly equal
that graded canonical first page; retain all32 lossless associations and page
objects, query, record objects/hashes and snapshot. This is one complete drain
per immutable identical query/snapshot, not32 independently executed drains.
Across48 children:96 complete canonical drains and3072 timed first pages.
Truncated-chain and duplicated-row controls must be rejected. Chain validation
is outside timed loops; different limits/snapshots cannot be associated.

## Resource limits and decisions

Full matrix:1/64Segments x1/4 readers x plain/count x3 fresh alternating pairs,
48 fresh native children,24 separately contained pair jobs. Pair1/3 clone first;
pair2 shared first. OS filesystem cache stays warm; no cache eviction.
Plain process CPU uses Linux process clock including all readers; report batch
wall, per-call p95 (32 samples), proc IO before/after. Counted uses existing
benchmark-only System-delegating allocator instrumentation, requested bytes,
call count, live baseline/final and incremental peak. Instrumentation unsafe
only delegates System/CPU-clock ABI; candidate production paths remain safe.
Plain allocation fields stay null. Counted timing never substitutes for plain.

Primary: absent requested bytes <=90% paired clone at64Segments for BOTH reader
counts, all3 counted pairs. Guard every layout/shape/count: allocation peak
<=105%; every plain population CPU, wall and p95 <=105%. All full-chain checks,
exact fixture/output association and real lifetime controls must succeed.
All24 cells required to nominate; no pooled win or allocation-to-speed inference.
Failure is retained evidence; incomplete campaign has no nomination.

The separate lifetime receipt `catalog-lifetime-01` already executed two real
Sources retention/cancellation controls (exit0;59.1MiB whole-job peak): Arc
identity accounting, held versions, actual file retention and weak reclamation.
Those establish metadata lifetimes, not file leases or universal memory bounds;
expired-token behavior belongs to independent operations/History Gone checks.
This screen's peak includes retained answer buffers; distinguish owned allocated
bytes from shared metadata retained by Arc. No version churn is inferred here.

512MiB reviewed capacity allocation includes all CR1/CR2/CR3 evidence;2GiB
aggregate catalog allocation,8GiB scratch,16GiB free reserve, original cgroup
16/20GiB/no swap,540s driver within600s coordinator and remaining shared budget.
Root admits64Segment/1reader/plain pair1 first to measure runtime, then counted
pair1 and remaining exact cells only if projected full matrix fits budget.
If it cannot fit, preserve the stated incomplete screen without changing gates.

Freeze once, after root registers this draft:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-many-segment-freeze-01 --lab memory --stage capacity --seconds 600 -- python3 -B tools/bench/labs/catalog/many_segment.py --stage freeze --out docs/experiments/benchmarks/data/catalog-many-segment-run-01/freeze
```

Run each exact SEGMENTS(1/64),READERS(1/4),BUILD(plain/counted),PAIR(1/2/3):

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-many-S-SEGMENTS-R-READERS-BUILD-p-PAIR --lab memory --stage capacity --seconds 600 -- python3 -B tools/bench/labs/catalog/many_segment.py --stage pair --freeze docs/experiments/benchmarks/data/catalog-many-segment-run-01/freeze --segments SEGMENTS --readers READERS --build BUILD --pair PAIR --out docs/experiments/benchmarks/data/catalog-many-segment-run-01/S-SEGMENTS-R-READERS-BUILD-p-PAIR
```

Aggregate only after all24 cells: same contained coordinator,60s,
`many_segment.py --stage aggregate --freeze RUN/freeze --out RUN/summary
--slices` followed by the24 explicit sibling pair directories. It verifies raw
metrics, full-drain receipts, exact fixture hashes, cleanup and unchanged guards.
Aggregate counts complete-chain and actual association entries from artifacts,
requires exact first-page object/hash equality, validates all24 unique cells and
injects missing/duplicate cell, changed guard/binary/association controls.
Successful owned scratch is removed; failures preserve receipts/scratch.
