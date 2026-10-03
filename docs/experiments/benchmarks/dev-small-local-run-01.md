# Sequential native development and small deployments, local run 01

Status: **both registered pilot workloads completed and met their criteria**. These are two sequential native trials on the current constrained cloud host, not deployment qualification, a soak, or a comparison of storage writers. [Protocol](dev-small-local-protocol.md); [runner](../../../tools/bench/run_dev_small.py); [environment and frozen hashes](data/dev-small-local-run-01/environment.json). Source/harness revision `c42f187` descends from sizing `1740a60` and registration `3fd695b`; runtime includes the experimental streaming output sink and earlier reclaim candidate.

## Outcomes

Each trial offered 60 seconds ordinary traffic, 60 seconds 3× traffic, then 60 seconds ordinary recovery, plus settling/drain. Development ran first with one real Spindle at 10/30/10 logs/s; it fully stopped and replayed before small started. Small used 20 real Spindles at aggregate 1,000/3,000/1,000 logs/s. Every Spindle used its durable disk Spool and native collection, TLS sender and ACK handling; the server used one sealing worker. Metrics were collected every 15 seconds; no spans or concurrent queries were generated.

| Metric | Development | Small |
| --- | ---: | ---: |
| Exact source logs recovered / offered | 3,000 / 3,000 | 300,000 / 300,000 |
| ACKed Batches recovered, exact printed SHA-256 matched | 183 / 183 | 3,647 / 3,647 |
| Collection to persisted ingestion p50 / p99, ms | 5.06 / 8.89 | 6.19 / 50.65 |
| Collection to durable ACK stdout observation p50 / p99, ms | 7.46 / 12.67 | 9.10 / 57.46 |
| Source-file flush to ingestion p50 / p99, ms | 413.85 / 910.31 | 507.98 / 997.99 |
| Native attempt RTT p50 / p99, ms (Batch-weighted) | 4.70 / 7.34 | 5.26 / 13.51 |
| Server peak process RSS, MiB | 7.91 | 440.41 |
| Largest individual Spindle peak RSS, MiB | 4.47 | 5.38 |
| Peak aggregate Spindle RSS, MiB (same-time sample) | 4.47 | 98.63 |
| Server mean CPU equivalents | 0.00118 | 0.03501 |
| All Spindles mean CPU equivalents | 0.00147 | 0.03350 |
| Published Segments at final resource sample | 0 | 5 |
| Largest sampled sealed-file backlog | 0 | 1 |
| Native non-ACK attempts | 0 | 0 |
| Encoded Batch bytes / source log, including metrics | 1,172.03 | 1,132.14 |
| Peak sampled logical trial disk, MiB | 9.46 | 694.34 |

All registered gates were true: exact source body hashes and unique identities, no collection gaps, contiguous recovered sequences with no duplicate Batches, exact ACK/recovery hashes/counts, zero child exits, overall collection-to-ingestion and collection-to-ACK p99 <=1 second, no negative primary clock differences, server RSS <=2 GiB, every node RSS <=64 MiB, and sampled clock-offset range <=5 ms. This supports H1 **for these finite local trials**; it does not statistically reject a general deployment null. Complete gate populations and values are in the [development summary](data/dev-small-local-run-01/development/summary.json) and [small summary](data/dev-small-local-run-01/small/summary.json).

## Reading the two clocks

Collection is the native OTLP `observed_time_unix_nano` recorded by the Spindle's collection cycle. Ingestion is the server's persisted `received_ns`, recovered with the exact Batch from journal/Segments after graceful shutdown. Their difference is calculated for each uniquely tagged source record using the independent Python query-oracle decoder, rather than matching records by replay position. Persisted receive time is taken before the commit group's syncs. **It identifies ingestion into that group; the ACK follows its durable commit.** ACK companion timings use the harness's wall-clock observation of the node's stdout, which adds scheduler/pipe delay.

The source-file flush companion includes waiting for the one-second log poll. Thus a 6 ms collection-to-ingestion median does not mean an application's freshly written line arrives within 6 ms: small's flush-to-ingestion median was about 508 ms. Small's maximum flush-to-ingestion delay was 1,100 ms; that companion had no registered <=1-second gate. Collection-to-ingestion maximum was 115 ms; collection-to-ACK observation maximum was 129 ms. No population was trimmed or clock inversion clamped.

Both endpoints share `CLOCK_REALTIME` on one host. Sampled realtime-minus-monotonic offset range was 0.00073 ms for development and 0.00122 ms for small, with no detected >5 ms step at one-second sampling. This does not certify sub-millisecond absolute accuracy or distributed clock synchronization. Native RTT uses the node's monotonic duration. Primary percentiles are nearest rank, record-weighted, and have many correlated samples sharing a Batch timestamp. RTT is Batch-weighted and answers a different question.

Small's collection-to-ingestion p50 / p99 by source phase, milliseconds:

| Ordinary 60 s | Burst 60 s | Recovery 60 s |
| --- | --- | --- |
| 5.48 / 13.94 | 6.69 / 73.65 | 5.30 / 12.53 |

The tail grew during burst and returned toward ordinary values. The maximum native reported per-node log backlog was 606 bytes, no sends were refused, and all offered records were recovered after the drain. Final server journal volume was about 3.57 MiB in small and 3.36 MiB in development. Five small Segments cover the sealed portion; no sealed files remained at the final sample. Durable replay also verified the unsealed tail.

## Resource and memory-budget interpretation

CPU equivalents are CPU seconds divided by sampled wall duration (about 204 seconds each, including settling/drain). Small's server used **3.50% of one CPU on average**, its 20 Spindles together **3.35% of one CPU**. These are averages over burst/ordinary/idle windows, not instantaneous peak CPU or percentages of all host CPUs. The producer and verification process are excluded. Two affinity CPUs were available to the server; a worker did not reserve either one. This workload supplies too little processing demand to justify more workers on utilization alone.

Development's low server RSS is **before its first seal**: it does not validate that 8 MiB is enough for the process's lifetime. Small exercised the 64 MiB whole-file builder five times and peaked at about 440 MiB RSS. This is the expensive processing inventory seen in the [earlier writer pilot](streaming-output-local-run-01.md), now beside real collection and delivery. One short run does not establish a stable allocator plateau or resolve the historical failed soak.

The owner's additional constraint is that **memory capacity is a configurable deployment dimension, not an architectural 3 GiB ceiling**. The installed profile's systemd `MemoryMax` is already a setting with a frozen default; no product guarantee follows from it. This experiment changed neither that profile nor source behavior, and ran native processes without systemd. Treat measured working-set demand, available process/cgroup budget, worker admission and safety margin as separate quantities. The proposed controller can use an explicit operator budget instead of a CPU-count proxy; implementation and calibration remain future work. Raising a budget may allow concurrency, but cannot by itself improve a pipeline limited by disk/CPU. Lowering it needs bounded builders/admission or can cause allocation failure/OOM. RSS also omits cgroup page-cache and kernel charges, so a measured 440 MiB RSS is not a safe 440 MiB cgroup setting.

The [sizing calculator](../../research/workload-sizing.md) used 1,024 encoded bytes/event. Actual native Batch volume was about 14% higher in development and 11% higher in small, including their host metrics. Adjust encoded-event size from real measurements instead of treating the assumed 1 KiB as a hard shape. For the five small Segments, manifest-listed stored bytes were **0.663–0.668×** their recovered encoded Batch volume ([derivation](data/dev-small-local-run-01/small/derived.json)). This mixed repetitive/entropy corpus compressed less than the model's 0.5× ordinary case and better than its 1.6× expansion case. This ratio excludes frame/manifest/filesystem overhead and the unsealed tail; it is an observation, not a universal bound.

## Environment, bounds and evidence

Linux x86_64, kernel 6.18.44, glibc 2.41, overlayfs; four CPU equivalents cgroup quota, five visible affinity CPUs, 16 GiB shared cgroup memory maximum. Server affinity CPUs 0–1, Spindles/producer CPUs 2–3. Native release binaries were frozen before either run; hashes and exact source revision are in environment evidence. Two affinity CPUs do not reproduce a separately enforced 2-vCPU/4-GiB deployment. Server address-space bound was 4 GiB, each Spindle 512 MiB; address space differs from resident/cgroup memory. The pilot RSS thresholds were evaluation criteria, not enforced service budgets.

Logical trial disk stayed below 3 GiB, and workspace free space remained above 4 GiB. The trial cap counts source logs, Spools, state and compressed evidence; it is not central retention volume. Shared cached/virtualized storage, loopback network, local co-tenancy and producer activity affect results. No physical durability, cold-disk behavior, WAN, configured days of retention, long soak, query workload, traces or capacity ceiling was tested. Both short workloads had generous headroom; neither estimates maximum throughput.

Command, exit **0**, followed development then small with fresh independent state:

```sh
timeout 1800 python3 -B tools/bench/run_dev_small.py \
  --bin-dir /workspace/scratch/dev-small-frozen \
  --out /workspace/scratch/dev-small-native-run-01
```

[Completion receipt](data/dev-small-local-run-01/complete.json); [run output](data/dev-small-local-run-01/run.txt). Each trial's compressed `sources.jsonl.gz` contains source body hashes, post-write times, phase and producer lateness; `data-clocks.jsonl.gz` contains record ID, node, sequence and source/collection/ingestion/ACK observation times; `recovered-hashes.jsonl.gz` records recovered exact Batch hashes and volume; `node*-events.jsonl.gz` records native progress/ACK fields; `resources.json` records once-per-second process/disk/clock samples. Retained evidence was about 0.28 MiB development and 14.43 MiB small, below the registered 64 MiB/trial. Credentials, tokens and TLS private keys stay in owned scratch, not evidence. The large decoded replay text was removed after retaining hashes/data clocks; original source logs and durable state remain in bounded scratch.

## Verification and setup

Before trials, exact-source comparison accepted identical data and rejected deliberate missing and changed records; nearest-rank quantile spot checks passed. The runtime saved the [missing-record control](data/dev-small-local-run-01/negative-control.json). Recovery uses the product diagnostic reader, followed by the independent Python OTLP decoder and producer hash comparison. This is **not a full delivery-oracle transcript**, and no independent query answer was measured in these trials.

Initial build setup used a misspelled package ID `fabric-o11y`; that invocation failed before measurements. Correct `fabric_o11y` built `fabric-node`/`fabricctl`, and `fabric-server` plus `server_dump` built in release mode; successful build commands used `--release --locked`, two build jobs, Rust 1.98. [Build log](data/dev-small-local-run-01/build.txt). This corrected setup did not change workload, clocks or criteria. Protocol, runner and output remain separate commits.

Retained raw-clock evidence was audited again after copying: all 3,000 / 300,000 source IDs appeared exactly once, and independently recomputing collection/ingestion/ACK differences reproduced the recorded p50/p99 values. `bun tools/docs/check.mjs` and `git diff --check` exited 0. `cargo xtask checks --profile fast` exited 1: 19 checks passed; the unchanged Rust 1.98 Clippy `chunks_exact_to_as_chunks` finding at `crates/fabric-observation/src/crc32.rs:81` failed, as in previous runs. [Check output](data/dev-small-local-run-01/fast.txt). No check or registered criterion was weakened.
