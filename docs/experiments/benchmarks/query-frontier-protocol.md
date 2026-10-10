# Query frontier Q3: source bounds and output cost

Status: **draft preparation; not executed or registered**. This protocol requires
the harness adaptations below and a separate registration commit before timing.
It changes no product contract, oracle, production default or historical result.
[Query packet](performance-frontier-labs/query.md),
[shared admission/resource rules](performance-frontier-lab-plan.md) and the
[historical allocation result](lab-completion-run-01.md) apply. Q4 concurrency,
restart, ingestion interference and default selection are outside Q3.

## Question and causal boundaries

H1a: overlapping timestamp bounds cause Walk to traverse most tail blocks even
with a small page. H1b: constructing returned JSON values and serializing them
accounts for much of the remaining broad-query cost. H0: neither intervention
materially explains the Scan/Walk latency difference. These are competing,
potentially simultaneous explanations; neither is assumed true.

[Source selection and threshold loops](../../../crates/fabric-server/src/query.rs),
[whole-block decompression/iteration](../../../crates/fabric-server/src/tail.rs),
and [native first-page/serialization measurement](../../../crates/fabric-server/examples/completion_query_probe.rs)
define the mechanisms. The current execution span includes JSON row construction;
its total cannot establish exclusive construction cost. Envelope construction and
external serialization are different boundaries. Nested inclusive allocation
snapshots are not additive ownership measurements.

## Frozen fixture and matrix

Use the existing native deterministic fixture: 65536 logs, 1024-byte requested
body size, 128 logs per Batch, existing node/sequence/index identities, generation
and attributes, START = 1600000000000000000 ns; seed 42 and the existing xorshift
shuffle. Preserve encoded custody bytes within each cell/pair. No ingestion,
retention, sealing or client concurrency runs during measurement.

| Factor | Frozen values |
| --- | --- |
| Timestamp ranks | `sorted`, `shuffled` |
| First-page limit | 100, 10000 |
| Layout | existing tail and Segment fixtures |
| Plan | Scan baseline, Walk comparison |
| Shapes | empty, selective, common, broad, unchanged strings/windows |
| Measured calls per shape | first call plus three warm calls |
| Observer | minimal; shape rotation 0; CPU affinity identical across trials |

The selective needle remains `bench-0007 `; common remains `RRRR`. Empty uses
[1,2); other shapes use [START,START+65536). Limit varies for every shape.
The existing generator assigns timestamp ranks to fixed body identities: the
order intervention therefore changes timestamp-to-body association. It is a
declared timestamp-distribution experiment, not a permutation of identical
semantic rows. Source bodies/identities remain fixed; independently grade each
rank assignment. Do not require cross-order encoded-Batch equality. Require
plain/counting and Scan/Walk fixture identity within each order/limit cell.

Enumerate cells in this order: shuffled/10000, sorted/10000, shuffled/100,
sorted/100. Each plan runs in its own fresh process, with fresh owned fixture and
History state. Pair 1 is Scan then Walk, pair 2 Walk then Scan, pair 3 Scan then
Walk. Complete pair 1 for all four cells before additional pairs. OS cache is
buffered/warm from fixture preparation; no cold-cache claim. Preserve first versus
warm populations separately. Complete continuations only after all measured
first pages in that process; their work is outside performance timing.

## Estimands and decision rule

Primary timing is plain-build `History::run` wall time for broad tail warm first
pages: the median of three warm calls per trial. Report Walk/Scan ratios within
each pair, by cell, and the order and limit interactions in those ratios. Keep
first-call startup, all other shapes/layouts and separately measured
`serde_json::to_vec` wall/CPU as individual secondary results. Do not choose a
different winning aggregate afterwards. Small samples do not support p99 claims.
Record CPU/call, returned rows/bytes, measured HWM and whole-job resource costs.

In separate counted runs record visited tail blocks/entries and Segment groups,
decompressed bytes, observations visited, qualifying candidates, rows materialized
for the bounded heap and returned rows. Counters must distinguish candidates from
retained rows and decompressed blocks from matching entries. Add exclusive
diagnostic spans around selection/heap work and row-to-JSON conversion; preserve
source-loading, inclusive execution, envelope and serialization boundaries.
Instrumentation must be feature-gated and absent from the plain binary.

A reproducible timing effect requires at least 10% lower primary wall cost in
each of three matched pairs, with no more than 5% regression in broad-tail CPU
or measured HWM. This is a discriminator, not authorization to switch defaults.
H1a is supported only when the traversal counters move in the predicted
direction with the timing interaction. H1b requires measured construction or
serialization attribution to track returned bytes; source work must be reported
alongside it. A correlation without phase separation leaves H1b unresolved.
Retain every null, regression and mixed result. No metric or threshold changes
after measurement begins.

Counterexample: limit 100 visits nearly every shuffled block but few sorted
blocks, rejecting the universal claim that small pages make Walk cheap. The
opposite diagnostic is constant traversal with output cost scaling with returned
bytes. If neither explanation survives three pairs, stop and review the phase
evidence before proposing implementation work.

## Exactness, evidence and controls

Reuse [completion profiling](../../../tools/bench/labs/completion/profile.py)
and its unchanged `query_oracle.check`. Grade every actual measured first page
with its own complete associated chain, including envelope, ordering, snapshot,
freshness, gaps and pagination. Preserve measured first-page bytes exactly;
continuation output must begin with those bytes. Scan/Walk agreement alone is
insufficient. Keep independent recovered custody ledgers and exact decoded-body
grading. Do not replace complete-chain grading with a first-page checksum or
sample, and do not omit chains to meet the time budget.

Retain existing changed/missing/duplicate first-row and continuation controls,
first-page-only/truncated-chain rejection, changed snapshot, exact-byte
association, phase-loader ownership and lossless archive reconstruction controls.
Adapt expected cardinalities to the frozen single-plan cell, never inferred
from observed output. Add defects proving rejection of a changed declared limit,
wrong plan loader, omitted shape and missing final continuation. Independent
oracle code and expected outcomes remain unchanged. Existing optional-filter
fallback/corruption tests remain prerequisites; Q3 adds no fault workload and
makes no new fallback claim.

Each trial retains command/environment, exit, binary/source/oracle hashes,
working-tree snapshot and protocol copy; fixture hashes, exact answer/chain
maps, controls and archive verification; raw timings/phases; resource/cleanup
receipts. A run receipt separates native runtime, grading, archival and cleanup.
Keep measured HWM separate from final HWM after correctness drains. Preserve
failures before cleanup. Root links results into current state after review.

## Admission, execution budget and commands

Root serializes all execution through `python3 -B tools/resource_group.py --`.
The complete Q3 allocation is at most 3600 wall seconds including builds,
controls, preflights, grading, archival and validators. Reserve up to 300 seconds
for builds, 120 controls, 300 preflight, 2400 plain trials and 480 attribution.
No individual job may exceed the launcher's 1800-second deadline. Record elapsed
time against this aggregate allocation; unused reserves may transfer within it.
Preserve the mounted drive, 16 GiB free reserve, 8 GiB owned scratch ceiling and
256 MiB retained evidence ceiling. Existing resource enforcement fails closed.

Preflight the frozen matrix at 128 rows, then admit the 65536-row cells. Obtain
three fresh alternating pairs only where the aggregate budget allows. A missing
pair is an incomplete screen, never confirmation. Limit 100 can require 656
pages per broad chain; repeated Scan drains may consume the budget. Do not lower
population, repetitions, exactness coverage or page-count guards after timing.
When a deadline/space limit interrupts a trial, retain partial evidence, mark it
interrupted and stop expansion. Revisit scope through a new protocol revision.

Existing executable controls command, after root admission:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/profile.py --controls
```

Current profile CLI exposes only `--controls`, `--out`, `--build-seconds` and
`--run-seconds`; it does not implement this matrix. Current native invocation is
`completion_query_probe OWNED_SCRATCH query 1024`, with `BENCH_RECORDS`,
`BENCH_BODY_SIZE`, `BENCH_ORDER`, `BENCH_QUERY_REPEATS`, `BENCH_OBSERVER`,
`BENCH_PREFLIGHT`, `BENCH_QUERY_ROTATION` and `BENCH_PHASES`. The profile runner
currently overwrites these with its old fixed fixture. No purported Q3 run
command is executable until the following adaptations are implemented and
registered. Root records final exact commands before any Q3 timing.

## Minimal harness prerequisites

1. Add an explicit bounded page-limit and single-plan selector to the native
   probe; retain legacy defaults. Derive native continuation termination from
   the declared limit (currently hardcoded `count / 10000 + 2`). Record selectors
   in the fixture receipt. Keep four shapes and both layouts.
2. Extend the existing profile runner to accept the frozen selectors, population,
   repetitions and variant; reuse frozen plain/counted binaries across trials
   without rebuilding each pair. Root owns deadline/accounting and binary
   snapshots. Keep the historical invocation unchanged.
3. Parameterize only transcript-adapter expectations: expected filenames/calls,
   frozen request limit, page-count ceiling and loader labels. Today these assume
   16 files, 64 calls and limit 10000. Archive-verification cardinalities also
   require adjustment. Keep independent oracle implementation unchanged. Such
   verification-harness changes need their own reasoned registration commit.
4. Add feature-gated work counters and separate JSON-construction attribution,
   with defined count ownership and checks for representative counting/label
   defects. Plain timing must contain no diagnostic observer. Register concrete
   instrumentation and new checker controls before diagnostic measurement.

These are preparation gaps, not implemented capabilities or successful checks.
No Q3 execution, validator or registration commit ran while drafting this file.
