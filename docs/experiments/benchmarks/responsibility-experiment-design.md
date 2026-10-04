# Responsibility experiments: design and execution on this machine

Status: **revised design, measurements paused at the owner's request**. This follows the [responsibility audit](responsibility-benchmark-audit.md) and [initial isolation protocol](responsibility-isolation-protocol.md). The initial protocol remains historical; this design does not reinterpret its results. Implement and commit a revised executable protocol, fixtures and controls before new timed runs. The aim is defensible component attribution, followed by a separate composite development → small → moderate experiment and evidence-based caps/threading choices.

## Current machine and implications

Observed during design: five affinity-visible logical CPUs `[0,1,2,3,4]`, cgroup quota `400000/100000` = **four CPU equivalents**, shared memory ceiling **16 GiB**, approximately **10.9 GiB currently charged** including existing cache and other work, and **13.0 GiB free workspace disk**. These are transient observations, not reserved resources or a capacity promise. PID 1 is `tail`; packaged systemd enforcement is not exercised here. The filesystem is the shared virtualized overlay used in the earlier native experiments.

“All resources available” means one owned experiment may use the allowed CPU affinity and current four-CPU quota, with no competing build, benchmark, compression, analysis or fixture-generation jobs of ours. It does not mean preallocating all RAM, inventing four-way parallelism in a serial API, reserving four physical cores, or controlling unrelated host work. Affinity alone does not isolate quota, page cache, writeback, memory bandwidth or storage.

Record `cpu.stat` quota throttling, available affinity, process CPU/high-water, cgroup memory categories/events/pressure and starting disk/cache context. Unexplained ambient interference becomes a recorded limitation or preregistered exclusion, never a silently discarded slow run. The new protocol must specify an exclusion/repetition rule before execution.

## What can run in parallel

| Activity | Parallelism on this machine | Reason / constraint |
| --- | --- | --- |
| Source/design review, statistical review, small document edits | Parallel; independent agents own disjoint files or remain read-only | No timed workload is active; source review can challenge common assumptions |
| Compilation plus light documentation work | One build invocation, initially two Cargo jobs; independent light work allowed | Bound preparation contention; avoid multiple writers/builds using the same target cache |
| Fixture preparation and static metadata checks | Bounded parallel preparation only; start with at most two jobs | Preparation is untimed; predict aggregate disk/memory before allocating |
| Full functional preflight / fixture debugging | Sequential per case initially | Makes causality, ownership and cleanup failures attributable |
| Isolated performance measurements | **One experiment at a time** | Shared quota/cache/filesystem would confound concurrent runs even on different pinned CPUs |
| Real client/server, sealer threads or concurrent queries within a case | Concurrent **inside the one experiment**, exactly as specified | Their competition is the controlled subject, not background work |
| Hashing, oracle grading, compression and cleanup | After measurement, before the next case; small validation tasks may overlap each other | These consume CPU/IO/cache and must finish before timing resumes |
| Analysis/report drafting while another timed case runs | No on-host heavy analysis or code/build tools | Use the gaps between runs; prose can be prepared without consuming benchmark-host resources |
| Three deployment tiers | **Development, then small, then moderate** | Stop, validate and clean each tier before starting the next |

## Three distinct experiment types

1. **Unit cost:** one native operation/worker, fixed input/state, repeat enough work for reliable timing. Measures CPU/bytes/waits and live inventories per useful record or encoded byte. It may leave CPUs idle because the operation is serial or waiting for sync; that is evidence.
2. **Capacity and scaling:** one responsibility at a time, allowed to use the full machine quota. Vary only concurrency that the real API/runtime supports, initially 1/2/4 where appropriate. Retain a named two-CPU constrained point separately rather than treating it as whole-machine capacity. For delivery, server and client are one combined experiment; compare a fixed 2+2 affinity partition with shared full affinity only as an explicit additional coordinate.
3. **Interaction and composite:** deliberately overlap specified work, such as query + sealing, while fixing offered loads and resource settings. Later run actual native collection/delivery/storage/processing/query/control together for each deployment tier. Background workload becomes part of the fixture with measured production rate, not an uncontrolled helper.

Do not multiply all dimensions into one giant test matrix. Screen one axis at a time against a frozen reference, then test the interactions suggested by the measured bottlenecks. Identical data, seed and operation counts are required for paired comparisons, but fresh independent state is required when one case would otherwise warm or mutate the next.

## Phase 0: readiness before timing

Run a small untimed end-to-end functional preflight of **every mode and fixture**, including every query shape, plan, storage layout and control operation. Check independent source fidelity, native exact ACK/replay custody, query rows/envelopes/snapshots, control persistence and cleanup. Reject changed, missing and duplicate records; reject altered query rows/metadata and ACK hashes. Failure must stop campaign launch, not merely stop after earlier groups have already produced measurements.

The first query fixture defect is understood from source: the Segment fixture created a directory without a valid active journal, while `History::sources` always opens the active journal and interprets its absence as `Interrupted("journal moved")`. Initialize legal query state through native storage APIs and verify its actual snapshot/state before timing; do not suppress the error or add retries around a permanently invalid fixture. Preserve the original failure as a regression input/recipe.

Validate every measured query answer outside its timing span, not only the first answer on a reused History object. Bound retained answers or use a separately verified streaming/digest representation; a digest is not a substitute for oracle validation of rows and envelope. Retain enough immutable fixture recipe/metadata and diagnostic state for failures before owned cleanup.

## Phase 1: measurement calibration

- Use monotonic wall clocks for operation spans. Source/collection/receive/ACK/query-visibility wall clocks are separate data observations.
- Current `/proc` CPU counters have tick granularity, commonly 10 ms here, and snapshot reads allocate/do IO. Their acquisition presently lies partly outside the measured wall interval while inside the CPU delta. Prefer an appropriate high-resolution process/thread CPU clock or measure long batches with counter access once at each end; report actual resolution and counter overhead.
- Keep timing samples in preallocated bounded storage and serialize them after the timed batch. Printing JSON, hashes or allocation-rich `/proc` snapshots between tiny calls perturbs allocator/cache state even when excluded from each wall span.
- Calibrate **minimal observation, low-rate observation and intended observation** on identical useful work. Include the Python sampler and directory inventory cost, not just the counted allocator. Use final process `rusage`/high-water for short operations; monitor absence must not imply zero memory use.
- Move recursive scratch-footprint walks out of 100 ms timing loops. Enforce precomputed fixture/output bounds and check inventory at explicit operation boundaries; use low-cost free-space checks as appropriate. Do not claim hard instantaneous disk enforcement from sampled checks.
- Compare uninstrumented allocation timing against counted-allocation timing in matched fresh processes. Keep live baseline, incremental peak and total peak separate. Allocation, RSS and cgroup file/kernel memory answer different questions.

Calibration yields a recorded minimum useful batch duration and observer perturbation estimate. Freeze those values before the main measurement campaign. An observer-induced change of the same order as a candidate speedup makes that improvement inconclusive, even if an arbitrary percentage threshold passes.

## Phase 2: isolated responsibility matrix

| Group | Core cuts | Important variations and precision checks |
| --- | --- | --- |
| Collection | File read/validate/cursor, host sampling, OTLP encoding, separate native collect+Spool inclusive boundary | 1/16 paths; 128/900/3,500-byte bodies; quiet/backlogged; exact source and cursor/gap outcomes; separate polling wait |
| Delivery | Native Spool fetch/decode, throttle/backoff, real TLS send/server response, durable local ACK | First connection/keep-alive; uncapped/configured cap; normal/reproducible retry; one Batch in flight; exact bytes/sequence on every retry |
| Storage | Native append/encode/write/sync inclusive boundary, submit-to-durable-answer, reopen/replay, checkpoint/reclaim/retention | Small and actual 64 MiB journal boundaries; multiple files; acknowledged versus unacknowledged inventory; buffered/warm state labeled; fixed sync semantics |
| Processing | Decode/project, sorting, materialization, full Segment build/publication | Sorted, seeded shuffled and interleaved/out-of-order timestamps; 16/64 MiB input where bounded; same bytes with different record/cardinality counts; exact projected/raw rows |
| Query | Source planning, scan/walk, read/decode/pruning, filtering/rates, result construction/serialization | Valid tail/Segment/mixed states; first call versus warm calls; empty/selective/broad; metrics/rates with reset/cardinality cases; traces when claimed; pagination/snapshot errors; oracle grades every answer |
| Control/scheduling | Enroll/configure/poll/inventory/persist; actual native sealer pass | 1/20/200 nodes; desired-to-applied time; 1/2 workers initially; heterogeneous task costs and enough files to fill workers; complete replay/reclaim ordering |
| Buffer ownership | Allocations and lifetimes across all preceding cuts | Baseline/live/peak/cumulative bytes, copy amplification, cached retained capacity, queue bytes and age; no double-counting borrowed/shared allocations |
| Self-observation | Capturing timings, native SHA/logging, sampler, formatting | Observation disabled/minimal/intended; recorded sample loss/races/counter precision; equivalent correctness work |

The first implementation's 4,096 rows span roughly 0.5–14 MiB and its sorter receives already ordered timestamps. Those points do not characterize a 64 MiB sealer or sorting under disorder. Larger representative cases need explicit preflight size/memory predictions; they are not silently substituted into protocol 1.

Public operations sometimes cross groups: Segment build includes IO and publication; native Spool append includes cloning/encoding/sync. Label inclusive boundaries and nested spans. Do not subtract timings from separate trials to invent an unmeasured subphase. Keep an explicit residual for work not attributed by instrumentation.

## Repetitions and statistical interpretation

Use an initial five fresh-process repetitions per frozen baseline case for descriptive characterization, with matched AB/BA order for candidates and instrumented variants. Freeze a variance-triggered extension rule before launch; any extra repetitions preserve all originals. Keep setup, first-call/startup, warm steady operations and final drain as distinct populations. Rotate query shape/order deterministically to expose cross-query cache effects.

Report per-trial counts, median, range/IQR, throughput and normalized CPU/allocation demand; retain between-process variation. For fewer than 1,000 comparable operation observations, label p99 as insufficiently characterized and show the observed maximum instead of presenting it as a stable tail estimate. Even 1,000 observations and repeated fresh processes yield an exploratory tail, not a production SLO. Correlated calls sharing buffers/caches/commits are not independent samples simply because there are many of them.

When an operation is too expensive to reach that population within its bound, report its repeat distribution and sample count. Do not expand the run unboundedly to obtain a percentile. Estimate the suite duration/storage from untimed preflight and calibration; then freeze per-case iterations and a campaign wall/disk budget before starting it.

No defaults are chosen from isolated winners. Primary endpoint and practical improvement margin are registered separately for each candidate before confirmation, with exactness a hard constraint and CPU/memory/IO/queue costs retained. A fast query that starves sealing, a small Spool that postpones collection, or a small result heap with an unbounded decoder is not a successful whole-system configuration.

## Phase 3: selected interactions, then later three-tier composite

After isolated evidence identifies candidate bottlenecks, measure collection × delivery, delivery × journal/reclaim, seal × query, query concurrency × caches, and control updates × intake where warranted. Run one intentional interaction at a time, with unchanged reference configuration and workload. Compare useful completion, queue age and per-window tail behavior rather than averages alone.

Only after that preparation freeze the composite protocol and run development → small → moderate sequentially. Development must cross its first seal. Moderate needs separate 20-busy-node and 200-lower-rate-node cuts at equal aggregate input. Keep native Spindles, original source clocks, actual query consumers and exact independent checks. Record write-to-first-matching-query-response, response time and missing/error/coverage outcomes. The current owner's request is to settle this design first; no composite run has started.

## Resource and evidence lifecycle

Before each case, predict source + native state + replay + retained evidence + diagnostic failure state against available disk. Retain the existing **4 GiB free-disk floor** and owner-marked directories. Per-process address-space limits are experimental guardrails, not RAM/cgroup budgets; record their exact values. Budget the aggregate client/server/probe/harness demand and leave room for the host/cache. Four concurrent builders may exceed a cap that one builder fits; do not silently raise it or label that limit a product failure.

The available 13 GiB workspace space is enough for bounded fixture/replay work, not long retention simulation. Use one mutable trial workspace at a time. After the timed work stops: close native processes, perform fresh replay/oracle checks, write summaries/raw sample files and content hashes, record outcome and failure reproducer, verify evidence, then remove only owned temporary data. Record logical bytes removed plus free space before/after. Free-space changes can differ from deleted logical bytes due to allocation, caching, other work and retained evidence.

Keep success/failure evidence before cleanup. Do not preserve credentials. Any intentionally deleted raw state must have a documented reproducible fixture or adequate retained diagnostic snapshot; the initial run retained only its query error/partial timing/recipe, so that limitation stays visible. Repeated builds, compressed artifacts and scratch are separate inventories; never purge unrelated caches or historical evidence to satisfy a trial bound.

## Preserved initial execution and design consequences

[Partial run status](data/responsibility-isolation-run-01/status.json): 72 completed trials across collection, delivery, storage and processing; first query trial failed; control, scheduling and observer modes did not execute. All 73 trial directories were removed, about **1.91 GiB cumulative logical scratch** reclaimed; roughly **39 MiB evidence** retained. [Raw run output](data/responsibility-isolation-run-01/run.txt), [query failure](data/responsibility-isolation-run-01/evidence/query-128-p1-plain/stderr.txt), source/binary metadata and per-trial summaries are preserved. The entire suite is not a pass. Query suboperations before its failure are partial evidence, not completed query trials.

The partial run used two-CPU probe affinity, three pairs, ordered sort input and the initial observer. It is exploratory evidence under those conditions; do not promote its p99s, full-machine capacity, sorting cost or cap recommendations. Query corruption controls were placed after measured queries rather than completing all-mode preflight. This methodological departure is preserved and corrected by the next design, not retroactively described as a pass.

An independent read-only design review reached the same concerns about sorted input, first/warm-call mixing, sample size, two-CPU affinity, counter boundaries and observer interference. That review informs the design; it is not an executable correctness or acceptance oracle. Documentation and evidence-reference checks validate this design record only. All new performance measurements remain paused.
