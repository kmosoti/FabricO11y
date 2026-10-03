# Medium, enterprise and recovered worker-control simulations, local run 01

Status: **medium met the registered criteria; enterprise failed timing/creation-lag criteria while preserving custody; recovered medium control met its exploratory threshold**. Source/harness `75bd92b`; [main protocol](medium-enterprise-local-protocol.md), [recovered control protocol](recovered-worker-control-protocol.md), [runner](../../../tools/bench/run_scaled_fleet.py), [environment and hashes](data/scaled-fleet-run-01/environment.json). Three fresh-state native trials ran sequentially against the real TLS/durable server. These are short server/fleet simulations, not deployment qualification or physical enterprise capacity.

## Consolidation and recovered work

`milestone/streaming-segment-output` is the one active local experiment branch. It already contains the journal-reclaim candidate, multidimensional model, writer pilot, sizing calculator and native dev/small evidence as a linear ancestry. The redundant local `milestone/journal-reclaim-progress` and `milestone/workload-sizing` names were deleted after checking ancestry. No merge, main-branch change or rewrite of published history was needed. Older published journal refs are historical snapshots, not separate active work. Original experiment records retain their original branch/revision provenance.

| Recovered experiment or hypothesis | Evidence/status | Next value |
| --- | --- | --- |
| Earlier reclaim before/between worker groups | [mechanism run](journal-reclaim-local-run-01.md), included in tested server | Ordered custody preserved; isolated mixed-load baseline comparison still unrun |
| Streaming output inventory removal (R0) | [writer run](streaming-output-local-run-01.md), primary >10% heap target failed | Preserve failure; do not call the RAM problem solved |
| Adaptive concurrency/headroom, R5/R6 | [proposal](adaptive-sealing-hypotheses.md), static 1 vs 2 worker control below | Measure latency versus RAM cost before implementing controller |
| Remaining working-set attribution, R1 | [pipeline route catalog](sealing-pipeline-experiment-design.md), unrun | Highest-value memory follow-up: classify fleet/queue/builder/allocator demand |
| Byte-bounded loader/processing/merge | [computational model](../../research/sealing-pipeline-model.md), proposal | Requires ownership/admission/drain guarantees; no full bounded builder claimed |

## Workloads and measurements

Medium simulated 200 identities at 50 logs/s each; enterprise simulated 2,000 at 50 logs/s each. Planned phases were 10 seconds ordinary, five seconds 3× burst, ten seconds ordinary recovery. Half the 900-byte bodies were repeated, half seeded entropy; simulator metrics ran every 15 seconds. The fleet example retained one Batch in flight per identity and pending Batches in memory. It used 128 requested workers, partitioned into 100 actual worker slices for medium and 125 for enterprise. There were no deployed native Spools in these simulations, no application file-poll wait and no WAN.

| Metric | Medium, 1 worker | Enterprise, 1 worker | Medium, 2 workers |
| --- | ---: | ---: | ---: |
| Ordinary / burst logs/s, planned | 10,000 / 30,000 | 100,000 / 300,000 | 10,000 / 30,000 |
| Exact-custody logs recovered / planned | 350,000 / 350,000 | 3,500,000 / 3,500,000 | 350,000 / 350,000 |
| Collection to ingestion p50 / p99, ms | 34.06 / 168.44 | 1,119.27 / 6,984.90 | 29.69 / 149.59 |
| Collection to ACK p50 / p99, ms | 41.05 / 175.13 | 1,125.95 / 6,991.97 | 36.03 / 153.49 |
| Scheduled offer to ingestion p50 / p99, ms | 52.52 / 214.35 | 1,762.36 / 7,350.10 | 47.68 / 175.48 |
| Per-Batch collection creation-lag p99, ms | 64.97 | 1,375.94 | 55.68 |
| Server peak RSS, MiB | 641.98 | 1,061.05 | 756.35 |
| Simulator peak RSS, MiB | 177.46 | 1,640.89 | 177.68 |
| Mean server CPU equivalents | 0.080 | 0.841 | 0.076 |
| Mean simulator CPU equivalents | 0.141 | 1.198 | 0.129 |
| Largest sampled sealed-file backlog | 1 | 15 | 1 |
| Retry attempts (`unavailable`) | 0 | 43,305 | 0 |
| Final published Segments / sealed files waiting | 4 / 0 | 47 / 0 | 4 / 0 |
| ACK completion after simulation epoch, s | 24.07 | 28.94 | 24.07 |
| Independent delivery oracle | passed | passed | passed |

The oracle compared SHA-256 projections of every source, attempt, response and freshly recovered exact Batch as in the existing fleet harness. It checked 5,000 / 50,000 / 5,000 recovered Batches. Independent Python OTLP decoding checked every log's count, 900-byte body shape and collection timestamp against its created cohort. All planned cohorts were generated, all pending queues ended empty, and children exited zero. The experiment did not query projected answers or compare original plaintext body hashes separately; custody correctness uses the independent digest oracle and the collision assumption already used by delivery.

Actual encoded Batch volume was **921.01 bytes/log**, including simulator metrics/envelopes. The planned 1 KiB sizing assumption is therefore a different shape; real native Spindles in [dev/small](dev-small-local-run-01.md) used 1,132–1,172 bytes/log due to real file attributes and host metrics. Differences across those experiments cannot establish a pure rate-only scaling law.

## Enterprise null: overload without custody loss

Enterprise failed four timing criteria: creation lag p99 >1 second, collection-to-ingestion and collection-to-ACK p99 >1 second, scheduled-to-ingestion p99 >2 seconds. Resource/correctness/clock criteria passed. The failure is retained; no gate was changed.

The journal reached 15 sealed 64 MiB files waiting, and 43,305 attempts returned retryable `unavailable`. Pending data remained with the simulator and eventually all 3.5 million logs were recovered. These attempts are not lost logs or distinct lost Batches. The simulator itself shared two host CPUs and ran behind its planned collection clock; source-generation contention and server backlog both affect timing. Exact attribution to sealing versus TLS/commit/reclaim/generator needs a separate experiment.

By planned cohort, enterprise collection-to-ingestion p99 was about **1.86 seconds ordinary, 5.82 seconds burst, 7.87 seconds recovery**. Delayed burst work continued into recovery; overall p99 cannot hide that. All ACKs completed at about 28.94 seconds from the epoch, four seconds beyond the nominal offer window, and sealing completed within the additional drain. That shows finite recovery here, not a guarantee under repeated bursts or sustained enterprise load.

The interval's planned average is 140,000 logs/s because it includes a 3× burst. All ACKed logs divided by final ACK completion gives about 120,925 logs/s over this short interval; this includes buffered/burst behavior and is **not sustainable capacity or proof of a 100,000 logs/s production deployment**. CPU means include the subsequent 30-second server drain/idle period and can conceal peak saturation. Simulator CPU mean is over its live sampled interval. Do not interpret those averages as spare CPU during burst.

## Recovered control: latency versus memory

Two workers reduced medium's overall ingestion p99 by **11.19%**, meeting the predeclared exploratory >10% threshold, while peak RSS increased **17.82%** (about 114 MiB). Both variants passed custody/timing/resource criteria. [Control calculation](data/scaled-fleet-run-01/worker-control.json).

One sequential trial per variant establishes no significance or universal winner. Source identities/timestamps, scheduling, grouping and co-tenancy are nondeterministic even with fixed body seed. Ordinary-phase p99 was higher with two workers (186.70 versus 174.69 ms), while burst/recovery tails were lower. CPU means differed slightly; no CPU-saving conclusion follows. A repeat/held-out confirmation and a measured memory ledger are needed before accepting a controller policy.

Memory remains a deployment dimension. One sealing worker peaked near 642 MiB in medium and 1,061 MiB in enterprise, so peak demand cannot be represented by a universal per-thread constant alone. Fleet connections, queue/active journal state and allocator behavior may contribute; this run did not individually attribute them. The memory cap should be a configurable budget governing admission/headroom. The installed 3 GiB systemd default is not an architectural ceiling, and process RSS does not include all cgroup page-cache/kernel charges.

## Reproduction and limits

```sh
timeout 2700 python3 -B tools/bench/run_scaled_fleet.py \
  --bin-dir /workspace/scratch/scaled-fleet-frozen \
  --out /workspace/scratch/scaled-fleet-run-01
```

Exit 0 means the three experiments/analyses completed; enterprise's verdict remains failed. Frozen binaries: server/server_dump identical to dev/small; simulator and offline enrollment example release-built from source above. Original simulator default entropy test still matched its independent frozen Python digest. Added test checked 50/150 log counts, 900-byte bodies and unchanged collection clock. Optional flags default to historical 512-byte/two-log workload. [Build log](data/scaled-fleet-run-01/build.txt); [run output](data/scaled-fleet-run-01/run.txt); [completion](data/scaled-fleet-run-01/complete.json); [negative control](data/scaled-fleet-run-01/negative-control.json) rejected missing recovery and accepted complete custody before trials.

Host: same Linux/overlayfs, four-CPU-equivalent quota, five affinity CPUs, 16 GiB shared cgroup. Server CPUs 0–1, simulator/analysis CPUs 2–3; one or two sealing workers. Address-space limits: server 4 GiB, simulator 6 GiB. Journal ceiling 1 GiB; Segment ceiling 8 GiB, age 24 h. No intended retention eviction occurred. All runs remained inside finite bounds with >4 GiB free workspace disk. Resource sampling was once per second; high-water RSS is retained by `/proc`, while backlog peaks between samples may be missed. Large decoded replay was removed after retaining hashes/clocks; original custody state remains in owned scratch. Logical disk samples are taken during simulation/drain, not while temporary replay text is materialized afterward.

Retained evidence: about 1.15 / 16.52 / 1.15 MiB, below 128 MiB/tier. Under [data](data/scaled-fleet-run-01/), each tier retains summaries, oracle verdict, native source/attempt transcript and events compressed, compact Batch-clock rows with record counts/exact digest, resource samples, and derived ACK completion. Tokens, TLS keys and credentials were excluded. Nearest-rank record-weighted quantiles use Batch log-count weights; records in one Batch share clocks. Scheduled clocks use simulator epoch plus sequence-minus-one seconds, avoiding coordinated omission from late generation. Sampled realtime-minus-monotonic offsets showed no >5 ms step; cross-host synchronization remains untested.

These finite local runs do not emulate 200/2,000 physical hosts, native collection/storage cost, cold storage, concurrent queries, long retention, sustained overload, HA or sharding. Candidate production changes remain experimental and unmerged. Next valuable recovered route is stage/fleet/queue memory attribution; enterprise worker-count controls require separately registered budget/headroom so increased concurrency does not trade latency for uncontrolled RAM.

Retained Batch-clock evidence was audited after copying: counts and unique identity/sequence keys matched 350,000 / 3,500,000 / 350,000 logs, and independently recomputed weighted p50/p99 reproduced all three ingestion summaries. `bun tools/docs/check.mjs` and `git diff --check` exited 0. `cargo xtask checks --profile fast` exited 1: 19 checks passed; the previously recorded Clippy `chunks_exact_to_as_chunks` finding at unchanged `crates/fabric-observation/src/crc32.rs:81` failed. [Check output](data/scaled-fleet-run-01/fast.txt). No check or criterion was weakened.
