# Performance refinement audit

**Source audit at `dfe7a3d`; focused correctness diagnostics executed; optimization experiments below are proposals.** No runtime configuration, wire format, durability rule or production implementation changed. This extends the [responsibility audit](../experiments/benchmarks/responsibility-benchmark-audit.md) and [revision-2 measurements](../experiments/benchmarks/responsibility-isolation-r2-run-01.md). It challenges the current defaults and the proposed replacements, including the accepted bounded-sealer design. Coverage is explicit; this is not a claim that every possible inefficiency has been exhausted.

The strongest direction is to reduce work and overlapping data representations per exact answer or durably delivered record, then schedule the remaining work against shared resource budgets. More workers and larger buffers can move the queue upstream without improving that objective. Transport is one dimension, not the organizing assumption.

## 1. Precision findings discovered during the audit

The performance review of [`rates`](../../crates/fabric-server/src/query.rs) found two correctness defects in addition to comparator allocation churn. The exact existing Rust function and row types were extracted without modification, compiled against the current `fabric-core`, and compared with the unchanged independent Python oracle's rate-row function. Compilation/execution exited **0**; comparison exited **1**, with three disagreements:

| Minimal input, one second apart | Independent expected result | Existing Rust transformation |
| --- | --- | --- |
| Two single-point series: attributes `{"a=b":"c"}` and `{"a":"b=c"}`, values 10 and 20 | No rate; neither series has two points | One invented rate of 10/s |
| One integer series: `2^53` then `2^53 + 1` | 1/s | 0/s |
| One integer series: `2^53 + 1` then `2^53` | Reset | 0/s, not a reset |

**Cause and repair direction.** Series identity uses a vector of formatted `key=value` strings; delimiter-containing keys and values make that representation non-injective. Use structural attribute-map identity and precompute comparison keys once. Keep identity separate from the contract's presentation ordering, and resolve any ordering ambiguity explicitly. Both integer values are converted to `f64` before comparison/subtraction, hiding small differences above `2^53`. Compare integers and form their delta exactly, using a sufficiently wide intermediate, before the final rate conversion. Mixed integer/double cases, non-finite doubles and resets need an explicit model and tests; indiscriminate floating-point reassociation is unsuitable.

These are **function-level counterexamples**, not executions through OTLP decoding, History or HTTP. The next repair must add those integration regressions and preserve the independent oracle. The [retained-history contract](../architecture/retained-history.md), [core counter-step](../../crates/fabric-core/src/query.rs) and [oracle](../../tools/qualification/query_oracle.py) are the relevant boundaries. Neither defect is fixed in this audit.

Retained evidence: [inputs, expected and actual rows](../experiments/benchmarks/data/performance-refinement-audit/comparison.json), [extracted Rust](../experiments/benchmarks/data/performance-refinement-audit/rate-probe.rs), [source hashes](../experiments/benchmarks/data/performance-refinement-audit/extraction.json), [execution receipts](../experiments/benchmarks/data/performance-refinement-audit/receipts.json), and [reproducer](../experiments/benchmarks/data/performance-refinement-audit/reproduce.py). At the recorded source, with Cargo/dependencies available, run:

```bash
python3 -B docs/experiments/benchmarks/data/performance-refinement-audit/reproduce.py
```

Exit 1 reproduces disagreement; it is not a successful correctness gate. The source and dependency lock are retained; this diagnostic does not measure performance or replace the full query oracle.

## 2. What the defaults actually constrain

| Setting/mechanism | Implemented value or rule | Assumption to challenge |
| --- | --- | --- |
| Journal file rotation | Configurable `journal_file_bytes`, default 64 MiB; minimum 64 KiB, maximum `journal_bytes` | Couples builder input, Segment count, reclaim granularity and query-source count. It is not a RAM cap. |
| Intake waiting bytes | Separate fixed 64 MiB `QUEUE_BYTES`, plus a 4,096-command channel | Why this much waiting inventory? A payload-byte cap does not bound HTTP bodies, active commits, decoded objects or concurrent queries. |
| Worker configuration | `seal_workers=1..16`; automatic `(available_parallelism / 2).clamp(1,4)` | CPU count and a historical memory multiplier are proxies, not measured marginal throughput or a shared admission policy. |
| Batch and commit group | Batch at most 1 MiB; group target 1 MiB, 50 ms maximum window, 2 ms quiet interval | Group size can overshoot its target by the next accepted Batch. Batching delay and sync amortization depend on arrival pattern and number of independent senders. |
| Parquet output | Maximum 8,192 rows per row group; Zstd level 3 | Equal row counts do not imply equal bytes or compression cost. The compression/CPU/read-amplification trade-off needs representative entropy. |
| Collection cadence | 1 s log poll; immediate catch-up when prior delivery caught up and source backlog remains | Idle latency, collection/encoding time, retries and source backlog can dominate freshness before networking starts. |
| Query plan | Default `scan`; configurable `walk` | Small/cold/selective and large/warm/prunable workloads need different startup and per-row economics. |
| Installed resource limits | Contract-frozen systemd limits; separate RSS qualification gates | Cgroup memory includes file cache/kernel charges. Raising a service ceiling does not reduce computational demand. |

Sources: [server config](../../crates/fabric-server/src/config.rs), [intake/commit](../../crates/fabric-server/src/store.rs), [sealer](../../crates/fabric-server/src/sealer.rs), [Segment writer](../../crates/fabric-server/src/segment.rs), [node loop](../../src/bin/fabric-node.rs), [product contract](../PRODUCT-CONTRACT.md).

**Why 64 MiB?** The setting entered with retained history (`aaeea14`); [ADR-0020](../decisions/ADR-0020-store-sealed-history-as-parquet-segments.md) names it as the default. The inspected introduction and decision do not establish an optimum from a parameter sweep. There is an intelligible trade-off: larger files amortize per-file work, while smaller files lower today's whole-file working set and permit earlier reclaim. Neither argument singles out 64 MiB. File rotation is also a threshold rather than a guarantee that every file is exactly that size.

**Why one, two and four workers?** Those were recent screening coordinates, not the supported range. Historical [ingest run 01](../experiments/benchmarks/ingest-run-01.md) also measured three. The recent screen used **512 KiB journal rotations**, even for its 64 MiB aggregate workload. It cannot qualify four simultaneous default-sized builders. Four workers gave 2.24 times the speed at 39% more CPU and 3.67 times incremental heap on that small-file fixture. Threads consume CPU when runnable; their count is neither a reservation of cores nor proof of useful parallelism.

## 3. Mechanisms worth challenging, across responsibilities

| Area | Source evidence or measured signal | Engineering route and its remaining risk |
| --- | --- | --- |
| Collection and representation | Native collection attaches path, device, inode and two offsets as strings to each log. The reader-only screen excludes OTLP assembly. | Measure full native assembly with real path lengths/cardinality. Move ownership instead of cloning; amortize shared metadata only where exact query attributes and stored-byte custody survive. A compact canonical format is a separate format decision. |
| Spool and delivery | `append(&Batch)` clones, then encodes; delivery fetches one Batch, sends, then durably records its ACK. Collection and delivery share the synchronous node loop. | Remove redundant encode/decode/copy passes and consider bounded overlap of collection with delivery. Multiple outstanding Batches require separate ordered-delivery/retry analysis; do not infer that an async rewrite alone fixes the serialized dependency. |
| Intake and commit | HTTP body is copied to a `Vec` before byte admission. Queued accounting is released before the active commit. Sync and grouped waiting contribute to answer wall time. | Account from request-body acquisition through commit completion; retain immutable bytes where lifetimes permit. Profile group fill, wait and sync independently. Preserve durable ACK semantics. |
| Sealing | Whole Groups, cloned raw entries, projected rows and output builders overlap. 64 MiB raw bodies caused 235.55 MiB incremental Rust heap and 1,519.70 MiB cumulative requested allocation. | Stream through leased buffers; remove repeated materializations; separately attribute decode, projection, column construction, compression and publication. Requested allocations are not measured copy traffic. |
| Scheduling/reclaim | Each worker chunk starts scoped threads and joins the whole chunk before admitting another. Reclaim remains an ordered prefix. | Persistent workers with a bounded ready queue can refill idle slots. Prioritize the oldest blocking file and bound work ahead of it. Heterogeneous tasks must demonstrate a benefit; equal-size tests do not measure this barrier cost. |
| Query loading/cache | Walk initializes and extends state under a mutex; releases it before later execution. The short selective-tail fixture favors scan, unlike earlier larger fixtures. | Measure lock hold/wait, cache lifetime and plan crossover. Shared immutable cache snapshots and demand-driven loading may help; do not claim the entire query is mutex-serialized. |
| Query execution/output | Rate queries collect all matching points; sorting repeatedly allocates series keys. `limit+1` bounds selected row count for paginated kinds, not decoder/cache/JSON bytes. HTTP uses `spawn_blocking` without a query-specific byte admission policy. | Correct and precompute rate identity; bounded sort/merge for high cardinality; separate input, operator and output reservations, cancellation and backpressure. A generic thread pool's thread cap does not establish an application memory bound. |
| Recovery and lifetime | Replay validates/decodes stored Batches; caches and allocator-retained capacity can persist after work. Most recent tests reset processes. | Measure warm reuse, restart/replay and retention cycles. A borrowed envelope fast path must reject malformed data identically. Restarting workers or changing allocator may reduce retained RSS without reducing peak demand. |
| Control/fairness | Durable inventory updates rewrite state; node authentication uses the shared control mutex. | Test configuration bursts against data intake and queries. Copy-on-write snapshots/indexing are candidates if contention is measured; preserve persistence and revocation. |
| Observer/compiler/hardware | Detailed hex evidence was much more expensive than hashing in the isolated screen. Historical Callgrind exaggerated SHA cost without native SHA instructions. | Keep evidence formatting outside useful-work spans; profile native CPU stacks, allocations and syscalls first. Test compiler/PGO/SIMD changes only after structural costs, and retain portability and exactness. |

Relevant paths: [collector/runtime](../../src/spindle/runtime.rs), [Spool](../../src/spindle/spool.rs), [Sender](../../src/spindle/sender.rs), [HTTP](../../crates/fabric-server/src/http.rs), [query](../../crates/fabric-server/src/query.rs), [earlier profiling limitations](../experiments/benchmarks/sealer-profile-run-01.md). Full-payload hex in the benchmark is not the same operation as formatting the production sender's 32-byte digest; the measured hex cost must not be assigned to that log line.

### Challenge the replacement design too

[ADR-0022](../decisions/ADR-0022-build-segments-by-external-merge-sort.md) remains a valuable accepted direction. It also explicitly acknowledges that opening every spill run adds a 64 KiB buffer and one row **per run**. Its observed 42–45 MiB prototype peak is not a universal constant-memory proof. Before implementing the stronger bound:

- Cap merge fan-in and file descriptors; use additional merge passes when necessary. Include the added disk traffic and temporary disk peak.
- Bound decoded objects and retained capacities, not just encoded frame bytes or estimated body bytes. Define oversized-record progress and bounded escape behavior.
- Reserve writer/compressor state, all signal tables (including traces added after the design), and sufficient resources to finish an admitted task. Avoid all workers holding input buffers while waiting forever for output reservations.
- Distinguish exact original Batch custody, exact query answers, deterministic output, and byte-identical Parquet. The ADR's opening “changes no bytes” language is broader than its evidence: it explicitly permits earlier row-group cuts and documents changed file hashes. Resolve that acceptance wording before implementation; this audit does not amend it.
- Compare spilling only when necessary against always spilling. On cheap storage with expensive IOPS, lower RAM can cost more CPU, disk traffic and latency.

Processing must have transient memory to compute. Separation of concerns means storage owns custody and reusable buffers while processors borrow bounded working sets; it does not mean processors can use zero RAM. Moving the same live objects to a loader merely changes their label.

## 4. Mathematical and temporal model

Let `F` be journal bytes per file, `v` encoded input bytes/s, `lambda` records/s, `n` active builders, and `d` CPU-seconds per encoded byte for a fixed data shape. A simple file model is:

```text
CPU demand/second ≈ lambda * per-record cost + v * per-byte cost + (v/F) * per-file cost
whole-file builder memory ≈ fixed state + alpha(data shape) * F
bounded merge memory ≈ run budget + k * (reader buffer + maximum resident row)
                       + writer/compressor buffers + decoded-frame overhead
```

The first equation separates fixed per-record, per-byte and per-file costs; avoid double-counting when fitting them. Equal MB/s with different record sizes/cardinality is not the same workload. For `r` spill runs, bounded fan-in `k >= 2` takes approximately `ceil(log_k(r))` merge levels; constant fan-in buys bounded merge residency with additional IO. Row comparison and compression still consume CPU.

Feasibility requires the combined demand of ingress, sealing, queries and control to fit CPU, storage and network supply, with latency headroom. For sealing alone, `v * d < C_available` is necessary, not sufficient; IO and serial publication can impose lower limits. Increasing `n` does not reduce `d`, and it can increase it through allocation, contention and cache misses. The host's four CPU-equivalent quota is not four reserved cores; co-running processes share it.

Global ownership accounting should cover:

```text
M_live = base + HTTP/intake + active commit + leased inputs + operators
         + output buffers + query/cache state + allocator/untracked overhead
D_live = source/spools + journal + spill + growing Segments + retained Segments
```

A buffer shared by reference is counted once, at its owning pool. Cgroup charges additionally include file cache and kernel memory; a userspace pool cannot guarantee that whole-cgroup total by itself. A soft budget decides when to delay, evict or spill; a hard OS limit is a last containment boundary. Proposed adaptive controls need hysteresis, minimum dwell time and reserve capacity for ingress/query/control, not just a CPU percentage threshold.

For a stationary stable queue, Little's law gives `L = lambda * W`. In comparable byte units, an initial queue-sizing estimate is `Q ≈ v * W_target + burst_allowance`. A fixed 64 MiB waiting queue has radically different implications at 1 MiB/s and 100 MiB/s. This estimates average inventory; it does not establish p99 or safe burst size. Under sustained overload, increasing `Q` delays the failure while increasing waiting time.

Measure the entire causal timeline:

```text
source write -> collection -> local durable Spool -> send admission
             -> server durable commit -> ACK receipt -> local durable ACK
                              -> first exact query visibility
```

Record per-event realtime timestamps for correlation and monotonic durations within each host. Cross-host subtraction requires a clock-offset/error estimate. Query visibility is a separate observation; sealing is not required for reading the journal tail. Do not subtract separate p99s to obtain a stage p99. Timeouts, rejected work, retry delay, source backlog and drain time remain in the population; otherwise backpressure makes the measured receiver look fast while the consumer gets stale data.

For low steady activity and an idealized uniform arrival phase, a one-second poll alone gives approximately 0.5 s median and 0.99 s p99 wait. This is analytical, not a new Fabric measurement; catch-up, IO and correlated arrivals change it. The [native scaled run](../experiments/benchmarks/native-scaled-spindle-run-01.md) already demonstrates why collection-to-ingestion and write-to-ingestion tell different stories.

Formal invariants for any faster mechanism remain: ACK implies a durable exact Batch; reclaim implies published exact coverage and durable checkpoint; a complete query cannot omit a possible match; resource acquisition must allow admitted work to finish. These are separate safety, liveness and boundedness obligations. A throughput result cannot establish them.

An adaptive controller should reason about trajectories: backlog growth, oldest unfinished age, estimated drain time, CPU-seconds per useful byte, memory pressure and IO wait. A low instantaneous CPU percentage alone cannot distinguish an idle factory from a stalled one. Begin with an explainable state machine and smoothed measurements; bound each concurrency change, reserve progress capacity and test its response to bursts and workload changes. Forecasts may prepare capacity, but never relax custody or admission invariants. An AI-assisted offline search can propose parameter combinations and counterexamples; held-out workloads and executable checks decide whether those proposals help. Online learning is not a prerequisite for the first efficient implementation.

## 5. Contemporary designs worth learning from

These are primary-source design references, not imported performance claims or recommendations to add a new engine dependency. [Fetch outcomes and content hashes](../experiments/benchmarks/data/performance-refinement-audit/external-sources.json) preserve successful reads and unavailable URLs.

| Reference | Useful mechanism | Fabric-specific caution |
| --- | --- | --- |
| [DuckDB memory management](https://github.com/duckdb/duckdb-web/blob/main/_posts/2024-07-09-memory-management.md) | Chunk-at-a-time execution, intermediate spilling, buffer-manager coordination | Streaming alone does not bound high-cardinality aggregation. Its memory default is not a suitable inherited percentage for a shared Fabric host. |
| [DataFusion memory pools](https://github.com/apache/datafusion/blob/main/datafusion/execution/src/memory_pool/mod.rs) | Operator reservations, release on ownership drop, shared pools across concurrent plans, spill-or-fail handling | Its source explicitly excludes some transient allocations. A reservation API is not automatically a process-RSS cap. |
| [OpenTelemetry Arrow / OTAP](https://github.com/open-telemetry/otel-arrow) | Columnar telemetry, efficient OTLP-byte paths, shared-nothing/thread-per-core design, durable buffering | Especially relevant to repeated attributes and repeated conversions. Lossless data-model conversion does not promise identical original protobuf bytes; Fabric's custody identity is byte-specific. Small Batches may not amortize dictionaries/conversion. |
| [Arrow format](https://arrow.apache.org/docs/format/Columnar.html) | Contiguous columns, dictionary representation, vectorized scans, zero-copy access to compatible layouts | Protobuf-to-Arrow conversion, Parquet decompression and JSON construction still do work. Shared slices may retain an oversized parent buffer. |
| [Seastar](https://github.com/scylladb/seastar/blob/master/doc/tutorial.md) | Per-core ownership, cooperative scheduling, explicit bounded cross-core communication | Borrow locality and small tasks. Dedicated per-core execution and busy polling can be poor defaults for a mostly idle, quota-constrained development host. |

The practical synthesis is a small number of explicit ownership domains, bounded chunks, reused immutable buffers, late materialization, and admission based on bytes plus useful work. It does not require replacing Rust, adding Docker, or introducing Wasm.

## 6. TLS in proportion

**Keep authenticated encryption for remote nodes.** The product currently requires TLS, and the measured approximately 4 ms send-to-durable-answer span contains transport, server queuing and durable commit; it is not a TLS measurement. The Spindle keeps a `ureq::Agent`; a fresh handshake per Batch cannot be assumed. Measure connection reuse, actual handshake frequency, reconnects, crypto CPU and application wait separately.

For a colocated Spindle/server, an optional Unix-domain socket could avoid network framing/TLS at that boundary, but it needs explicit local credential/peer authorization and contract scope. For remote traffic, moving encryption into WireGuard shifts trust and administration; it is not evidence that encryption becomes cheaper. mTLS changes client identity management and is not inherently a speed optimization. [Kernel TLS](https://docs.kernel.org/networking/tls.html) can move the symmetric record data path into the kernel and support `sendfile`; it still needs userspace handshake/key lifecycle support and stack integration. It is a later candidate if profiling shows TLS/copy cost dominates sustained transfer. [HTTP/3](https://www.rfc-editor.org/rfc/rfc9114.html) itself uses TLS 1.3, so QUIC does not remove that layer.

## 7. Prioritized hypotheses and experiment sequence

This is a **draft experiment agenda**, not a new registered protocol or a result. Freeze workloads, seeds, revisions, guard budgets, comparison order, metrics and numerical decision margins before executing performance comparisons. Preserve the current protocols unchanged.

| Priority and perspective | Hypothesis | Null / reason to stop or revise |
| --- | --- | --- |
| P0: formal logic and numerical precision | Structural rate identity and exact integer deltas repair the retained counterexamples; precomputed keys reduce repeated allocation | Counterexamples still fail, another type/reset case regresses, or allocation moves to equally expensive retained state |
| P1: computational representation | Moving immutable bytes and decoding/projecting once reduces CPU per exact record and allocation demand | No robust reduction, excessive parent-buffer retention, or custody/query changes |
| P1: memory/storage algorithm | Bounded runs, bounded fan-in and integrated reservations flatten working set across file sizes | Memory still grows with runs/cardinality, or extra IO makes the required latency/cost envelope worse |
| P2: scheduling and queueing | Continuously refill admitted slots, preserving oldest-prefix progress; select useful concurrency from backlog age and marginal throughput | Added CPU/RAM produces little completed-work gain, hurts query/ingress latency or starves the blocking prefix |
| P2: query economics | Correct rate keys, bounded intermediates, selective loading and measured scan/walk crossover improve mixed consumer latency | First-use/maintenance cost outweighs savings, output dominates, or memory merely moves into caches |
| P2: temporal reasoning | Bounded collection/delivery overlap and deadline-aware batching reduce source-to-queryable age | Receiver timings improve while upstream age, retries or idle CPU rise |
| P3: information density and compression | Amortizing repeated metadata and choosing compression effort by measured entropy lowers combined CPU/storage demand | Small-batch overhead, dictionary cardinality, additional format complexity or query cost erases the gain |
| P3: systems/hardware | IO submission, crypto/transport or code-generation changes improve an attributed bottleneck | Their share is too small, or hardware-specific gains fail portable/cheap deployments |

Run the next work in this order:

1. **Repair and integrate the rate counterexamples.** Add high integer values, delimiter-containing attributes, equal timestamps, resets, mixed numeric types and high-cardinality series to the existing independent-oracle coverage. A faster wrong answer cannot win.
2. **Complete attribution at representative shapes.** Full native collection/encoding; ingress/active-commit memory; exclusive decode/project/Arrow/compress/write/sync spans; query load/execute/serialize; real metadata, traces and metric cardinality. Generate inputs outside timed spans without retaining several fixture copies in the measured process. Record allocated bytes, lifetimes, CPU/record, CPU/encoded-MiB, IO, queue age and exact results together.
3. **Separate granularity from concurrency.** Screen 8/16/32/64/128 MiB journal rotations at one worker with fixed aggregate encoded input and enough files. Then counterbalance 1/2/3/4 workers at representative sizes including 64 MiB; attempt 6/8 only where measured waits and prospective memory/scratch budgets justify it. Test heterogeneous durations and cold/warm paths. Do not run an indiscriminate Cartesian product or infer defaults from the smallest task cut.
4. **Compare mechanisms, one change at a time.** Ownership/copy reduction; bounded builder; continuously refilled scheduling; query key/admission changes. Include traceability from baseline source to candidate and exact independent grades. Combine winners only after their interactions are measured.
5. **Run development, small and moderate composites sequentially.** Real Spindles plus human-style selective queries, agent polling and programmatic broad/rate workloads. Include quiet, burst, catch-up, first-seal, retention and restart phases. Select configuration families from the nondominated latency/CPU/memory/IO results; keep manual overrides. Default selection requires the full pipeline and consumers, not an isolated sealer score.

Use paired, counterbalanced repetitions and an untouched confirmation population; report variation and inconclusive results. Seek sufficient comparable observations before estimating p99, and retain failed/timeout requests. Use coarse screening to select a small confirmation set rather than tuning against noise. The resource guard is chosen from observed free capacity and a prospective whole-run budget, not recycled as a proposed shipping cap.

**On this machine:** correctness/property work, low-overhead ownership probes, bounded native comparisons and the development-to-moderate composite are feasible. Independent source analysis and fixture/oracle preparation can run in parallel; measured CPU/IO/resource runs should get the host sequentially with competing builds stopped. The effective four-CPU quota, overlay filesystem and shared cache limit external validity.

**On the PC:** confirm physical-storage sync/cold-read behavior, cache and allocator lifetime over longer runs, real cgroup/service enforcement, representative CPU instructions/core topology and larger concurrency. Use another host or controlled network path for cross-host clocks, RTT/loss and connection behavior; a single PC does not recreate a fleet. Measure power or use actual prices before claiming monetary/energy savings.

## 8. Audit limits and validation

The recent responsibility measurements are synthetic, mostly small, with several unstable populations; they did not include complete native OTLP assembly, default-sized concurrent builders, high-cardinality rates, all trace shapes, concurrent consumer admission, true cold-device IO or long-running memory retention. The existing 1,920 graded answers remain valid for their fixtures; the new rate counterexamples show why that finite coverage cannot establish universal exactness.

Documentation also needs careful provenance: the top of [delivery ownership](../architecture/delivery.md) describes the old local demo, while its later section describes real networking; that network paragraph omits the current 2 ms quiet-close rule. The bounded-sealer ADR predates trace-table integration and the streaming writer candidate. Those discrepancies are recorded here, without silently rewriting accepted decisions or treating old cost assumptions as current measurements.

The preserved rate diagnostic reproduces three mismatches and leaves production unchanged. Its checked-in reproducer exited **1** with those same three mismatches; `bun tools/docs/check.mjs` and `git diff --check` exited **0**. Rust 1.98.0 and Python 3.12.14 were used; the standalone Cargo lock is retained. No new throughput/latency comparison or target-host qualification ran, and the full fast profile was not rerun for this audit-only change. The earlier fast profile remains **19 passed, one existing Clippy failure** at the revision recorded in the responsibility results; this audit does not turn that into a clean product check.
