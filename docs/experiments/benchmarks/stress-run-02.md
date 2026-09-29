# Burst, rejection and concurrent management, run 02 (target profile)

Status: the three trials of [revision 1](alpha-phase5-stress-protocol.md) of the stress protocol, as registered, ran on 2026-09-28 on the 12-CPU target host and each passed every gate. The burst, rejection and concurrent-management capability is **Qualified** on the target profile for commit `e68d6ce`.

## Method

As registered in revision 1, with its default placement: the server on CPUs 0 to 3 and the simulator on CPUs 4 to 11. 1,000 identities ran the frozen workload for 180 s with five times the log rate from second 60 to 79; 200 rounds of revoked-token, unknown-token and malformed-body probes from fresh identities ran concurrently; every 0.5 s the harness listed the inventory, changed a configuration and ran a 30 s log query. The artifacts are the set frozen from `e68d6ce1364ad02f2dfaa86e27922d3b8627155b` described in [history run 02](history-run-02.md), on the same [host](data/target-qualification/alpha-q1-host.txt).

For each seed `N`:

```sh
python3 -B tools/qualification/runner.py --out target/alpha-q1-stress-seedN --duration-s 1500 \
  --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
  python3 -B $F/tools/stress_tier.py --seed 0xA11FA00N --bin-dir $F
```

All three runner invocations exited `0` with `passed=true` and no stop reason; each took 206 s. Start and end times are in [progress.txt](data/target-qualification/stress/progress.txt); runner results, summaries and simulator summaries are in [data/target-qualification/stress](data/target-qualification/stress/).

## Results

| Seed | Batches created / acknowledged | Max backlog at 55, 79, 100, 175 s | Rejections: revoked, unknown, malformed | Rejected batches committed | Management ok / failed | Concurrent query p99 ms | Server VmHWM KiB |
| ---: | --- | ---: | --- | ---: | --- | ---: | ---: |
| 1 | 180,000 / 180,000 | 0 | 200 × 401, 200 × 401, 200 × 400 | 0 | 268 / 0 | 337 | 665,596 |
| 2 | 180,000 / 180,000 | 0 | 200 × 401, 200 × 401, 200 × 400 | 0 | 268 / 0 | 363 | 686,452 |
| 3 | 180,000 / 180,000 | 0 | 200 × 401, 200 × 401, 200 × 400 | 0 | 269 / 0 | 336 | 688,800 |

ACK latency in milliseconds, request start to acknowledgement, reported and not gated:

| Seed | Before burst p50 | Before burst p99 | Burst p50 | Burst p99 | Burst max |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 62.6 | 74.1 | 70.4 | 86.1 | 137.2 |
| 2 | 62.5 | 71.7 | 69.6 | 97.7 | 144.0 |
| 3 | 62.4 | 74.7 | 70.1 | 82.3 | 108.6 |

Every gate was true in every trial: oracle exact, both processes exited 0, revoked and unknown tokens always 401, malformed bodies always 400, no rejected batch recovered, backlog recovered after the burst, server VmHWM at most 2 GiB, and every management call succeeded.

## Interpretation

A fivefold log burst raised ACK p99 by about 10 to 25 ms and left no backlog at any sample, so the server absorbed the burst within the 50 ms grouping window it already pays. Rejections were exact and never reached the journal. Concurrent administration and queries under load all succeeded, with query p99 near the history run's segment-mode latency.

## Limits

One host, loopback TLS and three trials. The queue-full answer (503) is covered by its unit behaviour, not forced here. Burst latency is reported, not gated.
