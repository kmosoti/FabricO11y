# CQ1: storage read boundary preservation; CR3 shared-metadata diagnostic

Status: **registered before execution; comparison not run**. Root registers this protocol separately
before timing. [Planning packet](catalog-labs-query.md), [queue](catalog-lab-queue.json)
and [historical precursor](catalog-metadata-findings.md) apply. Operations' typed
handle/coverage cut results are a prerequisite for acceptance, not a replacement
for full independent answer grading. No default, oracle or durable format changes.

## Changes and compared revisions

The candidate extracts Walk source discovery/index/cache ownership into internal
`read_catalog.rs`. Query/core retain logical predicates, ordering, threshold,
page validation, rate semantics and envelopes. Clone metadata remains the default;
hidden `History::with_shared_catalog` acquires Arc metadata handles instead.
Handles pin object memory, never raw files or retention rights. Existing source
movement retries, optional-index fallback and live snapshot-floor checks remain.
`refresh_catalog` is explicit synchronous derived maintenance; CQ1 never calls it.

Compare fresh pre-extraction binaries frozen by root, rather than the historical
completion binary: [plain provenance](data/catalog-preboundary-build-01/receipt.json)
and [counted provenance](data/catalog-preboundary-build-02/receipt.json).
Decoded hashes are respectively
`7500edaeae90f7e4bf00875af0f1f5f059d096a8184b936551b4a7d4e44b672c` and
`321ec3ac1048c1b2a8f63bb8baef68e9c6347eb7b0a69e16251a9c535637acf3`.
Coordinator source archives capture the original tree. The driver refuses a
different decoded baseline. Candidate source/protocol/binary hashes are frozen
before timing. Extracted clone and shared variants use the same candidate binary,
with `BENCH_SHARED_CATALOG=0/1`, acknowledged in its native fixture receipt.

An executable between-listings publication/reclaim cut subsequently reproduced
an inherited empty-source view despite intact durable Segment custody. Candidate
acquisition now rechecks all published labels after tail discovery and returns
Interrupted on a changed set, using History's existing bounded retry. An analogous
Scan cut also reproduced empty acquisition despite intact custody; Scan and
explicit refresh now use the same label-set validation. The regressions include warm/cold catalogs and clone/
shared ownership. This correction is part of the compared candidate: the CQ1
overhead estimate combines ownership extraction and coverage validation, rather
than isolating pure module movement. [Failure evidence](catalog-discovery-race-findings.md)
remains preserved and does not become a retrospective pass.
Stable named directories with missing manifests now fail bounded discovery
rather than silently claiming an absent source; this changes error visibility,
and establishes no broader raw-corruption completeness guarantee.

H1-CQ1: the boundary plus discovery correction preserves behavior with at most 5% overhead.
H0: it changes answers, errors or lifecycle behavior, or exceeds that guard.
H1-CR3: real shared metadata reduces cloning/lock cost; H0: acquisition is too
small to matter or a retained-handle trade-off outweighs savings. This first
real-query fixture has one Segment: it screens CR3 but cannot qualify its scaling
or paused-reader reclamation claim. Those require a later declared many-Segment
and lifecycle fixture. The precursor is not substituted for that missing result.

## Frozen fixture and exactness

Reuse corrected completion fixture/seed42: 65536 logs, requested1024-byte bodies,
128 logs/Batch, existing node/sequence/index and timestamp shuffle. Preserve
the legacy four shapes, limit10000, tail and one-Segment layouts, Scan/Walk,
first call plus three warm calls, minimal observer and rotation0. Baseline and
candidate have identical source/timestamp-rank/Batch hashes. Receive stamps vary
through real Intake; the existing ledger control permits only that difference.
No eviction, concurrent ingress, proactive maintenance or OS-cold claim.

Six128-row preflights cover three mechanisms × plain/counted. Full plain trials
run three fresh alternating triads: baseline/clone/shared, shared/clone/baseline,
baseline/clone/shared. One full counted diagnostic triad is collected in a separate
diagnostic job alongside the six preflights. Total18 trials,
64 actual first-page-associated chains per trial. All first-page measurements
finish before continuations. Grading/archival occurs outside native timed spans.

Use unchanged completion `profile.collect`, independent query oracle and complete
retained chains. Preserve exact measured first-page bytes, snapshots/envelopes,
changed/missing/duplicate first/continuation rows, truncated/first-page-only
chains, changed snapshot, ledger and lossless archive controls. Require one
matching source-loader per measured counted query. Do not replace an oracle with
Scan/Walk agreement or remove a chain to fit budget. Operations separately cover
mixed metrics/rates/spans, simultaneous sources, publication/reclaim/retention
and dishonest numeric/filter bounds; CQ1's log-only matrix adds no broader claim.

## Estimands and decision

Primary: each trial's median of three plain broad-tail Walk warm first-page
`History::run` wall and process CPU measurements. Preserve per-shape/layout,
first/warm and separate serialization observations. Use measured pre-continuation
VmHWM as a whole-native-process guard, never as exclusive catalog memory.
CQ1 passes its performance guard only when extracted-clone/pre-boundary ratios
are at most1.05 for wall, CPU and measured HWM in every matched pair, with zero
oracle/cut failures. Exactness and command completion are distinct from this guard.
If noise or changed ownership fails it, retain the result; do not broaden thresholds.

CR3 shared/clone ratios are reported separately, without promoting a default.
Counted diagnostics preserve cumulative/requested/live/peak allocation and nested
phases; new lock wait/hold spans separate waiting from work under the mutex.
Do not sum inclusive phases or use counted timings as plain performance evidence.
Snapshot expiration and post-cancel memory costs stay explicit dependencies.

## Executable command and resources

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-boundary-diagnostic-01 --lab query --stage query --seconds 1200 -- python3 -B tools/bench/labs/catalog/query_boundary.py --slice diagnostic --out docs/experiments/benchmarks/data/catalog-boundary-diagnostic-01 --baseline-plain docs/experiments/benchmarks/data/catalog-preboundary-build-01/plain.gz --baseline-counted docs/experiments/benchmarks/data/catalog-preboundary-build-02/counted.gz --run-seconds 900
```

Then run each plain triad independently, substituting pair2 and pair3 for pair1
in both ID/output and selector. Preserve the declared alternating order:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-boundary-pair1-01 --lab query --stage query --seconds 1000 -- python3 -B tools/bench/labs/catalog/query_boundary.py --slice pair1 --reuse-build docs/experiments/benchmarks/data/catalog-boundary-diagnostic-01 --out docs/experiments/benchmarks/data/catalog-boundary-pair1-01 --baseline-plain docs/experiments/benchmarks/data/catalog-preboundary-build-01/plain.gz --baseline-counted docs/experiments/benchmarks/data/catalog-preboundary-build-02/counted.gz --run-seconds 900
```

The diagnostic driver builds/freeze-copies current `completion_query_probe` plain, then with
`responsibility-alloc-probe,phase-probe`, within300 build seconds. Optional actual
`--plain`/`--counted` paths reuse already frozen current binaries instead. Native
commands remain `FROZEN_BINARY OWNED_TRIAL query 1024` with the frozen environment;
legacy binaries ignore only the new metadata selector. Later jobs extract the
diagnostic job's archived candidate binaries, verify their decoded hashes and
retain that original build's provenance; later source edits cannot alter them.
Consolidation must require all18 trials, identical candidate/baseline binary
hashes and fixture identities across jobs, and all three plain paired guards.
No slice alone completes CQ1. The split changes job admission only, preserving
all shapes, chains, repeats and thresholds; it avoids a single long collection
exceeding the launcher's deadline. No invented plan/limit flags.

All descendants use existing16/20GiB memory high/max, no swap and the launcher
30-minute deadline; coordinator1200/1000 seconds includes build/grading/cleanup.
Require mounted-drive scratch≤8GiB, evidence≤256MiB and16GiB free reserve.
Stop on provenance/fixture/oracle/phase mismatch, OOM, deadline or resource failure.
Archive failures before cleanup; success removes only owned fixtures/binaries.
Retain commands/exits, raw compressed phases/timings, exact answer maps,
source/binary/oracle hashes, resource observations, cleanup and comparison ratios.
Deadline/missing pairs mean incomplete evidence. CQ2/Q3/remaining CR3 stay pending.
