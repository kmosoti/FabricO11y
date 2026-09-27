# Stage 6 local-log baseline

## Status

**First registered workload measured on 2026-09-26.** This is a local receiver baseline, not a comparison or a production performance claim. The current path is [generator](../../../src/generator.rs) → [bounded buffer](../../../src/buffer.rs) → [EventLog](../../../src/log.rs). The [storage contract](../../architecture/storage.md) and [ADR-0006](../../decisions/ADR-0006-use-framed-local-log.md) define a successful append as an event-frame sync followed by a commit-marker sync.

## Question and correctness gate

What throughput, latency, storage, memory, allocation, and recovery costs does that exact path have for one named synthetic workload on this host?

Every trial must use a fresh log path. After the ingest interval, close the writer, reopen with `EventLog::open`, replay every record, and compare its encoded contents with the same seeded generator sequence using `same_record_contents`. Require exactly the requested count, in order, with no rejected event lost or duplicated. Any mismatch, append error, or failed sync invalidates that trial; do not report its latency as a successful run. File sync success is interpreted under the assumptions in the storage view. This gate checks process restart and bytes; it does not simulate physical power loss.

The buffer's full-push result remains caller-owned and must be retried after draining. The benchmark may choose a different buffer capacity or batch size only in a later, separately registered comparison. It must never remove a sync to obtain a faster number under the same durability label.

## Fixed workload and execution

| Item | Registered value |
| --- | --- |
| Input | `EventGenerator` Gauge workload, seed `42`, `2,000` events per measured trial |
| Buffer | Capacity `256` events; drain batch limit `64`; retry each rejected event after draining |
| Log | One fresh `FOL2` file per trial on the repository's ext4 filesystem; no prior records |
| Build | `cargo build --release --offline --locked --example local_log_probe`; run `target/release/examples/local_log_probe` without per-event console output. Build the separate allocation run with `--features stage6-alloc-probe` |
| Repetition | One untallied warmup, then five measured trials in fixed trial order |
| Recovery sizes | Separate fresh valid logs of `0` and `500` events; use the `2,000`-event measured logs for the third size |
| Raw output | One CSV row per trial and one append-latency sample per successfully committed event; keep files under an ignored `target/stage6/` directory and record their checksums in the result document |

Record the exact source revision or dirty-tree digest, `rustc` and Cargo versions, CPU model, kernel, WSL version, filesystem and mount, storage device, build flags, and whether other loads were present. Preserve the exact command line and raw samples. A second host or filesystem is a new experiment, not a pooled repeat.

## Timing boundaries and metrics

- **Append latency:** `Instant` immediately before `EventLog::append(&event)` to its successful return. It includes encoding, event write and sync, marker write and sync. Report p50, p99 (nearest-rank), maximum, and the number of samples per trial. Do not infer a hardware `fsync` duration from it.
- **Pipeline throughput:** `2,000 / elapsed_seconds` from just before generating the first event until the last successful append returns, excluding open, replay, verification, and printing. This includes generator, buffer, and append work. Report events/s per trial and the median/range of the five trial values. Append samples alone do not equal pipeline time.
- **Storage bytes:** length of the closed log divided by `2,000`, with total bytes and exact record count. This includes frame headers and commit markers; it is not a compression ratio.
- **Recovery and replay:** time a fresh process takes for `EventLog::open` on each valid size, reported separately from the time to visit and verify records through `replay`. The open scan and directory/file sync are part of recovery. Report size and time together; three sizes are an exploratory scaling check, not an asymptotic proof.
- **Peak resident memory:** read Linux `/proc/self/status` `VmHWM` after a representative release ingest, outside the timed interval. Report KiB and the whole-process boundary. It includes process and runtime overhead and is not a live-byte measurement of `EventBuffer`. If `VmHWM` is unavailable, report the missing measurement.
- **Allocations:** instrument one separate release run of the same `2,000`-event workload with the `stage6-alloc-probe` feature and a System-forwarding counting allocator, enabled only over the pipeline interval. Count successful `alloc`, `alloc_zeroed`, and `realloc` calls and the requested sizes of those calls; this is gross requested allocation, not net live memory. The allocator changes timing, so do not combine its latency or throughput samples with the normal five runs. If this measurement cannot be implemented or checked, report it as missing and keep this part of Stage 6 open.
- **Loss and retry:** report full-buffer rejections, retries, append failures, and the number of replayed records. For a successful probe run, each full-buffer rejection is retried once immediately, so `retries = full_rejections`; append failures are zero because any append error aborts before CSV output. Report a failed invocation separately with its error. A full-buffer rejection followed by retry is backpressure, not data loss.

Compute percentiles within each trial before summarizing across trials. Do not pool events across runs and hide run-to-run variation. Record any incomplete or failed trial as a failure with its cause. There is no preselected throughput winner or acceptance threshold for an optimization: this baseline identifies costs to investigate. A later ablation must keep the successful-append durability boundary and workload fixed unless it explicitly studies a changed contract.

## Planned learning increment

The [probe](../../../examples/local_log_probe.rs) prints machine-readable counters and samples. Run a short correctness smoke case before the five registered release trials. It borrows each event during append; on any append error it aborts that trial, and a later attempt must start from the reproducible seed on a fresh healthy path. It is not a general sender-retention implementation. Compare recovered records in a separate process to exercise the restart path. Record the run and limits here before choosing any optimization. See [Stage 6 in the learning path](../../LEARNING_PATH.md#stage-6--measure-and-challenge-the-baseline) and the [Homa/SIRD follow-on proposal](../ablation/receiver-driven-transport.md).

For a short follow-along run, choose a file name that does not already exist:

```sh
cargo build --release --offline --locked --example local_log_probe
mkdir -p target/stage6/lesson
target/release/examples/local_log_probe write target/stage6/lesson/five.fol2 42 5 > target/stage6/lesson/write.csv
target/release/examples/local_log_probe verify target/stage6/lesson/five.fol2 42 5 > target/stage6/lesson/verify.csv
```

The write output has five `append` rows and one `ingest` row; the verify output has `open` and `replay` rows. Run the write command again only with a new filename. The registered `2,000`-event runs use the same commands with five fresh paths.

The [registered runner](../../../tools/bench/run_local_log_stage6.sh) creates all trials in a new output directory. The [summary script](../../../tools/bench/summarize_local_log_stage6.py) checks counts, order, file sizes, and CSV schema before computing per-trial nearest-rank percentiles. Use a new directory name for each complete attempt:

```sh
bash tools/bench/run_local_log_stage6.sh target/stage6/run-01
python3 -B tools/bench/summarize_local_log_stage6.py target/stage6/run-01 > target/stage6/run-01/summary.json
```

## Executed run and results

The commands above exited **0** on 2026-09-26 local time. The runner's normal release build, one warmup, five measured write/verify pairs, 0- and 500-event recovery cases, and separate allocation-feature build/write/verify all completed. The analyzer exited **0**; `sha256sum -c SHA256SUMS` in `target/stage6/run-01/` and `sha256sum -c CSV_SHA256SUMS` in the [preserved raw-data directory](data/local-log-stage6-run-01/README.md) also exited **0**. The raw [summary](data/local-log-stage6-run-01/summary.json), [CSV](data/local-log-stage6-run-01/write-1.csv), and [environment record](data/local-log-stage6-run-01/environment.txt) are linked here. The environment record includes SHA-256 hashes for the exact Rust sources used because the repository still had only its initial commit plus uncommitted files.

All five measured trials appended and separately verified exactly 2,000 records. Each had 28 full-buffer rejections followed by 28 retries, zero append failures, and a 296,000-byte log: **148 bytes per event** for this fixed Gauge shape.

| Trial | Pipeline events/s | Append p50 ms | Append p99 ms | Append max ms |
| --- | ---: | ---: | ---: | ---: |
| 1 | 381.0 | 2.586 | 3.398 | 7.949 |
| 2 | 390.4 | 2.504 | 3.627 | 7.624 |
| 3 | 340.0 | 2.839 | 5.284 | 8.873 |
| 4 | 394.8 | 2.492 | 3.168 | 15.088 |
| 5 | 383.4 | 2.573 | 3.403 | 7.953 |

Median pipeline throughput was **383.4 events/s**, with a **340.0–394.8 events/s** range. Per-trial p50 and p99 values are nearest-rank statistics from 2,000 append samples each; they are not pooled across runs. Summed time inside `EventLog::append` was 99.965–99.969% of measured pipeline time. That identifies the append call as this workload's immediate investigation target; the probe does not separate encoding, writes, and the two syncs within it.

The `VmHWM` whole-process peak was **2,220–2,404 KiB** across the five normal trials. The separate allocation build counted **20,039 successful allocation or reallocation requests** and **989,024 gross requested bytes** over its 2,000-event pipeline. Those counts exclude setup and output and include encoder and standard-library requests; its throughput is not compared with the normal build.

Fresh-process `EventLog::open` took **1.533 ms** for 0 records and **2.982 ms** for 500 records in one run each, and **5.024–5.576 ms** for the five 2,000-record logs. Replay plus exact content verification of the 2,000-record logs took **3.652–3.969 ms**. Open scans records and syncs the file and parent directory; these few warm-cache sizes suggest an increasing cost but do not establish a scaling law.

## Interpretation and limits

The first question to challenge is which part of `append` accounts for its time: encoding, event write and sync, or marker write and sync. The next experiment should instrument those phases without changing the success boundary before choosing an encoding or storage alternative. The measured append share does not prove that sync calls alone are the bottleneck.

This is one fixed synthetic Gauge shape, one sender, 2,000 events per trial, a warm ext4 filesystem inside WSL2, and one laptop CPU. Background activity was present on the host during this session, and trial 3's p99 and trial 4's maximum show meaningful variation. `VmHWM` is whole-process peak memory, and allocation counts are gross requests from an instrumented build. These results do not test power-loss durability, a production telemetry distribution, sustained larger logs, a network transport, or a Homa/SIRD mechanism. They select no optimization winner.
