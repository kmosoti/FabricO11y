# Responsibility benchmark audit before the three-tier trials

Status: **source/evidence audit and benchmark design, not new performance measurements**. Audited consolidated `milestone/streaming-segment-output` at `440aaef`. The owner requested responsibility-level benchmarking and multidimensional inefficiency analysis before sequential development, small and moderate deployment tests. Existing aggregate pilots do not establish independent per-responsibility attribution. [Study focus](../../research/development-moderate-plan.md), [consumer overview](../../research/consumer-performance.md), [earlier route catalog](sealing-pipeline-experiment-design.md).

## What is ready, and what is missing

There are six functional responsibility groups and two supporting groups. They are logical boundaries in existing processes, not independent daemons or necessarily distinct threads. A timing of one public operation can cross several groups. Shared copies, caches, syncs and locks must not be charged twice. A source audit identifies candidate mechanisms, not their cost or causal dominance.

| Group | Current executable boundary and evidence | Benchmark status | Measurements needed before attributing inefficiency |
| --- | --- | --- | --- |
| Collection | Linux `read_lines_costed`, host adapters; native `collect_logs`/`collect_once`; [Spindle study](spindle-run-01.md), [native pilots](dev-small-local-run-01.md) | Native aggregate runnable; pure file reader callable; no current unified phase harness | Poll wait, filesystem/host reads, line validation/cursor work, OTLP encoding, bytes/records read versus committed, CPU/allocations; separate Spool append |
| Delivery | `Spool::next_unacked`, `Runtime::deliver`, native Sender; [native scaled results](native-scaled-spindle-run-01.md), [delivery oracle](../../../tools/qualification/DELIVERY_ORACLE.md) | Native aggregate runnable; observed attempt timing is partial | Spool fetch/decode, throttle/backoff wait, connection/TLS, HTTP send/server answer, ACK persistence/reclaim, per-attempt hash/output; exact retry bytes and sequence |
| Storage | Native Spool, FrameLog, server intake/commit, Segment publication/reclaim/retention; [reclaim tests](journal-reclaim-local-run-01.md) | Correctness/recovery tools and aggregate resource observations exist; native phase attribution missing | Queue wait, encode/write/sync phases, ACK/checkpoint persistence, read amplification, physical-versus-buffered IO, startup replay, disk-full handling, retained storage |
| Processing | `rows::parse`, Segment sorting/projection/Arrow/Parquet/filter construction; [isolated writer pilot](streaming-output-local-run-01.md), `seal_output_probe` | Current isolated whole-build probe runnable; individual phases not exposed by that probe | Decode/projection, sort, Arrow materialization, compression/index construction, output hashing; CPU and live allocation lifetimes/copies per input byte/record |
| Query | `History::run`, source selection, scan/walk, HTTP `spawn_blocking`; [historical attribution](query-attribution-run-01.md), [query oracle](../../../tools/qualification/QUERY_ORACLE.md) | Engine/API callable; historical shape studies exist; current consumer/phase harness missing | Admission/dispatch and cache-lock wait, source planning, read/decode/pruning, filtering/rates/result heap, JSON serialization, client startup/TLS; first-query visibility |
| Control/scheduling | Control enrollment/poll/persist; runtime collection/delivery loop; sealer grouped threads | Native flows and mechanism tests exist; current isolated timing/interaction harness missing | Inventory size, poll/update cost, desired-to-applied time, lock wait, persist bytes/syncs, worker occupancy/idle barriers, arrival-to-start time and fairness |
| Buffer ownership | Native byte caps, allocation probe, bounded result heap, caches | Whole-build allocations and process RSS available; complete lifetime/ownership accounting missing | Live/peak/cumulative allocated bytes, retained capacity, copy bytes, per-cache charge, input/output/scratch ownership, queue bytes and age |
| Self-observation | Native meter/stdout, once-second Python resources and inventory sampler | Existing sampler runnable; timed memory categories and overhead calibration missing | Observation CPU/IO/allocation, SHA/logging/JSON overhead, lost samples, file-enumeration races, monotonic clock validity, cgroup events/categories |

[Source/evidence inventory](data/responsibility-benchmark-audit/inventory.json) stores exact file hashes, revision and coverage states. “Runnable aggregate” does not mean each subphase can currently be timed independently. The three-tier attribution prerequisite is **not yet satisfied**; this audit does not label it ready merely because files or historic results exist.

## Findings that earlier summaries can conceal

### Collection and delivery: the belt timer is not the whole route

[Runtime delivery](../../../src/spindle/runtime.rs) calls `next_unacked` **before** starting its attempt timer. The timer includes token-bucket waiting and `sender.send`, then ends before SHA formatting/stdout callback and `record_ack`. [Spool retrieval](../../../src/spindle/spool.rs) reads the payload and decodes the Batch to validate sequence; `record_ack` performs durable state updates/reclaim. Thus reported native attempt duration is neither complete delivery CPU cost nor pure network RTT. Decompose it without moving ACK boundaries. Separately retain the exact wall-clock source/collection/receive/ACK-observation clocks.

Spool append clones the supplied Batch before assigning identity/sequence and encoding it. That is an allocation/copy hypothesis, not proof that removing the clone improves throughput. Acquisition, candidate ownership, commit failure and restart behavior must remain correct.

The [native configuration validator](../../../src/spindle/runtime.rs) permits `spool_bytes` from 8 KiB through **256 MiB**. The default and validator maximum coincide. A modeled outage buffer greater than 256 MiB is **not reachable by simply raising the setting**. Distinguish default, accepted configuration range, admission threshold and process memory limit. This audit does not raise that ceiling or amend its contract.

Collection scans/cursors, OTLP encoding, metrics and durable append share a runtime cycle. Compare 1 versus 16 selected paths at equal input volume, short versus long lines and quiet versus backlog conditions. Bound busy-pass cost separately from one-second polling and metric sampling; unread source inventory is not Spool custody.

### Storage and processing: name changes do not remove material

The [Segment builder](../../../crates/fabric-server/src/segment.rs) clones Entries, projects rows, sorts, builds Arrow arrays and writes compressed tables. The output writer candidate removed an encoded-output Vec; it did not remove the decoded input/row/Arrow inventories. The isolated probe keeps inputs resident before timing, so incremental heap excludes that resident baseline. Report both baseline and peak total demand rather than treating the incremental number as the complete processing budget.

The [legacy append attribution study](append-attribution-s0.md) measures FOL2. It cannot supply native FAB1/Group journal sync fractions without new native instrumentation. Likewise `/proc/io` buffered byte counters are not device latency/IOPS. We need native write/sync boundary durations and cache state before calling IO the bottleneck. Maintain exact two-sync durability and publication-before-reclaim rules in every comparison.

### Query: output bounds do not bound the factory

[Query HTTP dispatch](../../../crates/fabric-server/src/http.rs) uses `spawn_blocking` without a dedicated Fabric query admission/deadline mechanism at that route. The paginated heap bounds output candidates by `limit+1`, not reader/cache/decoder/serialization bytes across concurrent queries. [Rate processing](../../../crates/fabric-server/src/query.rs) collects matching metric points into a Vec and has no paginated limit; series cardinality and time window need their own workload coordinate.

The walk plan refreshes/selects cached tail/Segment state under a mutex. Measure lock-hold/wait time, cache footprint and cache hits/misses before proposing another reader thread. Broad scan versus indexed walk, empty versus selective versus broad queries, tail versus Segments and CLI versus persistent HTTP are different cuts. Historic speedups belong to their old revisions and fixtures, not this audit.

### Control and scheduling: machines can be idle behind another machine

[Sealing](../../../crates/fabric-server/src/sealer.rs) creates scoped worker threads for a group and joins all of them before continuing with the next group. A slow build can leave finished slots idle until the group completes; spawning per group also has a cost to measure. Ordered publication/checkpoint/reclaim invariants still apply. A ready-task pool is a candidate, not an accepted replacement for ordered reclaim.

[Control persistence](../../../crates/fabric-server/src/control.rs) serializes the complete node inventory and uses synced file replacement for an administrative mutation. Enrollment/configuration cost may grow with fleet size even at unchanged event rate. HTTP control state is shared through a mutex. Test 1/20/200 enrollment cardinality, quiet polling versus configuration updates and their intake effects; do not weaken persistence to make an administrative benchmark faster.

### Buffers and observation: the measuring equipment has a cost

Shared cgroup charges include file cache, anonymous memory, kernel memory and the harness. The earlier enterprise run reached the shared limit despite small process RSS; no timed category/event baseline was retained. Future runs must record these categories and deltas, not attribute all shared memory to the server.

Native per-attempt SHA/output and Python directory scans/JSON/resource observation consume CPU and can perturb cache/IO. The monitor already failed once on concurrent Spool rename. Calibrate observation overhead with matched instrumented/uninstrumented trials and preserve correctness evidence. More detailed measurement is useful only when its perturbation and population are known.

## Common measurement contract

For every experiment, freeze an isolated boundary and an integration boundary. Retain fixture/source/binary hashes, seed, record/byte/cardinality counts, revisions, cache state, command/exits and failures. Use the following dimensions together:

- **Useful work:** records and source/encoded/stored bytes completed; exact fidelity, gaps and errors. Distinguish scheduled offers from actual production.
- **Time:** monotonic queue, active and wait spans; p50/p99 and sample populations; first-seal/startup/steady/burst/drain windows. Observation realtime clocks remain separate from local durations.
- **Compute:** CPU seconds per completed encoded MiB and per record; per-thread occupancy/context switches where supported. Wall duration is not CPU utilization.
- **Memory:** starting live bytes, peak live bytes, cumulative allocations, retained capacity and lifetime; process RSS and cgroup anonymous/file/kernel categories separately.
- **IO/network:** source/encoded/output/copy/read/write bytes, sync durations, requests/retries and transport conditions; no physical IO claim from logical bytes alone.
- **Coverage and cost:** oracle result, errors/timeouts/missing populations, measured host/resource budget. Money/energy need actual price/power inputs; do not manufacture them.

Spans have operation IDs and parent IDs. Nested spans are inclusive; compute exclusive values only with validated child intervals. Do not sum overlapping thread wall spans into elapsed time or attribute a shared buffer once to storage and again to processing. Record observer overhead and unavailable counters. Benchmarked operations cannot omit sync, coverage metadata, errors or cancellation work just to appear faster.

## Rotate and distinguish explanations

| Perspective / competing hypothesis | Perturbation | Supporting versus falsifying observation |
| --- | --- | --- |
| CPU/copies dominate | Hold bytes fixed, vary small-record count/attributes; resident-input processing cut | CPU/allocation demand rises with record count; flat CPU with growing sync waits weakens explanation |
| IO/sync dominates | Hold bytes/shape fixed, separate write/sync; warm/cold cache and measured real device later | Sync/read waits dominate boundary; resident decoding remains slow weakens IO-only explanation |
| Queue/admission dominates | Fix input/retained corpus; vary offered query concurrency and 1/2 sealers | Queue age grows while useful completions flatten; speeding the isolated transform alone need not fix it |
| Memory/cache/cardinality dominates | Equal bytes, vary nodes/series/paths/results; first-seal and repeated-seal windows | Peak/retained bytes grow independently of volume; falling RSS with increasing source backlog is not success |
| Scheduler barriers dominate | Equal aggregate work, mixed fast/slow file shapes; record slot occupancy | Slots idle while eligible tasks wait; uniformly busy slots weaken the barrier hypothesis |
| Control-plane contention dominates | Equal intake; vary polls/configuration mutations and node cardinality | Query/intake/ACK delay tracks lock/persist spans; no measurable interaction weakens hypothesis |
| Observation distorts results | Matched minimal/detail instrumentation; same exact work | Reproducible CPU/time/cost change; small changes with unstable workloads are inconclusive |

Higher-level interactions are mandatory after isolated screens: collection × delivery; delivery × journal/reclaim; seal × query; query concurrency × cache memory; control mutation × intake; all of them × instrumentation. Preserve tradeoffs instead of declaring the isolated fastest machine the factory winner.

## Smallest benchmark fixtures

These are candidate fixtures, not registered performance workloads. Exact counts/iterations/runs, budgets and decision margins must be committed in a separate protocol before execution.

| Group | Minimal fixture / baseline | Rejecting control / precision obligation |
| --- | --- | --- |
| Collection | Deterministic newline files with independent expected bodies/cursors; actual procfs capture separated | Missing/altered line must fail fidelity; overlong/invalid/nonregular source must produce expected explicit outcomes |
| Delivery | Precommitted native Spool → real TLS server; normal and reproducible retry response | Altered recovered ACK hash must fail; retries send identical bytes; rejected ACK cannot advance cursor |
| Storage | Native known Batches, append/replay/ACK/reclaim; native journal Groups/publication | Existing injected sync/reclaim failures retain exact custody and order; do not add physical/destructive faults here |
| Processing | Frozen encoded Groups; parse/build outputs versus independent raw record expectation | Altered/missing projected row fails; retain bit-exact raw Batch custody and ordering; resident baseline counted |
| Query | Tail-only, Segment-only and mixed history; fixed query shapes and pages | Independent query oracle rejects altered rows/coverage/order/snapshot; include rate/reset series and missing optional-index fallback |
| Control/scheduling | Fresh 1/20/200-node inventory, polls/configuration, heterogeneous seal tasks | Desired/applied/restart semantics preserved; thread changes cannot reclaim across an unpublished hole |
| Buffers | Same inputs at different window/cardinality/file/concurrency coordinates | Hidden retained payload or underestimated allocation must be exposed; configured capacity exhaustion is explicit |
| Observation | Same useful operation with minimal versus detailed capture | Missing/reordered/invalid timing samples rejected; filesystem rename race remains a regression |

Do not benchmark an invented lightweight Sender or substitute in-memory Spool as though it measured the native deployment. Pure-kernel and fixed-memory probes are useful only with their narrower scope stated. Include actual human CLI startup, persistent programmatic sessions and agent search/pagination patterns at the consumer boundary.

## Prerequisites before sequential development → small → moderate

1. Implement/freeze current native boundary probes and timed spans where missing. Keep pure component cuts separate from real native adapters. Reuse existing public functions/tools where possible; no generic plugin framework or new service is required.
2. Validate precision, counts and span/observer rejecting controls before recording performance. Establish instrumentation perturbation through its own bounded registered calibration.
3. Run isolated representative fixtures sequentially, identify concrete cost concentrations and preserve null outcomes. Rank routes by avoided CPU/bytes/wait and engineering risk, not novelty. Any optimization gets a fixed baseline/candidate protocol and independent confirmation.
4. Freeze the selected baseline configuration and consumer-inclusive three-tier protocol. Do not silently optimize between tiers. Run development through its first seal, fully stop/analyze, then small, then moderate; moderate fleet-cardinality cut is distinct from 20 busy-node volume.
5. Evaluate the factory as well as machines: exact answers, useful evidence freshness, per-window query/ACK tails, bounded queue age, memory/cache/disk headroom and recovery. Choose defaults from measured Pareto operating points, not from the number of available CPUs.

Current next implementation is the common boundary/observer instrumentation and missing native probes, followed by calibration and isolated measurements. No caps, threads, wire formats, query semantics, or installed defaults changed in this audit. No three-tier run was started.

## Verification of this audit

Source/evidence paths and SHA-256s in the inventory were generated and checked against the working tree at the audited revision. Referenced code confirms the identified mechanisms, but none of the new cost hypotheses has been benchmarked. `bun tools/docs/check.mjs` and `git diff --check` exited 0. No runtime code changed, so fast checks were not repeated; the earlier 19-pass/one-existing-Clippy-failure receipt remains historical evidence, not verification of future probes.
