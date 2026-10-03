# Medium and enterprise volume through real Spindles, local run 01

Status: **medium met all finite pilot criteria; enterprise recovered all data but failed generator-lag and latency criteria**. Twenty actual `fabric-node` processes forwarded both workloads. This is a short aggregate-volume experiment on one constrained host, not a 200/2,000-host deployment or sustainable enterprise-capacity result. It extends the [earlier simulation](scaled-fleet-local-run-01.md) by exercising application-file collection and durable native Spools.

[Protocol revision 1](native-scaled-spindle-protocol.md), committed `4de12f1`; medium harness `d837707`. The first enterprise attempt stopped on a **monitor defect**, a Spool rename between enumeration and stat. Its [failure and trace](data/native-scaled-spindle-run-01/interrupted-enterprise/run.txt) remain recorded; it has no capacity verdict. [Retry protocol revision 2](native-scaled-spindle-protocol-r2.md), committed `3a98178`, preceded the corrected enterprise harness `9eb15a7`. No runtime, workload, limit or acceptance threshold changed for the retry. All trials ran sequentially with fresh state.

## What ran

```text
native app-file producer -> 20 real Spindles -> durable FAB1 Spools
 -> native TLS Sender -> server durable journal -> Parquet Segments
 -> stopped server -> fresh replay -> independent OTLP/source-body audit
```

The [producer](../../../tools/bench/native_source.rs) only appends log lines; it constructs no Fabric Batch and sends no network request. The [runner](../../../tools/bench/run_native_scaled.py) enrolls nodes, observes native stdout and resources, and uses the independent Python decoder plus a disk-backed SQLite audit to compare every recovered body against every original file line. Exact ACK/recovered Batch SHA-256 maps and contiguous sequences were checked. No gaps, duplicate/missing/altered/unexpected logs or missing/altered ACK custody were observed in either completed run. This is not the full delivery-oracle transcript, a physical power-loss test or a query test.

Each workload scheduled 10 seconds ordinary, five seconds 3× burst, ten seconds ordinary recovery. Bodies: 900-byte tagged ASCII, half repeated/half deterministic printable entropy, seed 2703163393. Nodes also collect native host metrics. Twenty nodes share 10,000 / 100,000 ordinary logs/s and 30,000 / 300,000 burst logs/s. Producer lateness means the latter is a **scheduled**, rather than maintained, burst rate. Enterprise burst writes actually completed from 10.04 through 17.02 seconds; recovery writes from 17.02 through 24.93. All planned records were written, with overdue ticks caught up.

Frozen release server/node/replay binaries were reused from [native dev/small](dev-small-local-run-01.md), runtime source `c42f187`, including experimental streaming Segment output and earlier reclaim. Their exact hashes match both runs' environment files. The new source feeder was built with Rust 1.98 `rustc -O`. Linux x86_64, kernel 6.18.44, glibc 2.41, overlayfs; four CPU equivalents in a shared 16 GiB cgroup, five affinity-visible CPUs. Server uses CPUs 0–1, nodes/producer 2–3; one sealer, 64 MiB journal files, 1 GiB journal limit. Each Spool ceiling is 256 MiB. Address-space bounds: server 4 GiB, each node/producer 512 MiB. These are not RSS/cgroup service budgets or fixed architectural memory requirements.

## Outcomes

[Medium summary](data/native-scaled-spindle-run-01/medium/summary.json), [enterprise summary](data/native-scaled-spindle-run-01/enterprise/summary.json). All latency entries are record-weighted p50 / p99, milliseconds. Collection time is native observed time; ingestion is persisted server receive time **before sync**, ACK is node stdout observation. File-write time is post-write observation, and scheduled time includes generator delay.

| Dimension | Medium | Enterprise retry |
| --- | ---: | ---: |
| Exact unique source logs recovered | 350,000 / 350,000 | 3,500,000 / 3,500,000 |
| Exact recovered/ACKed Batches | 692 | 4,597 |
| Collection → ingestion, ms | 15.58 / 102.99 | 66.56 / 1,854.64 |
| Collection → ACK observation, ms | 22.20 / 126.70 | 79.63 / 1,865.86 |
| File write → ingestion, ms | 527.97 / 1,013.62 | 2,241.75 / 21,314.51 |
| Scheduled write → ingestion, ms | 530.84 / 1,021.85 | 4,151.09 / 21,360.38 |
| File write → collection, ms, computed directly | 506.63 / 991.38 | 2,085.28 / 21,176.34 |
| Producer completion-lag p99, ms, node/tick weighted | 15.56 | 2,685.96 |
| Additional drain after producer, s | 1.00 | 21.02 |
| `unavailable` retry attempts | 0 | 837 |
| Peak server RSS, MiB | 470.39 | 521.79 |
| Largest individual Spindle RSS, MiB | 9.24 | 9.13 |
| Peak aggregate Spindle RSS, MiB | 149.45 | 155.97 |
| Peak producer RSS, MiB | 1.26 | 2.66 |
| Server / all-node CPU equivalents during scheduled window | 0.259 / 0.175 | 1.261 / 1.026 |
| Server / all-node CPU equivalents over monitored phase | 0.127 / 0.090 | 0.980 / 0.588 |
| Largest aggregate reported unread-source inventory, MiB | 7.07 | 1,046.54 |
| Largest individual reported Spool inventory, MiB | 9.21 | 9.55 |
| Peak aggregate Spool files on disk, MiB | 161.91 | 124.05 |
| Largest sampled sealed-file queue | 1 | 15 |
| Final published Segments / sealed files waiting | 5 / 0 | 58 / 0 |
| Encoded Batch bytes per source log, including metrics | 1,123.27 | 1,127.89 |
| Final retained Segment tree, MiB | 220.29 | 2,576.50 |
| Peak timed logical disk inventory, GiB | 0.66 | 5.87 |

CPU is cumulative process CPU divided by elapsed wall time, **not CPUs reserved**. Scheduled-window samples cover about 24.24 / 24.88 seconds; whole monitored windows cover 50.40 / 70.65 seconds and include settling, drain and quiet time. In particular, do not use the lower whole-run averages as burst demand. Sampled inventory and CPU windows are approximate. Spool disk occupancy includes retained acknowledged data until whole-file reclamation; it is not solely undelivered work.

Medium passed every registered gate. Enterprise failed completion-lag p99 <=100 ms, collection-to-ingestion and ACK p99 <=1 s, and source-to-ingestion p99 <=2 s. Custody, clean exit, contiguous sequences, 120-second drain, process-RSS thresholds and clock-offset thresholds passed. These are conjunctive finite criteria: preserved data and restrained RAM do not turn a latency failure into a pass.

## Rotating the interpretation

**Precision and durability.** Every original body was compared against fresh durable replay, not regenerated from server output. Exact bytes and tags survived both completed workloads. The [post-hoc analyzer](../../../tools/bench/analyze_native_scaled.py) separately recomputed counted-clock percentiles and exact ACK hash maps from retained evidence. Post-hoc rejecting controls also rejected an altered reported percentile and recovered ACK hash. Revision-1's pretrial controls were shallow SHA examples; revision 2 exercises the actual SQLite comparison helper accepting identical bytes and rejecting changed/missing entries, plus a deterministic disappearing-file monitor regression. No control demonstrates arbitrary fault tolerance.

**Queueing and latency.** In Satisfactory terms, source files are ore waiting beside the miner, Spools are durable intermediate containers, and the server journal is the buffer before refining into Segments. Enterprise reached about 1,023.91 MiB of journal files with 15 files awaiting sealing, and unread source inventory reached 1,046.54 MiB. Native forwarding/backpressure allowed backlog to remain before collection. Collection-to-ingestion alone hides most of the 21-second source-to-ingestion tail. The source-to-collection percentile above is computed from its own distribution; subtracting latency percentiles would be invalid. Small negative source-to-collection differences (641 / 1,815 records) remain visible: a collection cycle's observation timestamp can precede the post-write stamp while collection reads during the write. No primary ingestion-clock inversions occurred; same-host realtime-minus-monotonic range was about 0.0010 / 0.0012 ms.

**Computation and concurrency.** Enterprise's scheduled source demand exceeded this configuration's ability to finish work promptly. The producer itself also lagged, so this trial cannot isolate server capacity or claim an achieved sustained 300k/s burst. The queue and retry evidence is consistent with server journal/sealing backpressure; it does not prove a unique CPU or disk bottleneck. The earlier synthetic many-identity trial has different node count, source path, encoded bytes and queue ownership, so its lower/higher percentiles are not a causal improvement comparison. Demand-driven worker admission remains a separate hypothesis, not an implemented consequence of this result.

**Memory and storage ownership.** Increasing volume tenfold raised peak server RSS only about 11%, and left node RSS essentially unchanged in this finite configuration. Queues moved into disk-backed sources and journal rather than an in-memory simulated fleet. That is useful restraint, but not free: original app files must remain available until collected; rotation/deletion during backlog was not tested. Processing still owns decoded whole-file builder/Arrow working sets; the streaming output candidate did not implement a bounded loader/processor. Server RSS remained 303.49 / 341.26 MiB at final sampling after starting near 8 MiB. This is not a memory plateau or solved allocator problem.

**Host budget and cheap resources.** Shared cgroup memory ranged 10.32–11.34 GiB in medium and 11.55–16.00 GiB in enterprise, including pre-existing cache, page cache from app/journal/Segment files, kernel charges and harness processes. Process RSS alone would miss that pressure. No per-category timed `memory.stat` or pretrial memory-event baseline was captured, so no OOM, cache attribution or isolated deployment budget claim follows. The server's `/proc/io` reports zero `read_bytes` but substantial buffered reads; this is not a cold-storage benchmark. Server charged writes were about 601 / 6,373 MiB. Logical file size, cache demand and physical device I/O/cost are different dimensions. No dollar-optimal deployment is established.

## Next optimization routes

| Route/hypothesis | Null/counterexample | Cheapest useful next experiment |
| --- | --- | --- |
| Quicker demand-aware collection reduces medium source latency | More wakeups/CPU/IO without meaningful p99 improvement, or altered bytes/gaps | Freeze native medium workload; vary polling/admission only; keep custody and CPU costs |
| More sealing capacity reduces enterprise source backlog | Extra workers inflate RAM without reducing queue/tail, or producer limitation dominates | Static 1 vs 2 workers with identical replayable offered-write timing; then independently confirm |
| Storage-owned bounded loading limits processing working set | Larger files/workers still inflate RSS, or added merge I/O erases speed benefit | Instrument builder/allocator/cache separately, then bounded-builder prototype under registered acceptance |
| Host cache/storage pressure materially changes capacity | Similar results after isolating source and cache state | On the PC, separate producer/Spindles from server; measure filesystem/device IO and timed cgroup categories |

Current cloud can test small controlled mechanisms and native correctness. The PC is useful for sustained/soak runs, cold and warm real-storage comparisons, realistic many-node/WAN arrangements, configurable service budgets, and producer/server isolation. Freeze binaries, schedule, seed, precision checks and clocks when transferring. Cross-host dual clocks require measured synchronization uncertainty; this same-host trial provides none.

## Reproduction and verification

```sh
rustc -O tools/bench/native_source.rs -o /workspace/scratch/native-scaled-frozen/native_source
timeout 2400 python3 -B tools/bench/run_native_scaled.py \
  --bin-dir /workspace/scratch/native-scaled-frozen \
  --out /workspace/scratch/native-scaled-run-01
timeout 1200 python3 -B tools/bench/run_native_scaled.py \
  --bin-dir /workspace/scratch/native-scaled-frozen \
  --out /workspace/scratch/native-scaled-run-02 --tier enterprise
python3 -B tools/bench/analyze_native_scaled.py /workspace/scratch/native-scaled-run-01/medium
python3 -B tools/bench/analyze_native_scaled.py /workspace/scratch/native-scaled-run-02/enterprise
```

First runner command used historical `d837707`: exit 1 after completed medium and interrupted enterprise. Retry used `9eb15a7`: exit 0, **enterprise hypothesis still failed**. A successful runner exit means analysis completed, not that acceptance criteria passed. Current runner supports both tiers with the monitor fix. Source/runtime build provenance is linked above; [medium environment](data/native-scaled-spindle-run-01/medium/environment.json), [enterprise environment](data/native-scaled-spindle-run-01/enterprise/environment.json). Retained evidence is under 2 MiB per completed tier, well below 128 MiB. Original application files, native state and SQLite audits remain in owned scratch directories; private keys and credentials are excluded from repository evidence. Temporary replay was deleted only after completed audit. Analysis completed in about 63.47 / 197.59 seconds from trial directory creation, within each protocol's 1,200-second bound.

Fast repository and documentation checks are recorded in the evidence README. These results remain local on consolidated `milestone/streaming-segment-output`; no remote branch publication or main merge occurred. No Docker, new runtime dependency, controller, wire-format change, query qualification or product capacity promise was introduced.
