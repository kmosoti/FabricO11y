# Development through moderate: configuration and query study plan

Status: **owner-directed focus and preliminary study plan; experiments and query refactor not implemented**. Focus testing and practical use on development, small and moderate workloads. Enterprise remains historical stress evidence, not the current optimization target. This does not change the [product contract](../PRODUCT-CONTRACT.md), installed defaults, qualification status or existing registered protocols. Freeze a separate protocol before each new measurement or acceptance comparison.

## Factory model and objective

In Satisfactory terms, Spindles mine host evidence, their durable Spools are local containers, delivery belts feed the server journal, and sealers refine journal files into retained Parquet Segments. Queries are a second production line drawing from both the Segment warehouse and the live journal. Human, agent and programmatic consumers order evidence from that line.

Optimize exact answers and useful evidence freshness under a measured CPU/memory/disk budget. The goal is the smallest resource configuration meeting declared ingestion, consumer response and coverage requirements, not the highest thread count or smallest RSS in isolation. [Existing consumer measurements and gaps](consumer-performance.md), [pipeline model](sealing-pipeline-model.md) and [sizing assumptions](workload-sizing.md) establish the starting evidence.

A thread is a machine processing admitted material; it does not reserve a CPU. A byte budget is floor space, a concurrency limit is the number of machines allowed to run, a queue cap is conveyor/container capacity, and retention is warehouse size. These controls cannot substitute for one another. Processing needs bounded scratch and access to inputs; separating concerns does not imply zero processing memory.

## Study sequence

1. Establish consumer-inclusive baselines for development (10 logs/s), small (1,000) and moderate (10,000), with 3× bursts, original source clocks, real Spindles, exact delivery/query checks and query visibility clocks. Development must cross the first-seal boundary. Moderate should separately test 20 busy Spindles and the model's 200 lower-rate Spindles at equal aggregate input; fleet cardinality is a separate coordinate.
2. Compare explicit one versus two sealing workers; then client query concurrency one, two and four. Fix workload/query shape and retention corpus before comparison. These are screening coordinates, not new shipping defaults. Client concurrency tests do not implement or demonstrate a server-side admission cap.
3. Attribute per-builder/per-query allocations, persistent caches, allocator retention, cgroup file/anonymous/kernel memory and disk IO. Record time-resolved categories and memory-event baselines; RSS alone missed cache pressure in earlier runs.
4. Introduce bounded query responsibilities only where profiling/correctness evidence supports a mechanism. A logical split may execute synchronously on one thread. Add dedicated loader/processor queues or extra threads only through a separately frozen comparison.
5. Independently repeat promising configurations, then longer soak/retention turnover and actual systemd budget enforcement on the PC. Keep failed configurations and intermediate evidence. Native cloud measurements do not qualify the installed service profile.

Each protocol must fix workload/seed, duration and burst, record size/cardinality, query mix/plan/window/page size, consumer concurrency/poll cadence, retained corpus, node count, cache state, resource bounds and exact runtime/harness revisions. Define p99 targets before execution. Keep the existing exact query oracle independent; no wire, query-answer or snapshot semantics are amended by this plan.

## Caps and threading to investigate

| Control | Current behavior/default | Study question and rejected shortcut |
| --- | --- | --- |
| Sealing threads | Configurable 1–16; default half available CPUs, clamped 1–4 | Does 2 materially reduce source/journal backlog versus 1 without unacceptable RAM/consumer-latency cost? CPU count alone does not determine useful concurrency |
| Query concurrency | HTTP dispatch through `spawn_blocking`; no dedicated Fabric admission limiter at that route | Does increasing offered concurrency improve completed queries or only queue/cache pressure? A runtime thread-pool limit is not a query resource budget |
| Query memory | Bounded `limit+1` output heap for paginated shapes; readers, tail/metadata caches and rate processing are additional | Can measured leased-buffer and scratch budgets bound working set without changing answers? Row limits are not total byte caps; rates have no pagination limit |
| Server/node memory | Installed defaults server `MemoryHigh=2560M`, `MemoryMax=3072M`; node `MemoryMax=256M`; slice `MemoryMax=3328M` | What working set plus cache/host reserve and burst margin fits each profile? These are settings, not intrinsic ceilings or universal minimums |
| Journal/Spool capacity | Default server journal 20 GiB, file 64 MiB; native Spool 256 MiB | What per-node outage and server sealing-interruption intervals must fit? Size from encoded production rate and required time, not from RAM |
| Retention | Default 24 hours and 20 GiB; either can evict old Segments | Does actual stored volume satisfy desired history? A nominal duration does not overcome an undersized byte budget |
| Task/queue capacity | Installed server `TasksMax=512`, node 128, aggregate slice 640 | What running/waiting work is useful before contention dominates? Task count is not a processing-worker count |

Source: [server config](../../crates/fabric-server/src/config.rs), [sealer](../../crates/fabric-server/src/sealer.rs), [query HTTP path](../../crates/fabric-server/src/http.rs), [query implementation](../../crates/fabric-server/src/query.rs), [installed units](../../packaging/systemd/fabrico11y-server.service). No new configuration keys or default values are selected here.

## Proposed query responsibilities: a second factory line

| Satisfactory analogy | Responsibility | Resource ownership/boundary |
| --- | --- | --- |
| Order counter / dispatcher | Validate query shape, identify snapshot, admit work | Small descriptors and explicitly limited active/waiting requests; avoid decoding payloads before admission |
| Warehouse inventory / route planner | Identify relevant journal ranges, Segments, row groups and optional indexes | Bounded metadata/cache accounting; preserve exact fallback when an index is unusable |
| Freight unloader | Read/decompress/decode selected data | Own bounded storage buffers and IO lifecycle; account library allocations and simultaneous reads |
| Constructors / assemblers | Filter, compare keys, compute rates and maintain requested results | Borrow supplied batches and own bounded per-task scratch; rate-series state needs a separate cardinality model |
| Output merger / packing station | Preserve global order, page boundary and answer metadata | Account result bytes/serialization, preserve completeness, freshness, gaps, snapshot and page-expiry behavior |
| Dispatcher ledger | Observe queues, CPU, bytes and errors | Distinguish waiting time from execution and IO; cancellation must actually release work/resources |

These are internal responsibilities in the existing native server, not separate services, new generic ports, or necessarily one thread per stage. Preserve the accepted [hexagonal boundaries](../decisions/ADR-0015-adopt-a-hexagonal-architecture.md), [query contract](../architecture/retained-history.md) and [query plan decision](../decisions/ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md). Storage mechanisms do not belong in the pure core merely because a transform can be pure. A budgeted partial-answer mode or changed rejection policy would need explicit contract work rather than silently truncating an exact answer.

Current source already separates HTTP dispatch, history/source selection, tail support and row parsing. The proposal is to make memory/admission ownership explicit where needed, not to claim that moving code between modules alone improves performance. Sealing and querying need coordinated host headroom: both read/transform evidence, and unconstrained concurrent caches/builders can compete even on separate threads.

## Hypotheses and evidence boundaries

- **H1 threading:** the smallest tested worker/concurrency combination satisfies fixed freshness/response criteria with exact answers and bounded backlog. **H0:** any criterion fails, or extra threads only move contention into CPU, IO or RAM.
- **H1 caps:** measured explicit buffer/admission budgets restrain peak allocations and cgroup demand across record-size/cardinality and burst perturbations without losing coverage. **H0:** hidden decoder/cache/serialization/rate state grows outside the budget, capacity fails, or exact answers change.
- **H1 query separation:** explicit loader/processor/output ownership improves the registered resource or latency metric without degrading other guarded metrics. **H0:** extra copies, handoffs, synchronization or cache eviction erase the improvement. Start with logical separation; parallel pipelines are a competing candidate.
- **H1 shared scheduling:** bounded query demand leaves ingestion and sealing sufficient headroom while interactive work meets targets. **H0:** either line starves or acceptable averages conceal unacceptable per-window tails.

Report a Pareto set rather than one magic configuration: memory-efficient, latency-efficient and throughput-efficient operating points can differ. Preserve warm/cold storage and quiet/burst/query-heavy cases. Reject apparent wins caused by missing records, stale snapshots, postponed collection, no first seal, altered input timing or excluded errors. Polling-based first-query visibility is an observed upper bound with cadence/response cost, not the exact internal publication instant.

## Next concrete work

Register and implement the finite consumer-inclusive baseline harness before changing runtime defaults. Start development and small sequentially, then moderate volume and fleet cardinality within declared disk/wall limits. Record source-write, collection, server receive, ACK observation, first matching query response and request-response clocks; verify precision and coverage independently. Use the baseline to freeze practical acceptance margins for the cap/thread experiments. No benchmark, cap change, query refactor, systemd enforcement or qualification was run as part of writing this plan.

Before the tier baselines, complete the [responsibility benchmark prerequisites](../experiments/benchmarks/responsibility-benchmark-audit.md). This source audit records current benchmark coverage, omitted phase boundaries, native cap restrictions and competing inefficiency hypotheses; it does not claim new benchmarks have run.
