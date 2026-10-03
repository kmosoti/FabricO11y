# Journal reclaim: local mechanism protocol

Status: **registered before the candidate implementation and experiment**. This is a bounded correctness/progress investigation for the [journal-reclaim milestone](../../milestones/journal-reclaim-progress.md), not a throughput benchmark, release, or target-host qualification. The separate mixed-load comparison in that milestone remains a draft.

## Question and fixed boundary

Does reclaiming a contiguous published prefix before building and between worker groups return journal capacity while a later build is still blocked, without crossing an unpublished label or continuing after a reclaim error?

Baseline: `5f0c52f16f8afe01cf29a118513cf8a43b4cb333`, which performs reclaim after its captured pending build set. Candidate: the revision recorded in the result, changing only the scheduling of the existing `Intake::reclaim` operation. Retain the same builder, workers, publication, checkpoint and deletion order, codecs, queries and retention policy. Direct native processes only; no Docker or Wasm.

## Deterministic workloads and decision rule

The scheduler fixtures use ordered labels 1 through 4, one or two workers, prepublished subsets, successful builds, a failed or panicking build, and a reclaim error. A later worker group is held by a bounded synchronization barrier. Observe the reclaimed prefix before releasing it; elapsed sleep time is not the assertion. The test must always release the barrier before asserting, including on a watchdog timeout.

Required checks:

1. A prepublished oldest prefix is reclaimed before any new build.
2. A completed first worker group is reclaimed while a later group is blocked.
3. A failed earlier build prevents reclaim of a later successful build; an earlier successful prefix still advances.
4. A reclaim error prevents every later reclaim and prevents launching another worker group.
5. Retry uses surviving published labels and does not rebuild them.
6. A worker panic behaves like a build error, preserving the same prefix boundary.
7. A small native journal/Segment fixture verifies successful reclaim, stream checkpoint/reopen and exact retained Batch bytes through the real store. A forced checkpoint-write error leaves the corresponding journal file present. Existing history and delivery tests exercise query/paging/deduplication and crash-state behavior.

The mechanism succeeds only if every applicable assertion passes. Run a representative negative control by deferring the candidate's reclaim to the end of the pass: the early-progress test must fail. Run a second control that lets reclaim cross an unpublished/failed prefix: the safety test must fail. Restore and verify the exact unmutated source after each control. Record commands, exit status and the injected diff; a surviving control invalidates the associated check. No independent performance benefit is inferred from the mechanism tests.

## Execution and resource limits

Install Rust 1.98.0 with rustfmt and clippy outside the repository. Use two build jobs and disable debug information for local test builds, consistently for baseline and candidate. Keep build/cache directories separate from disposable data; record their disk footprint and stop if workspace free space falls below 4 GiB. These build flags are not release-performance settings.

Run the baseline server tests before changes. Candidate commands from the repository root, with the toolchain environment configured:

```sh
cargo test --locked -p fabric-server --lib sealer::tests
cargo test --locked -p fabric-server --lib --tests
cargo xtask checks --profile fast
```

Run the two targeted negative controls against the affected scheduler tests only. Cap each test invocation at 15 minutes externally; the synchronization fixtures also use short bounded watchdogs. Each owned journal fixture is below 32 MiB; no random seeds, traffic generation, service installation, privileged faults or whole-disk exhaustion are needed. Preserve compact test output and diffs, not bulk telemetry. Do not change the fast-check registry or weaken unrelated baseline failures.

The full fast profile may expose preexisting lint or environment failures. Report each real result; establish whether it exists on the frozen baseline rather than labeling it a candidate regression or claiming all checks passed.

## What transfers to the PC

The scheduling assertions and their counterexamples are independent of disk speed. Re-run them as smoke checks on the PC if desired. ACK p99, useful throughput, storage-pressure behavior, long-run RSS and real-filesystem durability require the separately frozen mixed-load/target-host experiments. The local result must not claim those properties. The [milestone's work split](../../milestones/journal-reclaim-progress.md#current-environment-and-pc-work-split) defines the Docker-free handoff.

## Results

Not run at registration. Record the baseline/candidate revisions, all command exits, controls, environment and remaining work in a linked run record after execution.
