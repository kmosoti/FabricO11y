# E2R equal-batching cost: run 01

Status: measured on 2026-09-27 after the selected sector model and cost harness passed
GPT and Claude execution review. The [preregistered protocol](group-seal-cost-protocol.md)
sets the workload, boundaries and decision rule; its original text is preserved
[with the run](data/group-seal-cost-run-01/protocol-at-run.md.txt).

## Result

All 96 writer trials and 96 separate-process verifications exited 0. Each output
matched the independent fixture encoder byte for byte, recovered all 1,200 bodies,
and used exactly the expected sync count. Every configuration produced identical
bytes across variants and repetitions. There were 16 warmup and 80 measured trials.

The 512-byte group-1 cell **fails** the combined material-benefit gate: median paired
throughput improved 90.79%, but median paired pipeline CPU cost rose to 131.0% of
control (limit 110%). The 4096-byte cell **passes**: throughput improved 124.59% and
CPU cost was 49.2% of control. This is a noisy shared-host result, not evidence that
larger sectors intrinsically improve CPU use. The two E3 correctness workers were
active throughout. No idle-host or application FOL2 speed claim follows.

| Sector bytes | Bodies/group | Two / one bodies/s | Paired median gain | Paired CPU ratio | Group-1 gate |
| --- | --- | --- | --- | --- | --- |
| 512 | 1 | 207.7 / 395.4 | 90.79% | 1.310 | fail |
| 512 | 2 | 399.6 / 759.2 | 66.47% | 0.793 | separate batching cell |
| 512 | 4 | 786.1 / 1466.0 | 86.48% | 0.692 | separate batching cell |
| 512 | 6 | 1298.3 / 2205.4 | 47.93% | 0.929 | separate batching cell |
| 4096 | 1 | 192.8 / 414.2 | 124.59% | 0.492 | pass |
| 4096 | 2 | 384.5 / 756.5 | 90.43% | 0.722 | separate batching cell |
| 4096 | 4 | 728.2 / 1279.6 | 88.71% | 0.804 | separate batching cell |
| 4096 | 6 | 1108.8 / 2179.2 | 105.09% | 0.590 | separate batching cell |

Throughputs are medians of the five measured trials per variant. Gains and CPU
ratios are medians of *paired ratios*, so they need not equal ratios of the displayed
medians. Batching cells remain separate from the registered group-1 decision.

## Costs and variation

For 512-byte sectors at group size 1:

- two-sync: median group commit P50/P99 3.602/15.736 ms; median pipeline CPU 360.0 µs/body; encoding 86.2 ms/trial; process peak RSS 21688–22584 KiB.
- one-sync: median group commit P50/P99 2.069/9.490 ms; median pipeline CPU 471.7 µs/body; encoding 85.6 ms/trial; process peak RSS 21560–22456 KiB.

For 4096-byte sectors at group size 1:

- two-sync: median group commit P50/P99 4.047/16.003 ms; median pipeline CPU 516.3 µs/body; encoding 141.9 ms/trial; process peak RSS 24504–25272 KiB.
- one-sync: median group commit P50/P99 1.927/9.556 ms; median pipeline CPU 272.3 µs/body; encoding 111.2 ms/trial; process peak RSS 24376–25144 KiB.

The encoded bytes/body are 2,560 (512-byte sector) and 12,970.67 (4096-byte sector)
at group size 1. At group sizes 2/4/6 they fall to 2,048/1,792/1,706.67 and
8,874.67/6,826.67/6,144 respectively. Padding remains a substantial cost; both sync
variants pay it equally. The opaque source body sizes average 1,054.17 bytes.

Pipeline CPU and wall time include encoding, writes and syncs; RSS covers the whole
writer process. Setup, per-trial reporting and separate-process replay are outside
the pipeline interval. Raw timings, group latencies, all paired ratios and resource
records are retained, including the 512-byte group-6 pair with a 19.87% slowdown.
These are userspace/resource observations, not device IOPS or power-loss tests.

## Reproduction, review and decision

```sh
python3 -B tools/seal-probe/bench.py run target/seal-cost/new-run
```

The actual command exited **0**. [Run metadata](data/group-seal-cost-run-01/run.json),
[environment and source hashes](data/group-seal-cost-run-01/environment.json),
[commands with exits](data/group-seal-cost-run-01/commands.jsonl),
[raw trials](data/group-seal-cost-run-01/trials.json), and
[paired summary](data/group-seal-cost-run-01/summary.json) preserve the evidence.
The [GPT review](data/group-seal-cost-run-01/gpt-review.json) and
[Claude review](data/group-seal-cost-run-01/claude-stdout.json) approved the measured
harness after executing write-order, error-propagation and fixture checks.

The result justifies further Rust investigation of the 4096-byte modeled design;
it does not select a production format. The [sector model](../ablation/seal-e2-run-01.md)
requires honored successful syncs, intact earlier acknowledged sectors and an
independent witness after a known I/O error. The application retains two-sync FOL2.
