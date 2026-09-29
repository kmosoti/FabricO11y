# Soak, run 02 (target profile)

Status: the registered [soak protocol](soak-protocol.md) trial ran on 2026-09-29 on the 12-CPU target host with the target placement. It **failed** one gate, `no_rss_growth`, and passed the other nine, as the [qualification runbook](../../qualification-runbook.md) predicted. The state is **Failed** on the target profile for commit `e68d6ce`. The sealer fix named in [soak run 01](soak-run-01.md) has not landed, so this is the same binary behaviour measured on the target host, not the rerun that follows the fix.

## Method

As registered, with the target-profile placement: 100 identities for 5,460 s (a 60 s warm-up, then nine windows of 600 s), sealing at 64 MiB, a log query every 10 s, an inventory and configuration change every 60 s, the server on CPUs 0 to 3 and the simulator on CPUs 4 to 11, seed `0xA11FA001`. The artifacts are the set frozen from `e68d6ce1364ad02f2dfaa86e27922d3b8627155b` described in [history run 02](history-run-02.md), on the same [host](data/target-qualification/alpha-q1-host.txt); `sha256sum -c hashes.txt` after the soak matched all 28 files. No other heavy program ran on the host during the trial.

```sh
python3 -B tools/qualification/runner.py --out target/alpha-q1-soak-seed1 --duration-s 7200 \
  --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
  python3 -B $F/tools/soak_tier.py --seed 0xA11FA001 --bin-dir $F --server-cpus 0-3 --sim-cpus 4-11
```

The runner exited `1` with `passed=false` after 5,499 s. There was no stop reason; the peak live data was 967 MB, and process-group cleanup was confirmed. Start and end times are in [progress.txt](data/target-qualification/soak/progress.txt); the runner result, summary and simulator summary are in [data/target-qualification/soak](data/target-qualification/soak/).

## Results

| Window from s | ACK p50 ms | ACK p99 ms | ACKs | Server RSS median MiB | Server RSS max MiB | Server CPU s |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 60 | 62.4 | 69.9 | 60,000 | 138.4 | 457.8 | 20.9 |
| 660 | 62.8 | 69.8 | 60,000 | 481.8 | 523.9 | 20.1 |
| 1,260 | 62.8 | 69.9 | 60,000 | 525.5 | 527.7 | 20.3 |
| 1,860 | 62.6 | 69.5 | 60,000 | 533.3 | 533.3 | 20.5 |
| 2,460 | 62.8 | 69.9 | 60,000 | 533.5 | 621.2 | 20.8 |
| 3,060 | 62.8 | 71.4 | 60,000 | 537.0 | 537.0 | 20.9 |
| 3,660 | 62.8 | 70.9 | 60,000 | 538.3 | 538.3 | 21.7 |
| 4,260 | 62.7 | 69.1 | 60,000 | 538.3 | 538.5 | 20.1 |
| 4,860 | 62.7 | 70.0 | 60,000 | 538.5 | 538.5 | 20.1 |

| Gate | Rule | Result |
| --- | --- | --- |
| `oracle` | the delivery oracle passes | passed: 546,000 batches created, acknowledged and recovered, 0 violations |
| `exits` | the simulator and the server exit 0 | passed |
| `ack_p99_le_1s_every_window` | ACK p99 is at most 1 s in each window | passed: the largest was 71.4 ms |
| `no_growing_backlog` | the backlog 5 s before the end is at most 100 above the backlog at the end of the warm-up | passed: 0 and 0, and the largest sampled was 0 |
| `server_rss_le_2gib` | server VmHWM is at most 2 GiB | passed: 625.8 MiB |
| **`no_rss_growth`** | the last window's median RSS is at most twice the first window's plus 64 MiB | **failed**: 538.5 MiB against a threshold of 340.8 MiB (first-window median 138.4 MiB) |
| `queries_complete` | every query succeeds and every page is complete | passed: 546 queries, 0 failed, 0 incomplete |
| `query_p99_le_2s` | query p99 is at most 2 s | passed: 230 ms (p50 111 ms) |
| `management_ok` | every management call succeeds | passed: 91 of 91 |
| `sealing_caught_up` | no sealed journal file is left, and at least one Segment exists | passed: 10 Segments, 0 files left |

## Counterexample and interpretation

The failure reproduces [soak run 01](soak-run-01.md) on the target host: server RSS steps from about 138 MiB to about 480 MiB when the first journal file seals, near the end of the first window, then rises slowly to a plateau near 538 MiB. The 621 MiB maximum in the window from 2,460 s is one sample during a later seal, and VmHWM is 626 MiB, about ten times the 64 MiB file, against 623 MiB on the four-CPU host. The shape again fits the sealer's transient working set being retained by the allocator, not a leak that grows with delivered data; the plateau grew by 5 MiB over the last five windows.

Everything else held with margin on four server CPUs: ACK p99 stayed near 70 ms in every window, the backlog stayed at 0, and query p99 was 230 ms. ACK latency is about 8 ms higher than on the four-CPU host at every percentile; that difference was not investigated.

**Next action**, unchanged: bound the sealer's working set in `segment::build` (stream groups into row groups instead of materializing the whole file, and stop cloning entries), freeze again, and rerun this registered soak unchanged. The gate is not changed after the fact.

## Limits

- **Scale.** One trial, one host, loopback TLS and 100 simulated identities. Retention is not reached.
- **Sampling.** RSS was sampled every 5 s, so a peak shorter than that may be missed; VmHWM covers it.
