# Burst, rejection and concurrent management, run 01

Status: the three trials registered in [revision 2](stress-protocol-r2.md) of the [stress protocol](alpha-phase5-stress-protocol.md) ran on 2026-09-28, and each passed every gate. The state is **Measured and passing under revision 2 on a four-CPU host**. This is not a target-profile qualification: revision 1 on the target host has not run.

## Method

The run followed revision 2 as registered.

- **Workload.** 1,000 identities ran the frozen workload for 180 s, with five times the log rate from second 60 to second 79.
- **Rejection probes.** 200 rounds ran concurrently, each sending a revoked token, an unknown token and a malformed body, all from fresh node identities.
- **Management.** Every 0.5 s, the harness sent an inventory request, a configuration change and a 30 s log query.
- **Placement.** The server ran on CPUs 0-1 and the simulator on CPUs 2-3.
- **Artifacts.** Binaries, examples and harness were frozen from `2b5c939` into `target/alpha-p5r2-frozen`, with [28 SHA-256 values](data/delivery-recovery/hashes-p5r2.txt); the binaries are byte-identical to the outage and history sets. `sha256sum -c` after the soak found 0 mismatches.
- **Host.** The host was otherwise idle.

For each seed `N`:

```sh
python3 -B tools/qualification/runner.py --out target/alpha-p5r2-stress-seedN --duration-s 1500 \
  --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
  python3 -B $PWD/target/alpha-p5r2-frozen/tools/stress_tier.py --seed 0xA11FA00N \
  --bin-dir $PWD/target/alpha-p5r2-frozen --server-cpus 0-1 --sim-cpus 2-3
```

All three runner invocations exited 0 with `passed=true`, no stop reason and cleanup confirmed; each took 204 to 206 s. The summaries, runner results and simulator summaries are in [data/delivery-recovery/stress](data/delivery-recovery/stress/).

## Results

| Seed | Batches created / acknowledged | Backlog at 55 s, 79 s, 100 s, 175 s | Rejections: revoked, unknown, malformed | Rejected batches committed | Management ok / failed | Concurrent query p99 ms | Server VmHWM KiB |
| ---: | --- | --- | --- | ---: | --- | ---: | ---: |
| 1 | 180,000 / 180,000 | 0, 0, 0, 0 | 200× 401, 200× 401, 200× 400 | 0 | 265 / 0 | 369 | 652,668 |
| 2 | 180,000 / 180,000 | 0, 0, 0, 0 | 200× 401, 200× 401, 200× 400 | 0 | 268 / 0 | 324 | 656,244 |
| 3 | 180,000 / 180,000 | 0, 8, 0, 0 | 200× 401, 200× 401, 200× 400 | 0 | 268 / 0 | 329 | 663,356 |

| Gate | Result |
| --- | --- |
| The delivery oracle passes over the simulator transcript and `server_dump` | passed in all three trials, with 0 violations |
| Revoked and unknown tokens always get 401; malformed bodies get 400 or 413 | passed: every answer was 401 or 400 |
| No batch from a rejected identity is committed | passed: 0 committed |
| The backlog at 175 s is at most 1,000 batches above the backlog at 55 s | passed: every backlog at 175 s was 0 |
| Server VmHWM is at most 2 GiB | passed: the largest was 648 MiB |
| Every management call succeeds | passed |
| Both processes exit 0 | passed |

## Interpretation

On two server CPUs the server absorbed the burst with essentially no queueing. The largest sampled backlog was 8 batches, at the end of the burst in seed 3. Rejections were clean and consistent: 401 before any sequencing for revoked and unknown credentials, and 400 for bodies that are not a Batch. No rejected identity reached the journal.

## Deviation from the registered report

The protocol says latency during the burst is reported, not gated. The registered harness computes no burst-window latency: its summary gives only the backlog at four instants and the concurrent query p99. That metric is therefore not reported. Adding it now would change the harness after the run. The backlog at the end of the burst (79 s) stands in for it, and a later protocol revision should add the metric.

## Limits

- The run used one host, loopback TLS and simulated identities.
- A full queue (503) is covered by unit behavior and is not forced here.
- Management ran about 1.5 times per second, not twice, because each round waits for its three calls.
