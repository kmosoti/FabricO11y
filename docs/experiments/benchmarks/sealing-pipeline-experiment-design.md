# Sealing pipeline: experiment design and optimization routes

Status: **preliminary experiment design; all routes unrun**. Companion to the [computational model](../../research/sealing-pipeline-model.md) and [earlier adaptive-concurrency proposal](adaptive-sealing-hypotheses.md). This is a search plan, not a registered executable benchmark or a performance result. Runtime source remains at the journal-reclaim candidate; source checkpoint for the proposal is `ba0c25da3124eddc753950cdf7846c4cd8086cf8`.

## Rotate the representation before choosing a mechanism

One pipeline has several useful mathematical representations. A route must survive the relevant views, not merely improve the view that suggested it.

| View | Representation | Question exposed | Candidate route or rejecting evidence |
| --- | --- | --- | --- |
| Dataflow | Directed graph of stages and bounded edges | Is a costly handoff buying useful overlap? | Fuse adjacent CPU stages when queue/copy overhead exceeds overlap benefit |
| Ownership | Lease graph and allocation conservation ledger | Who pins each allocation, and for how long? | Move/borrow buffers; reject if sharing pins more memory than copying saves |
| Queueing | Arrival/service distributions and constrained queues | Does added concurrency reduce waiting or just move it? | Pull work within fixed bytes; reject growth in total backlog or ACK latency |
| Critical path | Completion DAG with oldest-file reclaim frontier | Which task actually releases capacity next? | Prefer draining/oldest-file tasks; reject starvation and throughput loss |
| External memory | Runs, block transfers and bounded merge fan-in | What RAM allocation removes a complete merge pass? | Allocate memory to runs/merge; reject excess spill or unbounded catalogs |
| Processor/cache | CPU work, memory traffic and locality | Are workers doing useful transforms or moving data? | Index sorting, fused projections or locality-aware tasks within bounded storage; reject pointer-chasing/cache costs |
| Storage | Mixed read/write/sync service demand | Will read-ahead help or interfere with durable ingest? | Small sequential prefetch and bounded write depth; reject worse sync/ACK tails |
| Feedback control | Delayed/noisy sensors, admitted work and hysteresis | Does adaptation settle or oscillate? | Slow additive admission changes; reject repeated overshoot or controller churn |
| Information fidelity | Exact raw bytes plus derived ordered projections | Does a cheaper representation preserve every required fact? | Retain raw custody bytes, verify all signals and filters; reject any semantic mismatch |
| Failure/recovery | Interrupted transitions and attempt generations | Are buffers or files reused while an old task still owns them? | Generation/attempt guards, joined cancellation, explicit publish/reclaim boundary |
| Scale/economics | Pareto surface over host and workload distributions | Which configuration gives the best result for this small server? | Retain regional winners; reject added complexity with no meaningful end-to-end gain |

No single view establishes optimality. For example, removing a copy reduces memory traffic but can extend an input lease, shrink the read window and reduce throughput. Adding prefetch can hide I/O while taking RAM from run buffers, adding another merge pass. Those are testable interactions, not reasons to assume either mechanism wins.

## Coordinates of the design space

A design point is `(theta, w, h)` rather than a worker count alone:

- `theta`: execution topology (sequential/fused/staged), buffer ownership (owned/moved/borrowed), byte budgets, task count, read/write depth, active files, output chunk target, run size, merge fan-in, ordinal lookahead, ready-task priority and feedback policy.
- `w`: offered rate and burstiness, file/job-size variability, encoded-to-decoded expansion, row/attribute sizes, identity cardinality, timestamp disorder and logs/metrics/spans mix.
- `h`: CPU quota/affinity and contention, memory ceiling/allocator, cache state, device/filesystem, shared read/write/sync behavior and available measurements.

Keep durable formats, sync order, exact Batch bytes, retention and ingest/retry semantics fixed. Changing them is a separate experiment with separate authority, not another point in this search grid. Query execution is outside the primary timing workload; output semantics and independent correctness checks remain in scope.

Initial *screening candidates*, to filter for feasibility before any run: processing slots `{1,2,4}`; read depth `{1,2,4}`; active files `{1,2}`; output chunk targets `{256 KiB,1 MiB,4 MiB}`; sort arenas `{8,16,32 MiB}`; merge fan-in `{2,4,8,16}`. Input framing stays valid: an output target below the maximum encoded frame is not permission to assume decoding fits that target. Use one global sealing budget per comparison; candidate budget strata `{96,192,384 MiB}` are exploratory possibilities, not promises that all legal inputs/codecs fit.

The Cartesian product is deliberately not the experiment. Prune points violating the ledger, frame/row workspace requirements, file-descriptor budget, temp-disk envelope or complete drain-resource feasibility. Do not call an infeasible low-memory point a slow implementation. Never infer feasibility from typical memory usage alone.

## Route catalog

All routes start as **PROPOSED**, not measured winners. Each promotes only through a rejecting control, a native pilot and an independently frozen confirmation where applicable.

| Route | Predicted mechanism | Smallest discriminating experiment | Null/rejection and next branch |
| --- | --- | --- | --- |
| R0: output streaming | Avoid retaining a complete encoded Parquet file in a `Vec` before storage | Same sequential builder, output buffering versus bounded hashing file writer; compare exact bytes and allocation lifetimes | Peak live-heap ratio >=0.90 or codec semantics differ; retain baseline or investigate row/codec retention instead |
| R1: bounded construction | Replace whole-file rows with bounded run generation/merge | Existing single-builder memory shapes and exact-output checks | Existing 80 MiB/10% finite gates fail; identify decode, cardinality, writer or run-reader growth before threading |
| R2: ownership transfer | Avoid cloning data solely to cross stage boundaries | Same bounded work, copy versus move versus immutable borrow; count copied bytes and pinned capacity-time | Copy ratio >=0.80 or memory/latency guard fails; prefer move or bounded copy over long-lived borrowing |
| R3: prefetch | Overlap otherwise exposed storage wait with useful computation | Same builder/budget with no read-ahead versus depths 1/2/4, cold-eligible and warm cases reported separately | Throughput ratio <=1.10 or ACK guard fails; suppress prefetch in that operating region |
| R4: pull scheduling | Remove slowest-worker group barriers | Matched fixed group/pull limits on mixed job durations | Throughput ratio <=1.10; prefer simpler grouping unless another declared objective improves |
| R5: stage fusion | Avoid queue synchronization and intermediate materialization | Bounded fused decode/project versus separate bounded tasks at equal worker/memory limits | No >10% CPU-cost saving or reduced overlap harms throughput; retain staged path in I/O-limited region |
| R6: run/fan-in allocation | Spend scarce RAM where it removes expensive merge passes | Feasible `(S,k)` grid with equal total budget; compare predicted/actual run count and read/write volume | No near-minimal feasible I/O or latency/memory guard fails; revisit division between prefetch, arenas and merge |
| R7: index/key sorting | Move compact indices/keys while preserving owned payload arenas | Within R1's same byte caps: sort row structs versus indices, including variable strings | No >10% CPU-cost saving or extra indirection harms full build; retain direct sort; never create an unbounded global index |
| R8: frontier-aware scheduling | Use resources on work that can release oldest journal capacity | Same task pool, neutral ready order versus drain/oldest-file preference and aging | Backlog-area ratio >=0.80, starvation, or >5% steady-throughput loss; tune lookahead or retain neutral order |
| R9: adaptive admission | Follow changing demand without spending unnecessary working memory | Best calibrated fixed pull point versus feedback on alternating low/burst/pressure load | Backlog-area ratio >=0.80 or instability/guard failure; fixed configuration is a legitimate winner |
| R10: lifetime reuse | Reuse a bounded set of arenas without accumulating retained capacity | Repeated identical build/drain cycles, bounded pooling versus fresh allocations | Post-warmup RSS trend exceeds frozen margin, or allocation savings do not help end-to-end cost; cap/trim pools and investigate allocator separately |

R0 is source-grounded: [`write_table`](../../../crates/fabric-server/src/segment.rs) currently writes through a `Vec` and later writes/hashes that byte array. It is also part of the accepted bounded-sealer direction, so isolate it as an attribution step rather than inventing a competing durable format. Byte-identical output is an explicit check, not an assumption about changing the writer sink.

Routes R5 and R7 are exploratory new hypotheses with proposed 10% CPU-cost margins (`R_CPU >=0.90` is their null); lower CPU cost is acceptable only with the end-to-end guards. R0 proposes >10% peak live-heap reduction (`R_heap >=0.90` is its null). R10's lifetime margin must be frozen from an operational growth allowance and service horizon, for example `delta_M = allowed_resident_growth / expected_build_cycles`; passing a finite slope test does not prove zero growth forever. Logical safety failures reject any route regardless of speed. Other statistical margins follow the [model](../../research/sealing-pipeline-model.md#8-hypotheses-from-different-perspectives).

## Interactions to test explicitly

| Coupled dimensions | Why a one-factor result can mislead | Required contrast |
| --- | --- | --- |
| Prefetch x sort memory | Read-ahead consumes bytes that could avoid a spill/merge pass | Fixed total budget, reallocate rather than add RAM |
| Worker count x decoder expansion | More workers can multiply a large transient expansion | Same encoded size with small/large rows and attributes; count real allocations |
| Borrowing x output delay | A slow sink pins source buffers | Delay writer completion while checking lease lifetime and throughput |
| Fan-in x active files | Per-file readers multiply globally | One/two files with the same aggregate reader/byte budget |
| Task fusion x I/O latency | Fusing saves overhead but may reduce overlap | CPU-dominated and slow-read cells with equal compute ceilings |
| Priority x file variability | Favoring the oldest can free capacity but delay small easy jobs | Variable first-file size, mixed completion order, bounded aging |
| Adaptation x sensor delay | A late pressure signal acts after work has allocated | Delay/jitter samples; check bounded admission and overshoot |
| Pool reuse x idle lifetime | Lower allocation count can keep RSS high between bursts | Equal input per cycle with fixed post-drain observation periods |

For a two-level exploratory contrast on positive cost `Y`, the interaction on log scale can be estimated as `I_AB = log(Y_11) - log(Y_10) - log(Y_01) + log(Y_00)`. Zero interaction is the multiplicative-additivity reference. Randomize/replicate cells and report uncertainty. Zero-cost cells require an additive or other predeclared model. This diagnostic does not replace the primary ratio-of-expectations test for the burst claim.

## Bounded search procedure

1. **Map and reject infeasible designs.** Apply the ledger, transition guards and resource lower bounds; preserve a reason for every rejected point. Enumerate the small ownership/deadlock model with mutations before performance claims.
2. **Measure cost attribution.** Establish where time/bytes/allocations go in the current and sequential bounded paths. Historical profiles nominate routes; they do not rank this host's bottlenecks. Apply instrumentation equally and quantify its perturbation.
3. **Screen mechanisms on calibration data.** Use a small, predeclared route budget (initially at most 12 feasible configurations per screening batch). Contrast endpoints and include a control; choose workloads that discriminate the proposed mechanism. Pilot thresholds never become confirmation results.
4. **Rotate to interactions and hostile cases.** For promising routes run the specific crossed contrasts above. Add the counterexample most likely to invalidate each apparent gain, such as a slow output sink for borrowing or high expansion for extra workers.
5. **Keep a Pareto archive and route graph.** Record parent design, changed coordinate, full resource cost, uncertainty and failed guards. Keep low-memory, low-I/O and low-latency alternatives even when their maximum throughput is lower. Failed routes remain visible.
6. **Freeze confirmation.** Choose a limited candidate set and comparator from calibration, then register all constants, independent seeds, trial count/power design, statistical estimands and error control. Use fresh data; do not continuously retune against confirmation results.
7. **Transfer and decide.** Reproduce feasible finalists on the PC. Publish operating regions and uncertainty, not one universal winner. Choose the simplest configuration meeting the declared workload/SLO and budget when gains are practically equivalent.

If a later search uses Bayesian optimization or another surrogate, it must model feasibility/uncertainty and retain independent confirmation; it is optional and not assumed superior to a small informed grid. No automated production self-tuning or unbounded search campaign is proposed.

## Experiment blocks and stopping rules

| Block | Artifact | What it can decide | Current status |
| --- | --- | --- | --- |
| E0 | Finite ownership/admission model and rejecting traces | Safety and conditional progress for explicit finite bounds | Designed; not implemented/run |
| E1 | Sequential bounded-build allocation attribution | Per-stage memory/copy/I/O causes and existing finite memory gates | Designed; product candidate absent |
| E2 | Native isolated-route pilots | Which mechanisms deserve more investigation on this host | Designed; no measurements |
| E3 | Crossed interaction pilots | Where route effects combine, cancel or reverse | Designed; no measurements |
| E4 | Frozen paired confirmation | Predeclared statistical and practical claims | Protocol/runner not yet registered |
| E5 | PC reproduction and repeated-cycle memory run | Hardware-specific performance, pressure and lifetime | PC environment not inspected |

Use the model's independent safety/equivalence checks before timing. The first safety or resource-envelope breach stops that candidate and preserves its trace; it is not silently dropped from the dataset. Timeout, infeasible input, missing sensor, unavailable controller and inconclusive censored latency have separate outcomes. Overload is a deliberate workload, not a reason to omit refused offers.

Each screening batch must predeclare total wall-time/compute/storage budgets and an upper trial count before execution; the 12-configuration cap alone does not bound cost. Per-trial limits are no more than 30 minutes, 5 GiB live data, 50 MiB retained evidence and a 4 GiB free-disk floor, with an explicit feasible memory ceiling. Trials run sequentially and retain compact evidence; all temporary data paths must be owned. Unprivileged delay injection can test scheduling response here, but does not emulate all real-disk behavior. Physical faults and installation changes remain separate scope.

A route record should contain: ID/state; parent SHA and design vector; workload/host vectors; predicted resource bottleneck; H1/H0 and practical margin; invariant obligations; exact command/fixture hashes; observed outcomes and confidence bounds; failures/counterexamples; resource budget/actual high-water; interaction checks; decision and next experiment. Until an executable harness and frozen protocol exist, fields requiring measurements remain **not run**, not estimated victories.
