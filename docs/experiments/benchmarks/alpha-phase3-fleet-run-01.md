# Phase-3 fleet tiers, run 01

Status: all nine registered trials passed on 2026-09-28 (plan step 3.5). The simulated identities do not represent 1,000 deployed hosts or networks; observation-to-query freshness and query latency need phase 4 and are not measured here.

## Method

As registered in the [fleet protocol](alpha-phase3-fleet-protocol.md): one `fabric-server` pinned to logical CPUs 0 to 3, one [simulator](../../../examples/spindle_sim.rs) process on CPUs 4 to 11 driving 10, 100 or 1,000 enrolled identities, 15 s warmup plus 120 s measured, three seeds per tier, grouped commit, and one configuration change to every identity at 60 s. Binaries and harness were frozen from commit `4921e5eac1b7b8e3c0f0a1af0dad4ba5ea42bb53`; SHA-256 values are in [hashes.txt](data/alpha-phase3/hashes.txt). During the run a storage-probe research test ran pinned to CPU 11, which the simulator shares; the server's CPUs were not shared. For each tier `T` and `N=1,2,3`:

```sh
python3 -B tools/alpha/runner.py --out target/alpha-p3-fleet-T-seedN --duration-s 900 --disk-bytes 5368709120 --max-output-bytes 1048576 -- python3 -B target/alpha-p3-frozen/tools/fleet_tier.py --tier T --seed 0xA11FA00N --bin-dir target/alpha-p3-frozen
```

All nine runner invocations exited `0` with `passed=true`. Per-trial summaries and runner results are in [data/alpha-phase3](data/alpha-phase3/).

## Results

| Tier | Seed | ACK p50 ms | ACK p99 ms | Delivery p99 ms | Max backlog | Apply p99 s | Apply max s | Server VmHWM KiB | Server CPU s |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 10 | 1 | 59.2 | 66.0 | 66.1 | 0 | 0.06 | 0.06 | 7736 | 0.28 |
| 10 | 2 | 59.1 | 64.9 | 64.9 | 0 | 0.07 | 0.07 | 7576 | 0.28 |
| 10 | 3 | 59.4 | 64.6 | 64.6 | 0 | 0.06 | 0.06 | 7644 | 0.27 |
| 100 | 1 | 60.7 | 66.1 | 66.2 | 0 | 5.01 | 5.01 | 12856 | 2.42 |
| 100 | 2 | 61.1 | 66.0 | 66.1 | 0 | 5.00 | 5.00 | 12712 | 2.42 |
| 100 | 3 | 61.3 | 69.4 | 69.5 | 0 | 5.00 | 5.00 | 12900 | 2.40 |
| 1000 | 1 | 62.4 | 74.3 | 531.7 | 0 | 4.98 | 5.34 | 49564 | 27.02 |
| 1000 | 2 | 62.7 | 72.9 | 533.0 | 0 | 4.97 | 5.24 | 49220 | 26.85 |
| 1000 | 3 | 62.6 | 73.0 | 534.5 | 0 | 4.98 | 5.42 | 49132 | 27.26 |

Every trial acknowledged every batch it created (1,350, 13,500 and 135,000), passed the delivery oracle, and every inventory showed each identity's applied revision equal to its desired one. Enrolling 1,000 identities took about 7 s.

## Interpretation

The server kept up with the frozen workload at every tier with a large margin on each gate: ACK p99 is about 14 times under 1 s and server peak RSS is about 40 times under 2 GiB. Apply latency is bounded by the 5 s poll interval, as designed. At 1,000 identities the delivery latency, from batch creation to ACK, rises to about 530 ms p99, with no growing backlog. That is the simulator's own queueing, since each of its 128 worker threads sends for about eight identities in turn; a real node sends only its own batches. Server CPU grows roughly linearly with identities: about 27 CPU seconds per 120 s at 1,000.

## Limits

One host and loopback TLS. The simulator keeps unacknowledged batches in memory, so it does not test node-side durability; the phase-2 runs do. The oracle compared SHA-256 projections of each batch's bytes on both sides, which preserves equality under the usual collision assumption. Server journal rotation did not produce segments here, because the phase-3 binaries predate sealing.
