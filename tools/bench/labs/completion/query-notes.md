# Query/availability completion preparation

Read-only preparation, 2026-10-05. No project workload, build, test, validator or
production edit ran. This note is the only owned change. Evidence inspected:
CURRENT, PRODUCT-CONTRACT, retained-history view, ADR-0024, HIST-1..9/TRACE-1,
experiment guidance/skill, dev-small plan, screen run02 and query-plan run01.
Historical passes remain bound to their historical revisions.

## Q2a: preserve first-page measurement; complete correctness separately

Native three-pair CPU/latency comparison already completed; repeating that entire
campaign does not repair its separate inconclusive allocation branch. Native live
timestamps/host metrics differ; identical recovered input applies to the private
allocation probe, not those native pairs. Spans remain a separate R2a dependency.

Exact mismatch: `responsibility_probe.rs:985-1022` creates four log queries with
limit10000, measures one `History::run`, repeats three warm calls, and retains each
actual first page. `run_responsibility_isolation.py:159` supplies `[answer]` to
`query_oracle.check`. Oracle `query_oracle.py:1085,1093` demands null final token
and all admitted rows. Tiny128 passes; full65536 broad/common cannot. The preserved
failure is `data/query-plan-run-01/memory/retry-02/full-failure/counterexample.json`.
This is a harness mismatch, not evidence of product row loss.

Smallest honest repair for the same first/warm metric: finish all four measured
first calls for each shape before any continuation work, then follow each retained
first answer's token with the original query and fixed newest group. Emit complete
chains outside measured scopes; retain every measured first-page byte unchanged.
Use a new private allocation collector accepting chain arrays and calling the
unchanged oracle once per chain. Do not edit the old grader/oracle or increase the
query limit/population: that would change the measured operation. Bound page count,
require terminal null, stable snapshot, and bind each chain to its measured first
answer. Continuations must not warm the next measured iteration. Deduplicate exact
page bytes plus wrapper/chain maps after byte equality; write pages incrementally
to avoid keeping four complete 64MiB answers live or crossing existing 256MiB
per-shape ledger/4GiB scratch limits. Reset/segregate allocator and phase population
around continuation collection; it must not enter first/warm summaries.

Register this as a new protocol before execution. Negative controls: preserve
original first-only failure, remove/duplicate/change a continuation row, truncate
chain, change snapshot/token association, drift fixture bytes. Grade tiny and full
plain before admitting full counted. Compare fixture identities, exact Batch
bytes/framing (except documented tail `received_ns`), and within-layout recovered
ledgers across plans.

Instrumentation defect: `query.rs:315-319` nests both source-loading labels inside
`sources_walk`; `sources` at523 lacks Scan's label. Move only the optional Scan
span into `sources`, keeping Walk's span in `sources_walk`. A counted tiny fixture
must show one correct loader label per query, never nested opposite-plan labels.
Nested global allocation snapshots remain inclusive, not additive ownership.

## Q2b: existing fixtures and missing cells

`crates/fabric-server/tests/history.rs` supplies executable starting points:
`retention_removes_old_segments_and_stream_state_survives_it` (byte eviction,
Gone token, retained boundary, restart sequence); `a_corrupt_segment_makes_the_answer_incomplete`
(truncated logs Parquet, both plans); `text_filters_skip_only_groups_without_the_needle_and_fall_back_when_corrupt`
(corrupt log filter fallback and forged-digest negative control);
`sealed_history_answers_exactly_and_pages_are_stable` (oracle chains and wrong-query
400); `walk_and_scan_plans_answer_identically` (full drain + independent grading);
`a_walking_server_keeps_its_tail_index_exact_while_sealing` (growing index).

Missing combined fixture coverage: age-driven eviction with controlled receipt
times; retained-ledger independent grading after eviction; concurrent seal/retention
and old-token continuation; quiet-source freshness versus collection gaps; deleted
raw table versus truncated table; missing/corrupt optional log **and span** filters;
multi-page spans/rates across these boundaries; restart-derived-cache rebuild.
Implement small diagnostic fixtures using existing test helpers, both plans, and
retained Batch ledger independent of current Segment enumeration. Feed unavailable
evidence explicitly to the oracle. Avoid treating legitimate eviction as custody
loss. Snapshot expiry here means retained-floor advancement, not a nonexistent TTL.
Missing manifest/directory differs from missing raw table; record each separately.

## Q1 remaining evidence

Screen run02 reports one near-rotation seal but explicitly cannot establish build
overlap (`dev-small-labs-run-02.md:131-135`). Seed sentinel chains in
`dev_small/measurement.py:172-194` are complete only for their selective tags,
not a complete seeded-history drain. Add quiescent full snapshot drains containing
oldest/newest seed and live rows; grade against partitioned seed/live ledgers.
Capture actual builder start/end boundaries against request boundaries. The
private `responsibility_probe::pending_trial` is a useful barrier starting point,
but its launch barrier alone does not prove requests overlap the builder. An
explicit publication barrier proves transition exactness only, not freshness.
Fresh development's natural first seal remains a separately budgeted lifecycle.

Coordinator-only commands after registration/repair:

```text
python3 tools/resource_group.py -- cargo test --offline --locked -p fabric-server --test history -- --test-threads=1
python3 tools/resource_group.py -- python3 tools/bench/labs/query_compare/profile.py --out docs/experiments/benchmarks/data/NEW-RUN/memory
python3 tools/resource_group.py -- cargo xtask checks --profile fast
```

New collectors/fixtures need their own concrete commands; none yet exists.
