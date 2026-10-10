# Milestone: bounded sealer

Status: **defaults adopted; local acceptance and final checks completed**.
The [combined campaign](../experiments/formal/encoded-page-memory-run-01.md),
[native R2 soak](../experiments/benchmarks/soak-run-02.md) and
[unflagged default verification](../experiments/formal/bounded-writer-default-run-01.md)
record the finite acceptance evidence. CI/merge completion and deployment
qualification are not claimed. Base: `main` at `58b694d` (verification tooling
merged). The milestone replaces Segment construction without changing wire or
persisted formats or query answers ([ADR-0022](../decisions/ADR-0022-build-segments-by-external-merge-sort.md)).

Why: [soak run 01](../experiments/benchmarks/soak-run-01.md) failed its `no_rss_growth` gate. The sealer's working set, about ten times a journal file, is the analysed cause, and its recorded next action is to bound that working set and rerun the registered soak unchanged.

The design is in the [sealer view](../architecture/sealer.md). The comparison that chose it is the [sealer study](../experiments/benchmarks/sealer-study-run-01.md), which is exploratory.

## Acceptance criteria

| ID | Criterion | Deciding evidence |
| --- | --- | --- |
| BS-1 | For every registered workload, the new builder's Segment holds the same rows in all four tables, an equal manifest apart from file hashes, and the same row order as the current build. `logs.parquet` and `metrics.parquet` are byte-identical to the current build's wherever no byte cap closes a row group early | a differential test against the current `segment::build`, kept as a test-only oracle |
| BS-2 | For any rows and any run size, the merged output equals the sorted rows | a property test in the verification layer; mutant M-SEAL-MERGE-ORDER |
| BS-3 | Peak heap stays at or below the registered ceiling on every workload, and grows by at most 10 % between a 64 MiB and a 256 MiB file | a counting-allocator test; mutant M-SEAL-RETAIN-RUNS |
| BS-4 | A completed or failed build leaves no run file and no `.building-*` directory | fault tests with injected read, write and out-of-space errors; mutant M-SEAL-SPILL-LEFT |
| BS-5 | Killing the sealer at each stage, then restarting, loses no record and gives identical answers once the Segment exists | the crash-state test for HIST-2, extended to the new stages, graded by the query oracle |
| BS-6 | The same input gives the same files | two builds compared by manifest hash |
| BS-7 | The registered [soak](../experiments/benchmarks/soak-protocol.md) rerun unchanged as run 02 passes all ten gates | the soak runner's summary and exit |
| BS-8 | The change adds no crate, dependency, configuration key or port | `cargo xtask check-layers`, `cargo xtask check-core-purity`, `cargo deny --locked check`, and the `Cargo.lock` diff |
| BS-9 | The records agree | ADR-0022, the sealer view and its diagrams, retained-history, system, roadmap, current state, matrix, glossary, learning path; docs check exit 0 |

Out of scope, stated so it is not implied: renaming the `.faj` extension or any change to the frame log; pipelining the sealer across threads; sealing while a journal file is still active; a child process or another allocator; splitting `fabric-server` into an adapter crate and a thin root; and the target-host runs in the [qualification runbook](../qualification-runbook.md).

## Definition of done

BS-1 to BS-9 met with the commands and exits recorded below, the PR's CI green (Rust, Documentation, Extended verification), and the PR merged into `main`. A criterion that was not run, was interrupted or is inconclusive is recorded as such and is not met.

## Registered acceptance protocol

Registered here on 2026-10-01, before any implementation exists. It fixes the workloads, metrics and thresholds that decide BS-1, BS-3 and BS-7. The exploratory study ran earlier and chose the algorithm; its numbers informed the ceiling below, so the ceiling is a decision of this protocol and not a result.

**Workloads.** Four generated sealed journal files of 64 MiB, plus the steady shape at 16, 32, 128 and 256 MiB. The shapes are the [study's](../experiments/benchmarks/sealer-study-run-01.md#method): steady, outage (20 of 100 nodes offline for 240 s, then draining), adversarial (random times in a 520 s window) and big rows (16 KiB log bodies). The seed is `0xA11FA001`. The generator is re-implemented as a fixture in the verification layer; the study's prototype source is evidence and is not reused as product code. The big-rows shape stays in the suite as the regression for the defect the study found in its first prototype, which counted rows instead of bytes.

**Measured, per workload, three runs each in a separate process:** peak heap above the starting level from a counting allocator; wall and CPU time; bytes read, written and spilled; row groups per table; query read amplification at 60 s and 10 s windows from row-group statistics; run files and build directories left behind.

**Gates.**

| Gate | Rule |
| --- | --- |
| Equivalence | BS-1, on every workload and run |
| Ceiling | peak heap of the worst run at or below **80 MiB** on every workload. This is the sum of the design's buffer caps; the study's prototype measured 43 MiB |
| Scale | peak heap at 256 MiB within 10 % of peak heap at 64 MiB, steady workload |
| Pruning | read amplification equal to the current build's on every workload |
| Leftovers | none, on success and on every injected failure |

Time, I/O and CPU are reported, not gated: the study found no speed constraint and has no baseline against which to set a limit.

**Soak run 02.** The registered soak protocol unchanged, on the four-CPU host, with the frozen runner. Every gate of run 01 applies, including `no_rss_growth` and the 1 s ACK p99 in every window. The sealer's effect on ACK latency beside a live commit thread is **reported**, as the p99 of windows that contain a seal against those that do not, because run 01 gives no baseline for a stricter gate. The expectation that run 02 passes `no_rss_growth` is a prediction, not a result.

**A result that fails a gate is recorded as failed**, with the counterexample kept under the [counterexample rule](../../AGENTS.md#counterexample-rule). The protocol is not changed afterwards to make it pass.

## Plan

Each step is a separate commit, and a policy change never shares a commit with an implementation change.

1. Register the mutants and the check entries (`xtask/mutants.json`, `xtask/checks.json`): policy, in its own commit.
2. Add the verification fixtures: the generator, the differential test against the current build, the merge property test, the counting-allocator test and the fault tests. They fail or are skipped until step 3, so each is shown able to fail.
3. Implement the streaming builder. Keep the current `segment::build` as the test-only oracle.
4. Run the fast profile, the semantic mutants and the extended profile.
5. Run the study's workloads under the registered protocol, then soak run 02.
6. Update the records, then merge.

## Results

The [readiness continuation](../experiments/formal/readiness-continuation-results.md)
records executed commands and launcher receipts. The frozen aligned-only
campaign remains historical. The
[combined aligned-input/disk-PageStore campaign](../experiments/formal/encoded-page-memory-run-01.md)
passed all eight cells with three pairs each and supplemental row/page scratch
observations. Its steady256 heap was 39,114,918 bytes and bigrows64 was
41,405,704, with 5.88% steady64-to-steady256 scaling. The same record retains
the high-entropy failure at 143,075,502 bytes and correction at 42,011,899 bytes,
with exact files, filters and pruning. These measurements belong to the frozen
builds with both selectors enabled.

The [native R2 trial](../experiments/benchmarks/soak-run-02.md) subsequently
passed all ten gates. The [normal writer](../experiments/formal/bounded-writer-default-run-01.md)
now enables both mechanisms without flags: 27 bounded tests and 13 kill cuts
plus no-hit passed unflagged. Four release artifacts match full-file hashes;
the server's loaded content is equivalent after validated metadata normalization,
with a byte-flip control rejected. This is default verification, not a second
counting campaign. Both freezes were archived, verified and removed. All 17 final
unflagged fast checks and three manual documentation checks passed; the historical
CI/merge definition of done above remains unchanged.

| Criterion | Current evidence | Remaining scope |
| --- | --- | --- |
| BS-1 | Opt-in campaign passed exact rows, manifests and applicable Parquet byte equality in all eight registered cells and all three pairs; mixed-signal custody/query fixtures also passed. | Unflagged bounded/recovery checks and loaded-content equivalence support the adopted defaults; real application bodies and inputs beyond the registered shapes remain unmeasured. |
| BS-2 | Twenty-one bounded tests include 64 seeds × five byte limits and thirteen fan-in populations, stable ties and changed-key/payload/order controls. M-SEAL-MERGE-ORDER was caught; the opt-in campaign passed strict pruning equality. | Finite generators do not prove arbitrary rows and run sizes. |
| BS-3 | The combined aligned-input/disk-PageStore campaign passed the registered 80 MiB ceiling and 10% scaling gates in all eight cells and three pairs; the [encoded-page record](../experiments/formal/encoded-page-memory-run-01.md) retains measurements and the corrected entropy counterexample. M-SEAL-RETAIN-RUNS was caught by the steady128 ceiling. | These frozen builder measurements do not establish a whole-server or arbitrary-input bound. |
| BS-4 | Seventeen native syscall fault cases recovered exact clean manifests; ordinary-error cleanup was checked before retry. M-SEAL-SPILL-LEFT was caught. Campaign success cleanup and the supplemental spill replay cleanup were recorded. | Scoped injected errors are not physical filesystem exhaustion or every double fault. |
| BS-5 | Thirteen named SIGKILL cuts plus an unmatched-path control preserved the original journal and exact recovered custody. All five query shapes in Scan/Walk were graded by the unchanged query oracle before cleanup and after restart/reclaim; missing-row controls were rejected. All thirteen cuts plus no-hit also passed unflagged after adoption. | Representative spill/merge/table/filter/manifest/directory/rename cuts do not cover every instruction boundary or physical power loss. |
| BS-6 | All eight opt-in campaign cells passed repeated-build determinism across three pairs at frozen settings. | Different constants are not inferred; the default evidence establishes loaded-content equivalence rather than a second repeated campaign. |
| BS-7 | The [companion-compatible R2 trial](../experiments/benchmarks/soak-run-02.md) passed all ten original gates with independent companion custody/containment and matching freeze checks. | Original run 01 remains failed; R2 is the separately registered successor. The companion-inclusive trial is not a causal single-variable comparison. |
| BS-8 | Layers and core-purity passed in both the combined-selector and final unflagged seventeen-gate fast profiles; dependency-policy checking exited 0 with private cargo-deny 0.20.2. The writer uses existing crates and no new configuration key or port. Exact commands and receipts are in the [continuation](../experiments/formal/readiness-continuation-results.md). | Scope the lockfile review to this writer: the shared dirty tree also contains dependency edges for the separate companion implementation. |
| BS-9 | Architecture, matrix, learning path and current state are reconciled; all three manual documentation checks passed. | Historical CI/merge definition of done is not claimed satisfied. |

The historical aligned-only campaign's exact frozen source and binary hashes, per-cell commands,
three-pair results and reduction are retained on the mounted data drive under
`results/readiness-continuation-group-01`. The command
`python3 -B tools/bench/labs/completion/continuation_campaign.py --screen /run/media/kmosoti/data/FabricO11y/results/readiness-continuation-group-screen-03 --out /run/media/kmosoti/data/FabricO11y/results/readiness-continuation-group-01`
exited 0 under launcher unit `fabric-work-a68baf85cd96446bb77842a28c5adb67`.
Its `result.json` explicitly records `full_bs_acceptance=false`; the campaign's
gates alone do not supersede the complete protocol or definition of done above.

A separate supplemental observer replay completed all eight cells, preserving
each frozen input hash and final manifest while counting successful logical
spill writes. Its known-write control counted nine bytes in two syscalls;
steady256 counted 336,302,946 logical spill bytes in 1,312 successful syscalls.
The command
`python3 -B tools/bench/labs/completion/spill_measure.py --campaign-root /run/media/kmosoti/data/FabricO11y/results/readiness-continuation-group-01 --binary /run/media/kmosoti/data/FabricO11y/results/readiness-continuation-group-01/frozen/completion_builder --out /run/media/kmosoti/data/FabricO11y/results/readiness-continuation-group-01/spill-01`
exited 0 under unit `fabric-work-a91d936a7dee4e9eb73a151db16ccea9`.
`spill-01/cleanup.json` confirms owned scratch removal, and the launcher receipt
confirms its stopped group and temporary-directory removal, with no OOM events.
This is one observer replay per cell, **not three-pair spill/I/O measurement**;
its timing and heap are not combined with the uninstrumented comparison.

All commands above used the resource launcher. The exploratory
[study](../experiments/benchmarks/sealer-study-run-01.md#findings) remains
historical and decides no criterion by itself.
