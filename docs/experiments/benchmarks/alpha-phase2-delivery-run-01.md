# Phase-2 delivery with ten real nodes, run 01

Status: all six registered trials passed on 2026-09-28 (plan step 2.6). One host; this is not the fleet tier gate.

## Method

As registered in the [protocol](alpha-phase2-delivery-protocol.md): ten `fabric-node run` processes, each offered 2 lines/s of 512-byte bodies and sampling host metrics every 15 s, one `fabric-server`, TLS on loopback, 15 s warmup plus 120 s measured, three seeds per commit mode. The binaries and harness were frozen from commit `71fef99e51695f3db594ac29ba26425783f121ab`; their SHA-256 values are in [hashes.txt](data/alpha-phase2/tier10/hashes.txt). The runner is the frozen phase-0 runner (SHA-256 `2425e950…`). A first attempt at this run did not start any trial: the runner refused an output path under a copied runner location, exit 2 for all six, and nothing was measured. For each mode `M` and `N=1,2,3`:

```sh
python3 -B tools/alpha/runner.py --out target/alpha-p2-delivery-M-seedN --duration-s 300 --disk-bytes 1073741824 --max-output-bytes 1048576 -- python3 -B target/alpha-p2-frozen/tools/delivery_tier.py --seed 0xA11FA00N --mode M --bin-dir target/alpha-p2-frozen
```

All six runner invocations exited `0` with `passed=true`. Compact summaries and runner results are in [data/alpha-phase2/tier10](data/alpha-phase2/tier10/).

## Results

| Mode | Seed | ACK p50 ms | ACK p99 ms | Max backlog | Server frames | Server CPU s | Server VmHWM KiB | Max node VmHWM KiB |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| grouped | 1 | 57.7 | 64.3 | 0 | 136 | 0.33 | 9044 | 5420 |
| grouped | 2 | 55.6 | 60.5 | 0 | 136 | 0.32 | 8680 | 5424 |
| grouped | 3 | 57.5 | 63.4 | 0 | 136 | 0.34 | 8824 | 5428 |
| individual | 1 | 40.7 | 79.0 | 0 | 1360 | 0.81 | 8632 | 5404 |
| individual | 2 | 41.1 | 78.2 | 0 | 1360 | 0.81 | 8600 | 5420 |
| individual | 3 | 40.7 | 77.8 | 0 | 1360 | 0.81 | 8456 | 5428 |

Every trial recovered 1,360 batches, had 1,200 acknowledged attempts in the measured window, passed the delivery oracle, drained all offered lines, and every process exited 0.

## Interpretation

Both modes meet the 1 s ACK p99 gate by more than an order of magnitude, with no backlog at this load. Grouping put about ten batches in each frame, so the server wrote one tenth of the frames and used about 40% of the CPU. The cost is the 50 ms grouping window, visible as a higher median. Individual commits had the lower median but a higher p99, because ten nodes polling on the same second queue behind one another's syncs. This is one host at about ten batches per second; it does not predict behavior at the fleet tiers.

## Limits

All processes share one host and one disk, so network latency is loopback only. Node CPU includes each node's TLS client and one-second log polling.
