# Adaptive sealing and builder memory: hypotheses

Status: **proposal; no implementation or performance experiment run**. This document develops hypotheses and a draft validation design. It is not an executable registered protocol and changes no existing qualification or bounded-sealer gate. Freeze a separate protocol with source revisions, executable commands, fixture hashes, instrumentation, budgets and statistical methods before measurement.

Source baseline for this proposal: `42eda24e7e0a2c22710acebc3b175826d71f7cee`, containing the [journal-reclaim candidate](../../milestones/journal-reclaim-progress.md). Keep that reclaim mechanism identical across scheduling comparisons. The [product contract](../../PRODUCT-CONTRACT.md), [Segment lifecycle](../../decisions/ADR-0020-store-sealed-history-as-parquet-segments.md) and [registered bounded-sealer acceptance](../../milestones/bounded-sealer.md#registered-acceptance-protocol) remain authoritative.

## Factory model and actual source

| Satisfactory role | FabricO11y mechanism | Resource consequence |
| --- | --- | --- |
| Miners and local storage | Spindle collection and durable Spool | Unacknowledged Batches remain upstream; storage is finite |
| Receiving station | Bounded intake queue and single journal commit thread | ACK follows durable commit; ingestion needs CPU, memory and disk headroom |
| Closed freight containers | Sealed journal files, normally 64 MiB | Jobs waiting on disk; their full contents need not all be resident in RAM |
| Processing machines | Segment builders | Each active build allocates its own working material |
| Factory floor inside machines | Live allocations for decoded Groups, copied records, extracted rows, Arrow arrays and compression | More active builds multiply simultaneous working sets |
| Temporary storage containers | External-sort runs on disk | Can reduce working RAM while adding disk bytes and I/O |
| Finished storage and clearance | Published Parquet Segment, checkpoint, ordered journal reclaim | Publication and returned capacity are separate events |

[`sealer::build_and_reclaim`](../../../crates/fabric-server/src/sealer.rs) assigns worker-sized groups and waits for every worker in a group. A thread is not a reserved CPU. Choosing workers from detected CPUs is a concurrency heuristic, not CPU reservation. Idle threads are not equivalent to active memory-heavy builds; reducing a configured ceiling need not reduce low-load RSS when only one job exists anyway.

[`segment::read_sealed` and `segment::build`](../../../crates/fabric-server/src/segment.rs) retain all decoded Groups, clone Entries into `records`, extract all projected rows, sort them, and construct column arrays for writing. Several representations coexist. In factory terms, the machine unloads a whole freight container onto its floor and keeps original material, sorting trays and packing material together. This is working-set amplification, not evidence of a memory leak.

The earlier [sealer study](sealer-study-run-01.md) measured peak **incremental live heap** around 356 MiB for a 64 MiB steady journal file, and 1,389 MiB for 256 MiB. These are historical fixture-specific results, not universal reservations or measurements on the proposed controller. Its external-merge prototype measured about 43–45 MiB over that range. Product implementation is still outstanding. The separate [ingest study](ingest-run-01.md) measured whole-server peak **RSS** of 584/976/1,564 MiB with 1/2/3 workers; do not substitute those totals for per-builder heap.

## Hypotheses and nulls

There are two independent changes: choose how many machines run, and reduce how much material each machine holds. Test them separately before their combination.

| ID | Alternative hypothesis | Null hypothesis | Proposed deciding metric |
| --- | --- | --- | --- |
| A: adaptive concurrency | A demand-driven pull scheduler reduces burst backlog byte-time by at least 20% relative to a calibrated fixed pull scheduler, while satisfying the safety and resource guardrails below | Burst backlog byte-time is not reduced by at least 20%; an apparent benefit may also be inadmissible because a guardrail fails | Paired burst backlog-area ratio, adaptive/fixed, at identical offered load and resource ceilings |
| M: bounded working set | A bounded builder uses at most 80 MiB incremental live heap on the registered shapes and grows by at most 10% from 64 to 256 MiB, while preserving output semantics | At least one tested shape exceeds the ceiling, or growth exceeds 10%; semantic failure rejects the candidate independently | Counting-allocator peak per build and file-size scaling; existing bounded-sealer gates unchanged |
| L: lifetime | After warmup, repeated equal-sized build/drain cycles reach a stable resident-memory plateau rather than an upward trend | Post-drain resident memory has sustained growth above a predeclared practical slope margin | Per-cycle live heap, RSS and cgroup charged memory, recorded separately |

For A, the formal primary null is `R >= 0.80`, where `R` is the ratio of expected backlog area for adaptive versus fixed pull scheduling under the frozen burst population. Support the alternative only if the predeclared one-sided 95% upper confidence bound for `R` is below 0.80, and every guardrail passes. Failure to reject is inconclusive or no demonstrated benefit; it does not prove identical behavior. A statistically detectable 2% improvement would not meet this proposed engineering margin.

M reuses a finite empirical acceptance rule, not a proof for arbitrary input. A short/noisy lifetime run is inconclusive, not evidence for either L or its null. L needs its own registered cycle count, observation interval and slope/plateau criterion before a run; this proposal cannot declare an existing soak repaired.

## Mathematical resource model

Let `Q(t)` be bytes in sealed journal files not yet reclaimed, including in-progress builds and published files blocked behind an earlier hole. Let `lambda(t)` be newly sealed journal bytes/s and `r(t)` completed reclaimed journal bytes/s. Between discrete events:

```text
Q(t) = Q(0) + integral(lambda(t) - r(t)) dt
A_Q = integral Q(t) dt                         [byte-seconds]
```

Record publication/build throughput separately from `r(t)`: a later finished file does not itself restore capacity. Stable drain needs long-run reclaim capacity above the arrival rate. Overload cannot be fixed by spawning unbounded threads; bounded backpressure remains necessary.

For planning, model resident memory as:

```text
RSS(t) ~= R_base(t) + sum live builder working sets
          + allocator-retained resident memory + other resident overhead
H_current(F, shape) ~= a(shape) * F + b(shape)
```

`F` is input journal size. The historical steady fixture suggests `a` near 5.5, not a proven maximum. An illustrative 512 MiB *incremental sealing budget* would fit one estimated 356 MiB build, not two. Dividing 512 by a historical 43 MiB prototype suggests about eleven builds arithmetically, but does not authorize that concurrency: writer overhead, allocator retention, intake, I/O and CPU still constrain it. Neither figure is a whole-server memory guarantee.

A bounded implementation should instead account for frame decode, raw-record chunks, byte-capped sort runs, output row groups, merge readers, writer/compression workspace, and per-identity metadata. Track each simultaneously live component. A row-count cap alone is insufficient: a few large strings can consume the floor before the row threshold is reached. Byte estimates must be checked against actual allocations, and maximum single-row/frame overshoot must be accounted for.

**Important limit of the accepted design:** ADR-0022 opens every spill run with a 64 KiB reader buffer and one merge-head row. With `k` runs, these costs are `O(k)`, and `k` grows with input size. A flat measured curve over 64–256 MiB is not an arbitrary-file-size constant bound. A stronger bound needs a frozen maximum input size/run count or bounded merge fan-in with multiple passes, plus bounds on per-identity metadata and codec workspace. This is a question to resolve before a new implementation; it does not silently revise the accepted design or its finite acceptance protocol.

Process RSS and cgroup charged memory are distinct from live heap. The allocator can keep emptied floor space mapped; filesystem page cache and kernel charges can remain in the cgroup. Do not add RSS to `memory.current` as if they were independent costs. Record both, plus `memory.stat`, swap and pressure. A CPU quota is normalized capacity, not a number of cores claimed by workers.

## Candidate controller to freeze before testing

A machine pulls the oldest available unclaimed file when admitted. Several admitted machines may finish out of order; only the oldest contiguous durably published prefix is checkpointed and reclaimed. Concurrent callbacks must not bypass that commit-thread operation. Do not introduce stage-parallel sorting/compression in this experiment.

Proposed starting policy, subject to pilot calibration before registration:

- Zero active builds when no work exists; start one when work arrives and memory admission permits. A waiting thread is not counted as an active builder.
- Sample once/s and use a five-second CPU smoothing window. Normalize server CPU seconds by elapsed monotonic time and effective CPU capacity (quota/affinity), with host contention recorded separately. Begin with a 70% CPU target to leave ingress headroom; this is a proposed tuning value, not a product SLO.
- Permit one additional active build after three consecutive queued/headroom observations, with at least ten seconds between increases. Do not interpret low CPU as spare disk capacity. Sustained I/O pressure, CPU throttling, insufficient memory or ACK-latency distress veto expansion.
- On distress, stop launching extra builds and reduce the desired concurrency; allow durable in-flight builds to finish. Never cancel a build by deleting journal files. Downscale after ten seconds of low demand; inject threshold jitter to verify hysteresis.
- Keep a fixed maximum concurrency, capped for this local proposal at four, and an independently explicit server-memory ceiling/reserve for intake and other services. A worker must acquire an atomic memory reservation before loading its file; reserve for all active jobs, not just their currently observed allocations. Release on every completion/error/panic path. A reservation based on a sampled historical peak is an estimate, not a guaranteed allocation bound.
- If metrics or safe reservations are unavailable, make no scale-up decision. Hold memory-heavy jobs on disk when admission fails. Lowering the desired count cannot instantly lower RSS or interrupt allocations already in flight. Whether to continue an already-admitted primary worker is a separately tested availability rule, not an excuse to bypass a memory veto.

The CPU target is feedback, not a hard CPU cap; sampling can overshoot it. Likewise a reservation protects only its declared resource model. An externally configured OS memory boundary remains a last-resort ceiling and does not make OOM termination acceptable evidence of safe scheduling. No new public setting, cgroup writer or resource-enforcement claim is implemented by this proposal.

## Comparisons that isolate the cause

| Variant | Scheduling | Builder | Question |
| --- | --- | --- | --- |
| G | Existing worker groups | Current whole-file | Frozen source baseline |
| P1/P2/P4 | Fixed pull concurrency 1/2/4 | Current whole-file | Does removing the slowest-worker group barrier help? |
| A | Adaptive pull, same hard ceiling and resources | Current whole-file | Does resource feedback help beyond pull scheduling alone? |
| M | Fixed one-worker | Bounded builder | Does reducing one machine's floor space work independently? |
| AM | Adaptive pull | Bounded builder | Only after A and M are independently interpretable; do their benefits compose? |

Choose the fixed pull comparator using a disjoint calibration workload before confirmation; freeze the rule and worker count, and retain all fixed-worker results. Do not cherry-pick a weak fixed setting after observing adaptive outcomes. If a resource budget excludes a fixed count, report it as infeasible, not as a performance loss. In a one-job workload, all feasible policies can use one builder; equal memory there is a valid null/control outcome.

Keep journal size, reclaim policy, retention, TLS, Batch encoding, checkpoint/sync order, allocator, build flags and instrumentation constant within each comparison. Query load is excluded from the primary pipeline experiment. Still run existing delivery/history tests to ensure stored output remains valid; a later query-load experiment needs separate scope. Existing bounded-sealer design assumes one builder: concurrent bounded builds are the additional AM experiment, not already accepted behavior.

## Workloads, metrics and decision guardrails

Draft workload cells: empty/one-job low load; steady input at 60% of the calibrated fixed baseline capacity; a reproducible burst followed by drain; sustained 110% overload; mixed-duration files to expose group barriers; large rows and high attribute/identity cardinality; controlled slow reads/writes while CPU is low. Use source bytes, encoded Batch bytes and journal bytes as distinct denominators. Keep valid generated logs, metrics, spans, gaps and out-of-order timestamps, with exact source/retained-byte manifests.

Use independent pilot and confirmation seeds. Pilot runs estimate variance, controller parameters and feasible budgets; do not count pilot outcomes as confirmation. Choose and freeze paired sample count from pilot variance for 80% power at the 20% margin. Predeclare the primary burst cell, paired run order randomization, trial-level confidence method and treatment of censored/incomplete trials. Other cells are guardrails or exploratory; any additional confirmatory claims require a declared multiple-comparison rule. Host runs, not individual Batches within one run, are the independent experimental units.

Measure:

- Backlog byte-time, publication-to-reclaim delay, journal occupancy, blocked admission time and unfinished sender backlog, with scheduled offers continuing to be accounted for under pressure.
- Offer-to-durable-ACK p99 including spool wait, refusals and retries; report successful-attempt latency separately. Count offered, admitted, ACKed and incomplete Batches. Missing ACKs must not vanish from the denominator.
- Published/reclaimed journal bytes/s and CPU seconds per reclaimed encoded MiB; drain duration separately from steady throughput.
- Live heap peak, whole-server RSS peak/area, post-drain RSS, cgroup charged-memory peak, swap/pressure and allocator retention; builder counts and reservation ledger. Use an allocator instrument in both variants for allocation studies and measure its perturbation before timing studies.
- Disk bytes, spill high-water, I/O pressure/throttling and checkpoint latency; fixed-size buffers move costs to temporary storage and may contend with intake.

Proposed engineering guardrails: zero custody/byte-equivalence violations and zero memory-ceiling/reservation violations on every trial; no completed-trial ACK p99 regression above 10% and no >5% sustained-throughput regression in feasible steady cells relative to the frozen comparator. Apply predeclared paired confidence rules, not ratios of selectively successful sends. If censored delivery could change the latency decision, classify that cell inconclusive. Memory peak must stay below the predeclared budget and CPU target excursions must be reported, even when the primary backlog metric improves. Lower thread count alone is not an efficiency result; completed work and total resource cost decide.

## Deterministic validation and negative controls

Before performance runs, build a small scheduler/controller model with explicit input samples and labelled jobs. State fairness: an admitted build eventually returns or errors; reservations, publication and checkpoint replies are delivered; available resources eventually suffice for any job whose progress is claimed. No progress claim applies under permanently inadequate memory or an unreleased worker.

Required cases: out-of-order completion; a failed oldest job; checkpoint failure; reservation denied; racing admissions; error/panic release; missing/stale metrics; low CPU plus I/O pressure; jitter around thresholds; a budget reduction while workers run; empty queue; repeated retry and shutdown/restart. Check hard active-count and reservation invariants, no duplicate job assignment, no deletion across a hole, and conditional eventual completion. A pure finite model verifies its own transitions, not arbitrary concurrent Rust.

Inject at least: admission that ignores existing reservations; leaked/double-released reservation; scale-up despite I/O pressure; no hysteresis; crossing an unpublished prefix; full-file/row-only buffering inside a supposedly bounded builder; accumulating completed allocations between cycles. Each checker must reject its representative defect. Preserve traces and source diffs. Retain independent delivery/query oracles and existing regression suites unchanged.

## Local environment and PC boundary

This request produces a hypothesis/validation plan only. No scheduler or builder is changed and no experiment result is claimed.

Here: develop deterministic models and allocation fixtures, implement scoped candidates when authorized, run correctness controls, and perform separately registered bounded pilot comparisons. The previously inspected four-CPU-equivalent/16 GiB environment can support exploratory native experiments but overlayfs and shared host contention limit disk conclusions. Each pilot needs a runner with a fixed memory limit, a 5 GiB live-data/50 MiB evidence ceiling, a 30-minute trial watchdog and a free-disk floor of 4 GiB; reject workloads that cannot fit before launch. Set an experiment budget from that host, not an assumed desktop capacity.

PC: rerun the frozen pipeline comparison and repeated-cycle memory test on the actual CPU quota/affinity, RAM budget, allocator/kernel and disk/filesystem. Record baseline and candidate sources, toolchain, fixture hashes, limits and contention. Use native unprivileged processes and owned scratch directories. Docker, Wasm, system installation and releases are unnecessary. Physical power-loss or whole-disk fault tests remain separately scoped; a simulated slow-I/O case is not hardware qualification.

## Preparation checklist and results

Before registration: implement/freeze the native runner and fixture generator; specify exact commands and result schema; choose measurable memory and I/O signals with an unavailable-metric rule; select legal memory/admission bounds; calibrate fixed concurrency and offered rates; freeze statistical sample counts and controller constants; name source SHAs and instrumentation hashes. Register scheduler and builder protocols separately. Reconcile the accepted bounded-builder assumptions before combining them with concurrent admission.

Results: **not run**. No hypothesis or null has been accepted or rejected. The historic study motivates M; it supplies neither a controller result nor a guarantee for the product builder.
