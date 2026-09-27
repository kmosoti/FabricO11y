# S0: attribute the existing two-sync append cost

Status: run 01 measured; registered before instrumentation or measured trials. This follows the
[Stage 6 baseline](local-log-stage6.md) and contributes to the
[end-to-end research contract](../ablation/end-to-end-prototype.md).

## Hypothesis and fixed semantics

Hypothesis: file synchronization dominates this machine's per-event append cost.
Measure it; do not infer a storage-format decision from this workload. The FOL2
bytes, two file syncs, commit boundary, poisoning on I/O failure, and borrowed-event
ownership remain unchanged. No one-sync candidate enters this experiment.

An optional `append-attribution` feature exposes the last successful append's five
wall-clock phase durations: encode/header/CRC/offset/marker preparation; data
seek/write; data sync; marker write; marker sync. Clear the sample before each
attempt, including rejected attempts. Failed appends produce no successful sample.
Use conditional compilation so default builds retain the uninstrumented path.
Timer sampling has cost, measured by a paired control below. Durations are elapsed
time, not CPU utilization or device latency.

## Registered workload and comparison

Use the existing `local_log_probe` on the repository filesystem, seed 42, 2,000
Gauge events, FIFO capacity 256, drain batch 64. Each trial uses a fresh log. Build
release/offline default and feature binaries separately with the same locked
dependencies. Warm each binary once, then run five measured pairs, alternating
default/feature order by pair. Run one process at a time, with separate-process
exact replay after every write. Keep raw CSV, commands, exits, source hashes,
environment/mount details and file hashes. Do not silently rerun an outlier.

The outer append timer remains the existing call boundary. The five phase values
must sum to no more than that outer duration. Phase samples and outer samples are
preallocated; print only after the timed ingest interval. The feature's extra
sample capture participates in pipeline cost. Open/recovery and CSV formatting
are outside pipeline timing; generation, buffering and final append are inside.

Report per-trial ingest events/s, outer append P50/P99, phase totals and their
fraction of total outer append duration, file size and peak RSS. Quantiles use
nearest rank. Compare median paired relative pipeline elapsed-time change; above
10% in magnitude flags materially perturbed attribution, requiring that limitation
beside results rather than an unqualified cost claim. Phase dominance means the
two sync phases together exceed 50% of summed outer append time in each measured
feature trial. Report all five trials even if the hypothesis fails. Equal event
counts, bit-exact replay and identical file hashes are correctness gates.

Run the existing allocation feature once with and once without attribution,
separately from timing trials, to report requested allocations and peak RSS.
Record process CPU and I/O counters where available; do not invent device IOPS,
network costs or energy measurements. This is one WSL filesystem/workload, not a
representative production distribution or physical power-loss test.

## Checks

Existing root tests must pass both with default features and all features. Add
focused checks that a successful sample exists, an invalid oversized record clears
it without poisoning the log, and the next valid append succeeds. Check byte and
replay equality across binaries. The CSV validator must reject representative
missing phase, duplicate phase, and phase-sum-over-outer defects; preserve those
real failures. Instrumentation must introduce no new log writes or flushes.

## Run-01 host-load note, before timing

The two fixed E3R correctness workers remain CPU-bound on this shared workstation.
Run-01 keeps the benchmark writers and replay processes serialized, and records
those background workers explicitly. The paired alternating control tests timer
perturbation under that observed load; the run does not estimate an otherwise idle
machine. Preserve a process-name/CPU snapshot at start and finish without command
arguments. Do not compare its absolute throughput to the earlier Stage 6 day as
though load, filesystem state and scheduling were held constant.

## Run-01 results

The runner `python3 -B tools/bench/append_attribution.py run target/append-attribution-s0/run-01`
exited **0**. Four release/offline/locked builds, warmups, five measured pairs,
separate allocation controls and every separate-process replay completed. Every
log contains 2,000 exact events, 28 full-buffer retries, 296,000 bytes and the same
SHA-256. Raw [summary](data/append-attribution-s0-run-01/summary.json),
[commands/resources](data/append-attribution-s0-run-01/commands.jsonl),
[environment](data/append-attribution-s0-run-01/environment.json) and
[background load](data/append-attribution-s0-run-01/host-load.json) are preserved.

**The instrumented build's median paired elapsed-time increase was 11.017%, above
the registered 10% perturbation threshold.** The phase fractions below describe
that instrumented run. They do not establish the phase costs of an idle,
uninstrumented application, or isolate timer overhead from scheduling/filesystem
variation. There is no unqualified speed claim or format selection.

| Pair | Default events/s | Instrumented events/s | Instrumented append P50 ms | P99 ms | Both sync phases / outer append |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 338.2 | 304.6 | 3.012 | 8.047 | 97.117% |
| 2 | 225.2 | 201.4 | 4.030 | 13.110 | 97.198% |
| 3 | 323.0 | 295.6 | 3.268 | 5.282 | 96.808% |
| 4 | 308.5 | 272.3 | 3.311 | 11.131 | 96.983% |
| 5 | 322.6 | 294.4 | 3.192 | 7.685 | 96.949% |

The sync-dominance hypothesis holds in every measured instrumented trial
(96.808–97.198% of summed outer append time). Encoding/header preparation was a
small fraction there, but the overhead control prevents treating these percentages
as precise costs for the default build. Per-event phase sums stayed within outer
append durations. Both allocation controls counted 20,039 requests and 989,024
gross requested bytes inside the pipeline; preallocated timing arrays are outside
that interval. Allocation-build timings are excluded from the comparison.

Whole-process CPU was 0.365–0.564 seconds for default measured writers and
0.455–0.681 seconds for instrumented writers; this includes setup and the feature
build's extra CSV output, so it is not ingest-only CPU. Peak RSS was 2,284–2,404 KiB
and 2,364–2,584 KiB respectively. Kernel input/output-block counters are recorded
per process without interpreting them as device IOPS.

Root Rust tests passed with default and all features (24 and 25 tests respectively).
The CSV validator's three defective fixtures each exited 1 at the intended validation,
with a clean fixture exiting 0. GPT and Claude executed additional probes and approved
the unchanged implementation. The [validator mutations](data/append-attribution-s0-run-01/validator-mutations.json),
[GPT review](data/append-attribution-s0-run-01/gpt-review.json) and
[Claude review](data/append-attribution-s0-run-01/claude-stdout.json) preserve evidence.
Claude's default-test/mkdir attempts were denied by its narrow tool allowlist;
its allowed all-feature tests and independent probe ran successfully. The default
build was separately tested by the worker and GPT reviewer.

This closes the scoped attribution experiment with an explicit perturbation limit.
A later idle-host or lower-overhead attribution run would need its own registration;
rerunning until the threshold passes would not repair this result. The next storage
hypothesis remains the separate [group-seal model](../ablation/seal-e2-run-01.md)
and its [unmeasured cost protocol](group-seal-cost-protocol.md).
