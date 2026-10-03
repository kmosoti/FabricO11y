# Milestone: journal reclaim progress

Status: **candidate implemented; local mechanism checks passed**. Branch: `milestone/journal-reclaim-progress`, based on `5f0c52f16f8afe01cf29a118513cf8a43b4cb333`. The [local run](../experiments/benchmarks/journal-reclaim-local-run-01.md) records bounded correctness/progress tests and their negative controls. The mixed-load comparison below remains a draft and must be registered with its executable harness before measurement. No throughput, latency or target-host qualification result is claimed.

## Finding: three kinds of progress

FabricO11y preserves evidence, builds representations for interpreting it, and returns capacity for new evidence. These advance separately:

| Representation | Progress event | What it establishes |
| --- | --- | --- |
| Custody | Journal data and commit marker synced; ACK becomes eligible | The server has accepted responsibility for the Batch under the filesystem assumptions |
| Query sources | A Segment is durably published | Queries can use its projection and exclude its duplicate journal coverage |
| Capacity | Stream state checkpointed, then the oldest covered journal file removed | Journal capacity can be reused without forgetting deduplication state |

This is an analytical model, not a new protocol or persisted state. A single scalar watermark cannot describe it: parallel builders may publish noncontiguous labels, while reclaim must advance through the oldest contiguous eligible prefix. Observation time is another ordering entirely; advancing reclaim does not establish freshness.

### Evidence at the base revision

- [`sealer::pass`](../../crates/fabric-server/src/sealer.rs) snapshots sealed labels, builds pending files in worker-sized groups, waits for the captured build set (or a failed group), and only then calls `Intake::reclaim` in label order. Already published files are also left until this build phase ends.
- [`Intake::reclaim` and `Store::reclaim`](../../crates/fabric-server/src/store.rs) route reclamation through the commit thread, require the oldest sealed file, save `streams.json`, then remove that file. The checkpoint is synchronous work shared with ingest.
- [`History::sources` and `sources_walk`](../../crates/fabric-server/src/query.rs) exclude journal coverage already represented by Segments. Deleting a duplicate journal file is therefore not itself a query acceleration mechanism.
- [`crash_states_of_sealing_never_serve_a_record_twice`](../../crates/fabric-server/tests/history.rs) already exercises a published Segment coexisting with its journal file, both query plans, and restart cleanup. It does not establish prompt reclaim while later builds remain pending.
- [Ingest run 01](../experiments/benchmarks/ingest-run-01.md) records journal-full refusals and subsequent drain with one to three sealing workers. It does not isolate how much is caused by scheduling versus sealing throughput.

The [custody and completeness contract](../PRODUCT-CONTRACT.md), [Segment lifecycle](../decisions/ADR-0020-store-sealed-history-as-parquet-segments.md), and [retained-history view](../architecture/retained-history.md) remain authoritative.

## Hypothesis and counter-hypotheses

**H1:** reclaiming the oldest contiguous published prefix before new builds and after each completed worker group reduces the time durable duplicate journal bytes occupy capacity. Under journal pressure, this may reduce refusal time and delivery backlog without changing evidence, query answers, or the durable ordering inside publication and reclaim.

The mechanism claim and the performance claim are distinct. The baseline can fail a deterministic progress assertion without violating the existing custody contract. No performance improvement follows merely from that failure.

| Perspective | Prediction to test | Counter-hypothesis or cost |
| --- | --- | --- |
| Queueing | Earlier reusable capacity reduces journal-full intervals | Sealing CPU dominates, so shifting deletion times changes little |
| Durability | The existing checkpoint-before-delete operation is sufficient | New interleavings expose a recovery, retry, or checkpoint failure bug |
| Commit scheduling | Earlier reclaim avoids large end-of-pass reclaim bursts | Interleaved checkpoint syncs delay submissions and worsen ACK p99 |
| Query interpretation | Both plans return identical snapshot-bound answers | Queries crossing publication/reclaim observe a missing or repeated source |
| Resources | Eligible duplicate byte-time falls with unchanged build concurrency | Segment memory is unchanged; aggregate RSS or I/O may still regress |
| Retention | Reclaim can change independently of expiration policy | Changing retention cadence as well would confound answers and resource accounting |

**H0:** the candidate has no useful pressure benefit, or its commit-path cost outweighs the benefit. Record that outcome and retain the baseline; do not widen the experiment until it produces a win.

## Proposed implementation boundary

1. Preserve the finite sealed-label snapshot and existing worker grouping. Track successful publication and already published labels locally to this pass.
2. Before building, reclaim only the already published oldest contiguous prefix through `Intake::reclaim`.
3. After each worker group has joined, reclaim the now-eligible oldest prefix before starting the next group. A later successful label cannot cross an earlier missing, failed, or unfinished label.
4. On a build failure, reclaim only eligible earlier labels, return the failure, and leave later journal files intact. On a checkpoint/reclaim failure, stop and propagate it without attempting later deletions.
5. Keep Segment publication, stream checkpoint, journal deletion, ACK, and retry semantics unchanged. Reuse the commit-thread reclaim operation rather than deleting from a worker.
6. Keep retention cadence and rules unchanged. Keep `seal_workers`, query-plan defaults, codecs, formats, and the builder unchanged during this comparison.

This first candidate still waits for the slowest worker inside one group. Completion-driven scheduling within a group is a separate hypothesis, justified only by measured residual delay. No new executor, plugin boundary, public configuration, dependency, or persisted watermark is proposed.

This is independent of the [bounded-sealer milestone](bounded-sealer.md): it cannot fix that milestone's working-set failure. Rebase and reevaluate the mechanism if the builder or scheduler changes before implementation.

## Implementation and verification sequence

Each step produces reviewable evidence. Policy/oracle/protocol changes are separate commits from implementation under [AGENTS.md](../../AGENTS.md).

1. **Freeze the comparison.** Record the baseline and candidate SHAs, confirm the base finding still exists, and register the draft below with exact harness commands. Add instrumentation for publication/reclaim events and full delivery lifetimes to both variants, then measure its perturbation. Do not edit historical results or existing qualification gates.
2. **Expose the scheduling decision to deterministic tests.** Prefer module-local injection of build completion/reclaim effects over a new production port. Use barriers and channels with bounded watchdogs, not wall-time assertions dependent on disk speed. Demonstrate the baseline misses the proposed progress condition and preserve that trace as the mechanism control.
3. **Implement the narrow candidate.** Change only when the sealer requests existing reclamation. Document the new scheduling and failure interleavings in the retained-history view and affected diagrams; reconcile ADR-0025's parallel sealer description. If implementation requires a change to sync ordering or public semantics, stop and propose that change separately.
4. **Check correctness before timing.** Run the cases below, both query plans, existing delivery/history tests, and the required fast checks. Register meaningful mutants/check entries separately when introducing them. Keep the Python oracles independent and unchanged unless a separately justified specification gap is found.
5. **Run the registered comparison when implementation/run scope is authorized.** Preserve every trial, including failed or inconclusive ones. Compare the same offered workload, not only successfully accepted traffic.
6. **Decide.** Adopt only if correctness holds and the stated performance rule passes. Otherwise preserve the counterexample or null result. Update `CURRENT.md`, the verification matrix, architecture records, and result links to the actual outcome. Existing qualification status remains unchanged until its protocols run on the candidate revision.

### Required correctness and progress cases

| Case | Deciding observation |
| --- | --- |
| Multiple worker groups | First group's eligible files are reclaimed before a deliberately blocked later group is released; the baseline fails this new progress assertion |
| Preexisting Segment | Its oldest journal duplicate is reclaimed before an unrelated build is allowed to complete |
| Out-of-order completion and failure | A later published Segment never permits reclaim across an earlier failed or unfinished label; an earlier successful prefix can still progress |
| Reclaim/checkpoint error | A failed checkpoint prevents deletion and later reclaim; an error cannot be silently converted to successful capacity restoration |
| Retry after partial progress | Already removed files are not reclaimed again; surviving published Segments are reused; remaining files finish in order |
| Query and paging | Logs, metrics, monotonic counters with resets, and spans agree under scan and walk before/after publication and reclaim, including a page boundary across the transition |
| Recovery states | Reopen states before publication, after publication, after checkpoint, and after deletion preserve every retained ACKed Batch and its exact bytes, bindings and deduplication behavior |

Use disposable fixtures for deterministic recovery states. Destructive fault campaigns and qualification runs are not authorized by this planning change. Negative controls must include a forced end-of-pass delay, skipping an earlier failed label, and deleting before checkpoint success; each relevant checker must reject its injected defect. These are proposed controls, not registered mutants or claims of current coverage. Existing [HIST-1/2 and delivery checks](../formal/verification-matrix.md) remain the baseline.

## Current environment and PC work split

The work is an investigation of the existing native product. It introduces no WebAssembly runtime, Docker requirement, release stage, or release tag. The journal-reclaim branch remains separate from any discussion of managed extensions.

The inspected development environment is Debian 13 on x86-64, with a four-CPU-equivalent cgroup quota (five visible CPUs), a 16 GiB memory ceiling and approximately 29 GiB free workspace storage. Its workspace is overlayfs and PID 1 is not systemd. These are setup observations, not workload measurements. Rust is not initially installed; install the repository's pinned toolchain before running Rust checks. Run builds and test processes directly, with bounded build parallelism; Docker is not needed.

| Work | Execute in this environment | Evidence that belongs on the PC or another representative host |
| --- | --- | --- |
| State-machine reasoning | Enumerate small completion/failure schedules; distinguish safety from conditional progress; inject an unsafe reclaim and a delayed-reclaim control | No hardware dependency for the abstract result; it is not proof of the full Rust implementation |
| Implementation | Narrow scheduling change, deterministic progress/failure tests, source review and documentation | No need to offload ordinary development |
| Correctness | Existing delivery/history tests, both query plans, independent oracles, simulated recovery states, meaningful negative controls | Repeat important recovery cases on the actual filesystem; timing or physical durability does not transfer from overlayfs |
| Exploratory behavior | Bounded native-process A/B experiments, byte-time accounting, checkpoint interference, query concurrency, memory and CPU measurements | Actual target-host latency/capacity and sustained-load results; rerun the same frozen protocol rather than extrapolating |
| Resource lifetime | Short repeated cycles with explicit data/time ceilings and recorded RSS | Longer soak, actual disk pressure and resource-enforcement behavior on the intended storage/kernel stack |
| Installation | Build/package checks and static systemd unit validation when relevant | Actual systemd lifecycle, account/permission setup and cgroup enforcement in a disposable VM or dedicated installation; this scheduler change alone does not require reinstalling the everyday PC |

The initial local mechanism investigation is specified separately in the [registered local protocol](../experiments/benchmarks/journal-reclaim-local-protocol.md). It is smaller than the draft mixed-load comparison and makes no speedup or qualification claim.

### Local stopping points

1. Establish a runnable baseline and retain any preexisting failed check separately from candidate regressions. Missing tools or failed setup are not test failures and do not become passes.
2. Demonstrate the progress mechanism and safety controls before trying to measure a speedup. Use the existing scheduler as a baseline, not as the semantic oracle.
3. Run the candidate's functional checks and bounded experiments that fit the inspected CPU, memory and disk budgets. Record the filesystem, CPU quota, source revision, build, commands and exits alongside every result. Smaller experiments need their own registered scope; they cannot silently stand in for the full draft comparison below.
4. Stop local measurement on the runner's resource limits or unresolved correctness failures. Classify an environment-limited result as such; do not transfer an unexplained failure to the PC as an installation task.
5. Prepare a handoff only once local evidence is interpretable. The handoff may establish a rejected hypothesis; it need not contain a candidate recommended for adoption.

### Reproducible PC handoff

Provide frozen baseline/candidate source SHAs, lockfiles, toolchain and build settings, fixture manifests and hashes, the registered protocol and bounded native runner, preflight commands, result schema and an automatic comparison. Include local counterexamples and limitations so the PC run answers the remaining question instead of repeating the investigation.

Preflight the PC's OS/architecture, available resources, filesystem, toolchain and binary compatibility. A binary built on this environment's newer glibc may not run on an older target: rebuild both variants from the frozen sources with one target-compatible toolchain, or supply verified compatible builds, and record the resulting hashes. Do not assume the repository's historical PC description is still current.

Use owned scratch directories and unprivileged processes for the performance comparison; no service installation, root access, Docker, module runtime or release is necessary. If systemd installation acceptance is separately required, adapt it for a disposable Linux VM. Do not run the existing container-specific `tools/qualification/install/acceptance.sh` unchanged on the everyday PC. Filesystem-sensitive results and any target-profile qualification remain separate from local functional correctness and exploratory measurements.

## Draft comparison protocol

Status: **proposed, not registered or executable**. No benchmark harness is added by this plan. Resolve the explicit preparation items below, record commands and fixture hashes, and freeze a protocol revision before any comparison. The thresholds are proposed engineering decision rules, not observations or changes to existing qualification criteria.

### Fixed comparison and workloads

- Compare the base scheduler and the narrow candidate with identical instrumentation, release build, dependencies, durability, retention, TLS, and resource placement. Record the full SHAs, toolchain, kernel, filesystem, CPU allocation and disk limits.
- Use seeds `0xA11FA001`, `0xA11FA002`, `0xA11FA003`; one paired baseline/candidate trial per seed per cell, alternating run order. Each invocation owns one disposable directory; run cells sequentially under the existing [bounded runner](../QUALIFICATION.md#harness-boundary).
- Use 32 enrolled senders with actual durable spools and normal sender retry behavior. Precompute identical seeded schedules for each paired trial; continue accounting for scheduled offers while senders back up. An open-loop source must not silently become a successful-ACK-paced source.
- Retain the three preparation tiers: (A) a fixed preloaded queue of eight 64 MiB sealed files to isolate reclaim progress; (B) continuous offered load at 60% of a frozen baseline sustainable rate; (C) at 110% to exercise pressure. Use `seal_workers=1` and `2`, a 1 GiB journal ceiling, and 64 MiB journal files. Calibrate the rate using only the baseline and a disjoint seed `0xA11FA000`; freeze the resulting numeric rates before candidate trials. If a stable baseline rate cannot be established within the budget, do not run the comparison.
- Use 30 s warmup and 120 s measurement for continuous-load cells, followed by a separately timed drain capped at 120 s. Bound source generation and reject a cell at preflight if it cannot fit the live-data ceiling. Reduce the offered scope only through a new protocol revision, not during a run.
- Freeze a corpus manifest and a realistic Batch shape including the actual agent's source attributes, metrics with cumulative counters/resets, and spans. Record raw source bytes, OTLP bytes, encoded Batch bytes, record counts and journal bytes separately. The current [ingest generator](../../crates/fabric-server/examples/ingest_load.rs) bypasses the Spool and omits file attributes, so it is not an interchangeable baseline for this comparison.
- Run the preloaded mechanism cell without background queries. For continuous-load cells, independently run `scan` and `walk` with one persistent client issuing one seeded query per second, rotating log search, metric history, counter rate and trace-ID lookup. Keep query text/window/limit distributions identical per pair and report scheduling lag. Retention should not expire fixture records within a trial; preflight its byte ceiling while counting journal plus Segments and all other live data.
- Per invocation: at most 5 GiB live data, 50 MiB retained evidence, 30 min wall time, and a four-CPU host allocation. Record sender and server memory separately. A watchdog breach, missing artifact, incomplete drain or uncontrolled host contention is incomplete/inconclusive, never a pass. The runner's sampled disk ceiling is not a hard quota.

### Metrics and measurement boundaries

| Metric | Definition |
| --- | --- |
| Eligible duplicate byte-time | Integral of bytes in the oldest contiguous published-but-unreclaimed journal prefix, in byte-seconds, over the measured interval; report bytes blocked behind an unpublished prefix separately |
| Publication-to-reclaim delay | Monotonic elapsed time from completed durable Segment publication to completed reclaim, per label, with file size and worker count |
| Pressure | Journal occupancy over time, time intake is capacity-blocked, refusal count, offered/admitted/committed records and bytes, and sender backlog at fixed interval boundaries |
| Delivery latency | Successful-attempt ACK latency and offer-to-durable-ACK latency including every refusal, backoff and spool wait, reported separately; unfinished offers remain right-censored backlog rather than disappearing from p99 |
| Throughput and drain | Committed encoded Batch bytes per measured second; completed sealed/reclaimed bytes; raw text rate separately; end-of-load backlog and post-load drain duration |
| Shared-resource cost | Server/sender peak RSS, CPU seconds, checkpoint count/duration, bytes read/written, and all live bytes including retained source files, spools, journal, Segments, scratch and logs |
| Query behavior | End-to-end query p50/p99, time to queryable evidence, oracle-exact rows and all answer fields, for both plans; no freshness gain presumed from reclamation alone |

For load latency, report completed/offered counts with the distribution. If censored offers could change the p99 decision, call the latency gate inconclusive. Use a monotonic clock for local durations; do not infer cross-host latency from unsynchronized clocks.

### Proposed decision rule

All correctness cases must pass, with zero missing retained ACKed records, retry-created logical duplicates, query-oracle mismatches, or deletion before checkpoint success. The deterministic progress cases must distinguish the candidate from the baseline.

For every continuous-load cell, compare paired per-seed results: the median across the three seeds of offer-to-ACK p99 and query p99 must not worsen by more than 10%, and no individual paired trial may regress by more than 20%. The same aggregate/non-outlier rule applies to peak RSS and measured server CPU per committed encoded MiB. Report absolute values beside ratios and do not treat a ratio over a zero/missing baseline as a pass.

For pressure cells, require at least a 25% reduction in median eligible duplicate byte-time and at least a 10% improvement in either capacity-blocked duration or end-of-load sender backlog, with neither regressing by more than 10%. If the baseline has no meaningful pressure, the cell cannot decide H1. A correctness failure rejects the candidate regardless of speed; a progress improvement without a pressure benefit establishes only the scheduling mechanism, not an optimization worth adopting.

## Local result and remaining decision

The candidate reclaims the contiguous published prefix before building and after each worker group. Six focused tests passed, including two rejected scheduling mutants and a real checkpoint-write failure/retry fixture; see the [run record](../experiments/benchmarks/journal-reclaim-local-run-01.md) for broader checks and limitations. This establishes earlier scheduling, not a measured pressure benefit. Keep the branch as an experiment until the mixed-load trade-off is measured; do not infer readiness to merge from progress alone.
