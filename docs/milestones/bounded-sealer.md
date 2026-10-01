# Milestone: bounded sealer

Status: **design accepted; implementation not started** ([ADR-0022](../decisions/ADR-0022-build-segments-by-external-merge-sort.md)). Base: `main` at `58b694d` (verification tooling merged). This milestone replaces how the sealer builds a Segment, so that its memory no longer grows with the journal file. It changes no wire or persisted format and no query answer, and it is not qualification.

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

None yet. The exploratory study's findings are in its [record](../experiments/benchmarks/sealer-study-run-01.md#findings) and decide no criterion above.
