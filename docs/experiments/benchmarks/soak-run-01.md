# Soak, run 01

Status: the registered [soak protocol](soak-protocol.md) trial ran on 2026-09-28. It **failed** one gate, `no_rss_growth`, and passed the other nine. The state is **Failed** on the four-CPU host. The failure and the next action are below; the gate is not changed after the fact.

## Method

The trial ran as registered:

- 100 identities for 5,460 s: a 60 s warm-up, then nine windows of 600 s;
- sealing at 64 MiB;
- a log query every 10 s, and an inventory and configuration change every 60 s;
- server on CPUs 0-1, simulator on CPUs 2-3;
- seed `0xA11FA001`;
- the frozen runner, with a 7,200 s limit, a 5 GiB live-data limit and a 1 MiB evidence limit.

The binaries, examples and harness came from the stress run's frozen set (`2b5c939`, [28 SHA-256 values](../benchmarks/data/delivery-recovery/hashes-p5r2.txt)). `sha256sum -c` after the soak matched all 28. The host was otherwise idle from 17:41:52 to 19:13:45 UTC.

```sh
python3 -B tools/qualification/runner.py --out target/alpha-soak-seed1 --duration-s 7200 \
  --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
  python3 -B $PWD/target/alpha-p5r2-frozen/tools/soak_tier.py --seed 0xA11FA001 \
  --bin-dir $PWD/target/alpha-p5r2-frozen --server-cpus 0-1 --sim-cpus 2-3
```

The runner exited 1 with `passed=false` after 5,512 s. There was no stop reason; the peak live data was 968 MB, and cleanup was confirmed. The summary and runner result are in [data/delivery-recovery/soak](data/delivery-recovery/soak/).

## Results

| Window from s | ACK p50 ms | ACK p99 ms | ACKs | Server RSS median MiB | Server RSS max MiB | Server CPU s |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 60 | 54.4 | 61.8 | 60,000 | 139.5 | 489.1 | 15.5 |
| 660 | 54.5 | 61.9 | 60,000 | 489.3 | 522.5 | 15.4 |
| 1,260 | 54.4 | 61.5 | 60,000 | 522.6 | 522.6 | 15.6 |
| 1,860 | 54.6 | 61.3 | 60,000 | 533.8 | 533.8 | 16.3 |
| 2,460 | 54.6 | 61.6 | 60,000 | 533.8 | 533.8 | 17.6 |
| 3,060 | 54.2 | 62.6 | 60,000 | 533.9 | 533.9 | 17.6 |
| 3,660 | 54.3 | 61.5 | 60,000 | 535.5 | 535.5 | 18.8 |
| 4,260 | 54.2 | 61.2 | 60,000 | 535.5 | 535.7 | 16.8 |
| 4,860 | 54.0 | 63.6 | 60,000 | 535.7 | 536.5 | 17.0 |

| Gate | Rule | Result |
| --- | --- | --- |
| `oracle` | the delivery oracle passes | passed: 546,000 batches created, acknowledged and recovered, 0 violations |
| `exits` | the simulator and the server exit 0 | passed |
| `ack_p99_le_1s_every_window` | ACK p99 is at most 1 s in each window | passed: the largest was 63.6 ms |
| `no_growing_backlog` | the backlog 5 s before the end is at most 100 above the backlog at the end of the warm-up | passed: 0 and 0, and the largest sampled was 0 |
| `server_rss_le_2gib` | server VmHWM is at most 2 GiB | passed: 623 MiB |
| **`no_rss_growth`** | the last window's median RSS is at most twice the first window's plus 64 MiB | **failed**: 535.7 MiB against a threshold of 343.1 MiB (first-window median 139.5 MiB) |
| `queries_complete` | every query succeeds and every page is complete | passed: 546 queries, 0 failed, 0 incomplete |
| `query_p99_le_2s` | query p99 is at most 2 s | passed: 302 ms (p50 134 ms) |
| `management_ok` | every management call succeeds | passed: 91 of 91 |
| `sealing_caught_up` | no sealed journal file is left, and at least one Segment exists | passed: 10 Segments, 0 files left |

## Counterexample and analysis

Server RSS rises in one step and then stays nearly flat.

- **The step.** The first journal file seals about 9 minutes in, near the end of the first window. That window's maximum is already 489 MiB.
- **After the step.** From the third window to the ninth, the median moves from 522.6 to 535.7 MiB, while seven more Segments are sealed: about 2 MiB per seal.
- **At the end.** The high-water mark, 623 MiB, lies between the two.
- **Not file pages.** A sample at about 62 minutes showed 525 MiB of the RSS as anonymous memory and 10.6 MiB as file-backed.

This pattern fits the sealer's transient working set being kept by the allocator after each seal. It does not fit a leak that grows with delivered data.

[`segment::build`](../../../crates/fabric-server/src/segment.rs) holds all of these in memory at the same time:

- a whole 64 MiB sealed journal file, decoded into groups;
- a second copy of every entry, for the `batches` table;
- the projected log and metric rows;
- their Arrow arrays;
- each table's Parquet output.

The high-water mark of 623 MiB is about ten times the 64 MiB file. The memory is freed only after the build.

The registered gate compares the last window with a first window that mostly predates the first seal. A step to a stable plateau therefore fails it just as a leak would. The gate still stands: an RSS that stays nearly four times higher than before sealing is a finding in its own right, and the 2 GiB gate passes with a large margin.

**Next action.** Bound the sealer's working set: stream groups into row groups instead of materializing the whole file, and stop cloning entries. Then rerun this registered soak unchanged as run 02. The failure is recorded in the [capability ledger](../../QUALIFICATION.md#capability-ledger) and the [current state](../../CURRENT.md); the counterexample registry takes an entry once there is a fix commit and a reproducer.

## Limits

- **Scale.** One trial, one host, loopback TLS and 100 simulated identities. Retention is not reached.
- **Sampling.** RSS was sampled every 5 s, so a peak shorter than that may be missed; VmHWM covers it.
