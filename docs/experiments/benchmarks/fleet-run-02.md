# Fleet tiers, run 02 (target profile)

Status: all nine trials of the registered [fleet protocol](alpha-phase3-fleet-protocol.md) ran on 2026-09-29 on the 12-CPU target host and each passed every gate. Fleet delivery and control at 10, 100 and 1,000 identities is **Qualified** on the target profile for commit `e68d6ce`. This refreshes [fleet run 01](alpha-phase3-fleet-run-01.md), which ran on `4921e5e` before Segments and the architecture work. The simulated identities do not represent 1,000 deployed hosts or networks.

## Method

As registered: one `fabric-server` pinned to logical CPUs 0 to 3, one [simulator](../../../examples/spindle_sim.rs) process on CPUs 4 to 11 driving 10, 100 or 1,000 enrolled identities, 15 s warm-up plus 120 s measured, three seeds per tier, grouped commit, and one configuration change to every identity at 60 s. The artifacts are the set frozen from `e68d6ce1364ad02f2dfaa86e27922d3b8627155b` described in [history run 02](history-run-02.md), on the same [host](data/target-qualification/alpha-q1-host.txt); `sha256sum -c hashes.txt` matched all 28 files after the soak that preceded these trials. Nothing else heavy ran on the host.

For each tier `T` and seed `N`:

```sh
python3 -B tools/qualification/runner.py --out target/alpha-q1-fleet-T-seedN --duration-s 900 \
  --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
  python3 -B $F/tools/fleet_tier.py --tier T --seed 0xA11FA00N --bin-dir $F
```

All nine runner invocations exited `0` with `passed=true` and no stop reason; each took 135 to 154 s. Start and end times are in [progress.txt](data/target-qualification/fleet/progress.txt); runner results and summaries are in [data/target-qualification/fleet](data/target-qualification/fleet/).

## Results

| Tier | Seed | ACK p50 ms | ACK p99 ms | Delivery p99 ms | Max backlog | Apply p99 s | Apply max s | Server VmHWM KiB | Server CPU s |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 10 | 1 | 61.3 | 67.3 | 67.3 | 0 | 5.01 | 5.01 | 9,316 | 0.34 |
| 10 | 2 | 61.6 | 66.2 | 66.2 | 0 | 5.01 | 5.01 | 9,340 | 0.34 |
| 10 | 3 | 61.7 | 69.5 | 69.5 | 0 | 5.01 | 5.01 | 9,304 | 0.35 |
| 100 | 1 | 62.9 | 77.7 | 77.8 | 0 | 5.01 | 5.01 | 14,312 | 2.50 |
| 100 | 2 | 62.5 | 72.1 | 72.2 | 0 | 5.02 | 5.02 | 14,276 | 2.56 |
| 100 | 3 | 62.5 | 67.1 | 67.3 | 0 | 5.01 | 5.01 | 14,136 | 2.50 |
| 1000 | 1 | 62.2 | 72.6 | 526.4 | 0 | 4.94 | 5.34 | 431,780 | 29.58 |
| 1000 | 2 | 62.2 | 74.5 | 531.2 | 0 | 4.95 | 5.03 | 423,208 | 30.76 |
| 1000 | 3 | 62.3 | 74.1 | 528.7 | 0 | 4.97 | 5.36 | 434,676 | 29.81 |

Every trial acknowledged every batch it created (1,350, 13,500 and 135,000), passed the delivery oracle with 0 violations, and every inventory showed each identity's applied revision equal to its desired one. Enrolling 1,000 identities took about 6 s. Every gate was true in every trial: oracle, exits, ACK p99 at most 1 s, no growing backlog, server RSS at most 2 GiB, and every identity applied within 30 s.

## Interpretation

The margins of run 01 hold on the current code: ACK p99 is at least 12 times under 1 s, apply time is bounded by the 5 s poll interval, and the 1,000-identity delivery p99 near 530 ms is again the simulator's own queueing across its worker threads, with no backlog. Two things differ from run 01:

- **Server memory at 1,000 identities** is 413 to 424 MiB instead of about 48 MiB. These binaries seal journal files into Segments, and the 1,000-identity trials wrote about 248 MB, enough to seal; the peak matches the sealer's working set in [soak run 02](soak-run-02.md). It is still about five times under the 2 GiB gate. The 10- and 100-identity trials did not seal.
- **Apply time at 10 identities** is about 5.0 s at p99 instead of 0.06 s, and the median is 0.03, 4.99 and 5.00 s for seeds 1 to 3. Each identity picks up the change at its next poll, so apply time falls anywhere up to the 5 s interval depending on poll phase. The gate is 30 s. The difference was not investigated.

## Limits

One host and loopback TLS. The simulator keeps unacknowledged batches in memory, so it does not test node-side durability; the outage run does. The oracle compared SHA-256 projections of each batch's bytes on both sides.
