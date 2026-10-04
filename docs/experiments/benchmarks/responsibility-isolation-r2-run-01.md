# Resource-aware responsibility screen, revision 2 run 01

**Measured, finite exploratory screen; exact checks passed for the completed revised campaign. No product/default selection or target-host qualification.** At source `a717c33`, [protocol revision 2](responsibility-isolation-protocol-r2.md) produced 58 corrected preflight trials, 20 calibration trials and **290 measured trials over 29 cases**, sequentially. Main campaign took **699.28 s**, graded **1,920 actual query answers** independently and removed **21.25 GiB cumulative logical scratch**. Every owned trial was cleaned; **235.95 MiB main evidence** remains. The earlier failed preflight is preserved separately, not counted as a pass. The later development/small/moderate composite has not started.

## What was actually exercised

[Native probe](../../../crates/fabric-server/examples/responsibility_probe.rs), [sequential runner](../../../tools/bench/run_responsibility_isolation.py), [descriptive aggregation](../../../tools/bench/summarize_responsibility_isolation.py). Five fresh-process plain/counted pairs per case; minimal observation for main timing. Native file reader, Spool, Sender with real loopback TLS/durable server, journal commit/recovery, row decoder/sorter, Segment builder/recovery, History scan/walk, persistent control and actual sealer/reclaim pass. Sorted/reversed/seeded-shuffled timestamps; 128/900/3,500-byte bodies; 16/64 MiB raw body processing fixtures; 1/20/200-node control inventories; 1/2/4 sealers.

Full [raw evidence and aggregate](data/responsibility-isolation-r2-run-01/aggregate.json), [phase completion](data/responsibility-isolation-r2-run-01/measurement/complete.json), [commands/build settings/exits](data/responsibility-isolation-r2-run-01/reproduction.json) and per-trial argv/env/hashes/resources/cleanup are retained. Source Batch records and actual answers are retained for first plain repetitions; **all** answers were graded before deletion, with verdicts/digests retained for every trial. Unchanged independent Python query/Batch decoder and exact source-body/ACK custody checks rejected changed, missing and duplicate records. Native Segment recovery was also checked against original exact Batch bytes. This is synthetic deterministic text, not a real application corpus.

## Responsibility costs

Figures are medians of five plain-process per-trial medians. Heap is median counted-process incremental Rust allocation peak; it is a separate population. Most boundaries have fewer than 1,000 observations, so their p99 is null/insufficient. Inclusive and separately measured cuts cannot be added or subtracted into an invented pipeline profile.

| Responsibility / 900-byte baseline | Useful boundary | Time | Incremental Rust heap | Interpretation |
| --- | --- | --- | --- | --- |
| Collection | File read, validation and cursor advance | 2.43 ms/call; 123.42 ms for ten full scans of 4,096 lines | 0.96 MiB/call | Cached reader cut; excludes native OTLP collection assembly and downstream delivery |
| Host observation | Native host sample | 0.039 ms | 0.026 MiB | Small local sampling cost, not fleet telemetry overhead |
| Delivery | Spool fetch/decode; TLS send through durable answer; local durable ACK | 0.025 / 4.02 / 0.795 ms per 128-log Batch | 0.34 / 0.59 / 0.11 MiB | Server wait/commit belongs in send wall time; probe CPU alone is not combined client/server CPU |
| Storage | Spool append inclusive encode/sync; server submit-to-durable-answer | 1.19 / 3.65 ms per Batch | 0.25 / 0.58 MiB | Spool append CPU 9.75 ms versus 39.37 ms wall over 32 calls; journal submission CPU 18.73 versus 126.37 ms wall |
| Recovery | Server open/replay, 4,096 logs | 9.03 ms | 3.82 MiB | Includes native decoding/recovery, buffered local IO |
| Processing | Full Segment build/publication, 4,096 logs | 44.03 ms | 21.24 MiB | Inclusive materialization/compression/output/sync/publication |
| Query | Tail scan broad, warm | 16.57 ms | 20.52 MiB | Actual 4,096-row answer; serialization separately measured |
| Control | Durable update at 20 / 200 nodes | 0.552 / 0.704 ms | 0.012 / 0.096 MiB | Full inventory rewrite; total enrollment cost grows with inventory size |
| Self-observation | SHA / hex of one encoded 128-log Batch | 0.065 / 3.66 ms | <0.001 / 0.25 MiB | Hex evidence formatting is about 56× SHA time on this cut; keep it outside useful-work spans |

Storage wall/CPU divergence is consistent with waiting, grouped-commit timing and filesystem synchronization. It does not isolate physical sync latency or prove a particular device bottleneck. Delivery records native server tick-CPU separately; at 900 bytes its median whole-probe interval added about 0.02 server CPU s, with 10-ms tick resolution.

## Memory and computational perspectives

| Shuffled processing fixture | Full build wall / CPU | Incremental heap | Cumulative requested allocations | Whole plain-process VmHWM |
| --- | --- | --- | --- | --- |
| 16 MiB bodies, 16,384 records | 156.82 / 151.74 ms | 64.47 MiB | 381.13 MiB | 137.09 MiB |
| 64 MiB bodies, 65,536 records | 638.45 / 628.37 ms | 235.55 MiB | 1,519.70 MiB | 493.22 MiB |

Four times the input produced about 3.65 times incremental heap: this supports input-dependent materialization demand. It does not establish a worst-case multiplier for attributes, metrics, spans or other entropy/cardinality. At 64 MiB, the builder made approximately **23.75 input-bytes' worth of requested allocation per input byte**. That is allocator traffic, not measured physical copies or simultaneous live memory; C-library allocations are not all captured by the Rust allocator.

The 64 MiB counted build starts with about **309.20 MiB live fixture state**. Whole-process RSS includes this benchmark fixture, previous parsing/sort work and allocator retention. Do not turn 493 MiB into a server deployment cap or multiply it by worker count. Component heap, whole-process RSS and cgroup cache are different inventories. Merely transferring nominal ownership to a loader would not eliminate any allocation or retained lifetime.

At 4,096 rows, standalone sort median was **0.0078 ms ordered, 0.0246 reversed and 0.4284 shuffled**. Disorder matters to sorting, but full builds stayed around 42–44 ms. At 65,536 rows, standalone shuffled sort was 12.00 ms versus 638.45 ms full build; these are separate cuts, not an exclusive phase decomposition. Sorting alone is therefore a weak first optimization target for these bodies. The full large build's CPU/wall ratio is about 0.98: materialization/output processing CPU deserves finer attribution before another loader thread is assumed beneficial.

## Threading and granularity perspectives

These native passes seal **64 MiB aggregate raw bodies split into 512 KiB journal-file cuts**, not multiple default-sized 64 MiB journal files. Fixture/recovery state contributes about 374.46 MiB live baseline; incremental allocation measures work added by the pass.

| Workers | Pass wall median (range) | Total process CPU | Mean CPU equivalents | Incremental heap |
| --- | --- | --- | --- | --- |
| 1 | 1,448.85 ms (1,377–1,660) | 1,048.90 ms | 0.72 | 4.70 MiB |
| 2 | 1,021.61 ms (970–1,064) | 1,349.66 ms | 1.32 | 8.89 MiB |
| 4 | 646.46 ms (576–731) | 1,458.79 ms | 2.26 | 17.27 MiB |

Four workers completed about **2.24× faster than one**, at approximately **39% more total CPU** and **3.67× incremental heap**. Two workers reduced elapsed time about 29.5% but added about 28.7% CPU. Four versus two reduced time another 36.7% for about 8.1% additional CPU. These are observed block-ordered screening comparisons, not randomized confirmation or adaptive-manifold implementation. They support testing useful concurrency, not selecting four workers as a universal default.

Smaller journal tasks show one way around working-set pressure, but they also create more Segments, checkpoint/sync work and metadata. A whole 64 MiB constructed Segment built in about 638 ms while the one-worker small-file native pass took 1,449 ms; their boundaries/layouts differ, so this is a prompt for a controlled granularity experiment, not a speedup claim. The accepted bounded external-merge builder remains the stronger route to preserving coarse files without whole-file resident rows.

## Query loading, processing and output

At 900-byte bodies, tail walk selective first/warm medians were **23.22 / 4.14 ms**, showing index/cache initialization cost. But tail **scan** selective first/warm was **2.24 / 2.36 ms** on this 4,096-row fixture; warm walk selective was unstable under the registered >20% CV rule. A cache speedup against itself is not a win against the other plan. Warm broad tail walk was 13.85 ms versus scan 16.57 ms, but first broad walk was 35.00 versus scan 17.17 ms. Several scan populations are also unstable; retain every trial and range.

Segment broad warm scan/walk medians were 13.11 / 12.33 ms. Broad answer serialization typically added several ms; the 4,096-result/900-byte answer is materially different from one selective row. This supports separate accounting for source/index loading, query execution and result construction/serialization, with shared buffer/cache ownership and admission still needing design. It does not prove a new threaded query pipeline is beneficial. First calls reset History objects; OS cache remains buffered/warm.

## Resource constraints: ceiling versus engineering signal

The host exposes five affinity CPUs under **four CPU equivalents** and a **16 GiB shared cgroup**. Main before-charge was about 12.20 GiB. End-of-trial file-cache observations reached about 12.33 GiB; anonymous memory end observations reached about 0.35 GiB. These snapshots occur after probe exit and are not timed cgroup peaks or owned-process attribution. Existing `memory.events max=11383` predates this campaign; summed recorded trial deltas for max/OOM/OOM-kill and CPU throttling were zero. Do not claim the host limits were binding or that their absence at snapshots guarantees no transient pressure.

| Constraint class | What it means | Route to test |
| --- | --- | --- |
| Host CPU quota / actual device and RAM | A real supply limit at a fixed host allocation | Reduce CPU/byte demand, improve useful overlap, or move a hardware-specific capacity test to the PC |
| Service MemoryMax, journal/file sizes, worker count, output rate | Configurable policy and operating coordinate | Measure latency, backlog and memory jointly; changing a cap moves the bottleneck and may only postpone it |
| Spindle configured Spool validator maximum 256 MiB | Code-enforced configuration range | A deliberate validated code/protocol change would be needed; editing a larger setting is insufficient |
| Exclusive journal ownership, exact bytes, durable ACK and publication-before-reclaim | Correctness constraint | Change lifecycle/scheduling while preserving the invariant; do not bypass the lock or skip sync |
| Whole-file rows/copies, first-query caches, per-group join barriers | Demand introduced by today's implementation | Bounded merge/buffer leases; query plan/admission/cache strategy; ready-task scheduling with byte admission |
| Shared file cache and historical data | Host accounting/context | Observe category/event deltas; retain safety margin and avoid deleting unrelated state |

In Satisfactory terms: the CPU quota is the power available to the factory. A cap is a configured container/belt allowance. Large resident inventories and duplicated buffers are material left beside every constructor. Splitting work lowers inventory but can add lots of small freight shipments. Additional constructors consume actual CPU while working; four workers here averaged 2.26 CPU equivalents, they did not claim four CPUs continuously. Optimize useful records completed with exact custody and acceptable queue age, not machine count or lower RSS achieved by leaving ore upstream.

## Calibration, failures and remaining work

[Calibration](data/responsibility-isolation-r2-run-01/calibration/calibration-report.json): long-batch median ratios were counted allocator 1.0158, 500-ms sampler 1.0032 and detailed/100-ms 1.0051; none met the registered 10% consistent-direction screen. Plain long-batch CV was 0.23%. Median empty wall span 20.5 ns gives an exploratory 2.05-microsecond floor at 1%; process CPU counter access is itself about a few hundred ns and tiny control calls are observer-sensitive. This calibration does not establish every instrumented primitive has the same overhead. No observer cost was subtracted. [Aggregate](data/responsibility-isolation-r2-run-01/aggregate.json) marks all >20%-CV cases unstable; no slow trials were discarded or rerun until favorable.

[Fixture counterexamples](responsibility-fixture-regressions.md) preserve the original missing active journal and the revised preflight's double-owned journal `WouldBlock`. Preflight 01 stopped after 44 completed trials, before calibration/main, and cleaned the failed trial too. Moving replay before opening the writer corrected fixture lifecycle; unchanged native production storage enforced its existing lock.

Next screening routes: (1) bounded external merge/explicit buffer lifetimes, with exact signal/custody output; (2) controlled journal granularity versus memory/CPU/metadata; (3) counterbalanced worker counts with heterogeneous files and slot occupancy; (4) query first/warm loading, scan/walk choice, cache/answer byte admission and serialized output. Register baseline/candidate/decision margins before each implementation comparison. Allocation traffic suggests opportunity, not proof that a particular copy can be removed safely.

Coverage still missing: full native collection+OTLP assembly and multiple paths/invalid-source cases; controlled throttle/retry interactions; isolated write/sync timings; per-phase compression/Arrow/copy attribution; metrics/rates/reset/cardinality and traces; mixed histories/pagination; concurrent HTTP/CLI/agent consumers; long-run retained memory and heterogeneous-worker barriers. This is a completed **bounded screen**, not thorough coverage of every mechanism. These extensions and selected interactions precede the later native-Spindle three-tier composite with original write/collection/receive/query clocks. No new end-to-end p99 or deployment requirements are inferred from engine timings.

On this cloud host we can develop byte-exact algorithms, ownership/admission models and bounded synthetic native comparisons. The PC is needed for representative physical-storage sync/cold-read behavior, longer lifecycle tests and hardware-specific capacity/cap enforcement; multiple default 64 MiB builders require a prospective aggregate-memory budget and a lighter fixture harness. A PC alone still does not reproduce fleet network topology. Native processes suffice; Docker and Wasm are not dependencies.

## Verification

Main, corrected preflight and calibration commands exited 0; initial revised preflight exited 1, preserved. Release plain/counted builds exited 0; unused-import warnings in initial builds were corrected before final frozen binaries. Initial fast checks exited 1: 16 passed, existing Rust-1.98 Clippy failure and three missing-Bun environmental results. Bun documentation/checker/hook checks then exited 0 with its installed path. [Final fast checks](data/responsibility-isolation-r2-run-01/final-fast-checks.txt.gz) exited 1: **19 checks passed; Clippy failed only at the existing `fabric-observation/src/crc32.rs:81` Rust-1.98 lint**. [Receipts](data/responsibility-isolation-r2-run-01/verification-receipts/clippy.json) retain actual candidate revision, toolchain, output hash and exits. Final documentation check and `git diff --check` exited 0; Python runner/reporter compile checks and aggregation exited 0. This is not a full fast-profile pass. Receipt revision is the source revision at check launch; final result prose has a separate documentation check.

An additional package-specific probe Clippy command (`cargo clippy --offline -p fabric-server --example responsibility_probe --features responsibility-alloc-probe --no-deps -- -D warnings`) exited 101 at the unchanged `fabric-server/src/text_filter.rs:153` instance of the same Rust-1.98 lint; [output](data/responsibility-isolation-r2-run-01/probe-clippy.txt) is preserved. Neither lint was suppressed or rewritten as part of this experiment.

After committing the archive, all four original revision-2 evidence directories were checksum-compared against the repository copies, then their redundant copies were removed (about 243.45 MiB). Frozen executables/build logs and all historical runs were retained. [Archive cleanup receipts](data/responsibility-isolation-r2-run-01/archive-cleanup.json) record exact bytes and free space; retained results remain in Git.
