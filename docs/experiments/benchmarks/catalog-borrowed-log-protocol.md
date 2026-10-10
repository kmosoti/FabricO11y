# CR2: scoped borrowed Parquet-log materialization

Status: **registered before execution; no outcome claimed**. [Admission evidence and design](catalog-borrowed-log-design.md).
H1: node/body ownership deferred until retention reduces selective Segment-query
cumulative requested bytes >=10% every pair without >5% CPU/peak/latency cost.
H0: validation/decode dominate or savings violate another guard.
No unsafe candidate, new parser, oracle changes, persistent format or default change.

Baseline uses existing owned scanners; compile-opt candidate uses
`FABRIC_BORROWED_LOG_EXPERIMENT=1`. All eight column checks keep their order.
Every in-window row validates16-byte identity and attributes through original
`attrs_of` before predicate rejection. Borrowed strings stay within one Arrow
batch callback; retained rows own node/body and move the validated attributes
map. Existing time/window/error/unavailable/page/top-k semantics remain fixed.
Tail already borrows; this experiment does not optimize that path.

## Frozen minimal scope

Reuse corrected Q2a native probe and unchanged full-chain oracle adapter:
65536 shuffled1024-byte rows, seed42, one Segment plus equivalent tail fixture,
both Scan/Walk, page limit10000, first plus3 warm calls. Four shape labels retain
the adapter's64calls/chains: empty uses IN-WINDOW absent text with
`BENCH_EMPTY_TEXT=1`; selective uses rare `bench-0007 `; common uses `RRRR`;
broad has no text predicate. Source/payload/timestamp hashes must match every
baseline/candidate trial. Three alternating fresh-process pairs, separately
plain/count:12 native children,768 independent complete query chains.
Continuations occur after all measured first/warm queries; retain exact pages.
Counted builds use existing allocator/phase features; plain timing decides CPU
and latency. Warm samples summarize per child without claiming they are fresh
independent repetitions. OS cache remains warm; no cache eviction.

Unit controls separately cover Unicode/escaped attributes, identity length,
wrong column types and malformed JSON on rejected in-window rows; owned and
borrowed error kind/text and row values must agree. Out-of-window bad attributes
retain their baseline skip behavior; column checks still occur before skipping.
Wrong/error rows must never reach a consumer rejection callback. Independent
query oracle and existing transcript negative controls are unchanged.
The transcript adapter adds explicit empty-text/page-limit expectations with
legacy defaults preserved, selector/limit rejection controls and a limit-based
page-count guard. This verifier extension requires its own registration commit;
the independent oracle itself remains untouched.
Measured fixtures have empty attributes and unique timestamps; tie/full-page/
attribute-rich H256 consumer combinations remain a separately registered
confirmation, not an inferred result from these controls. No SDK/Spool workload.

## Measurements and nomination

Primary population: warm selective Segment queries, separately Scan and Walk.
Each counted candidate median requested bytes <=90% its paired baseline;
every counted population's incremental peak <=105% baseline. Every plain shape/
layout/plan CPU AND wall median <=105% paired baseline. All three fresh pairs
must meet guards. Tail is a semantic/no-regression control without a10% savings
requirement. Report first and warm observations, plain/count separately; retain
inclusive phase attribution, output serialization, IO, exact full-page answers,
whole-job cgroup/RSS and cleanup. No universal throughput/memory claim.
All oracle chains, source/custody and error controls must pass; a miss blocks
nomination and remains evidence. Service Spool/ACK/query demand guards require F1.
An executed collection can exit0 with false nomination; guards are not waived.

Root registers first, freezes matched default-catalog baseline/candidate
binaries, and executes serially:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-borrowed-log-01 --lab memory --stage capacity --seconds 1500 -- python3 -B tools/bench/labs/catalog/borrowed_log.py --out docs/experiments/benchmarks/data/catalog-borrowed-log-run-01
```

Driver unsets shared-catalog/spill experiment flags, checks native borrowed mode
acknowledgement, archives frozen binaries, source/protocol/oracle hashes, commands,
raw ledger objects and exact chain maps. Resource/budget guards reuse corrected
profile adapter: scratch8GiB, evidence256MiB, drive reserve16GiB,20GiB/no swap,
1400s child preparation/collection deadline inside1500s coordinator. No workload
when stage/shared budget cannot admit it. Successful owned scratch is removed;
failure state is archived/retained. Environment limitations remain explicit.
