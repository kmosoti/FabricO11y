# History queries and freshness, run 02 (target profile)

Status: all four trials of [revision 1](alpha-phase4-history-protocol.md) of the history protocol, as registered, ran on 2026-09-28 on the 12-CPU target host and passed every gate. The history query, freshness and journal-versus-Segment capabilities are **Qualified** on the target profile for commit `e68d6ce`.

## Method

As registered in revision 1, with its default placement: the server on logical CPUs 0 to 3 and the simulator on CPUs 4 to 11. The binaries, the examples the harness launches and the harness were built and frozen with `tools/qualification/freeze.sh alpha-q1-frozen` at `e68d6ce1364ad02f2dfaa86e27922d3b8627155b`; their 28 SHA-256 values are in [hashes.txt](data/target-qualification/hashes.txt), and `sha256sum -c hashes.txt` reported 0 mismatches before the runs.

Host ([host.txt](data/target-qualification/alpha-q1-host.txt)): Intel Core i7-10750H, 12 logical CPUs, 15 GiB RAM, Debian GNU/Linux 13 under WSL2 with systemd, kernel `6.18.33.2-microsoft-standard-WSL2`, ext4, cgroup v2 with the `cpu`, `io`, `memory` and `pids` controllers listed, rustc 1.98.0. The trials ran one after another with nothing else heavy on the host.

For each mode `M` and seed `N`:

```sh
python3 -B tools/qualification/runner.py --out target/alpha-q1-history-M-seedN --duration-s 1800 \
  --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
  python3 -B $F/tools/history_tier.py --seed 0xA11FA00N --mode M --bin-dir $F
```

with `F=$PWD/target/alpha-q1-frozen`. All four runner invocations exited `0` with `passed=true` and no stop reason. Start and end times are in [progress.txt](data/target-qualification/history/progress.txt); each trial's runner result, summary and simulator summary are in [data/target-qualification/history](data/target-qualification/history/).

## Results

Query latencies are p99 over 20 instances each, in milliseconds, from request start to the full answer including every page. Freshness is in seconds after the 15 s warm-up. Live bytes are the journal, segments and checkpoint at rest after the queries.

| Mode | Seed | host logs | text search | metric history | fleet metrics | rate | Freshness p50 | Freshness p99 | Samples | Live MiB | Server VmHWM KiB | Trial s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| segment | 1 | 228 | 242 | 278 | 221 | 261 | 1.26 | 1.60 | 235 | 242.3 | 664,704 | 352 |
| segment | 2 | 229 | 229 | 279 | 224 | 263 | 1.22 | 1.62 | 235 | 242.3 | 668,076 | 354 |
| segment | 3 | 251 | 276 | 274 | 259 | 270 | 1.23 | 1.62 | 235 | 242.3 | 681,820 | 353 |
| journal only | 1 | 1,145 | 1,245 | 1,136 | 1,284 | 1,143 | 1.88 | 3.97 | 169 | 304.3 | 912,836 | 432 |

Each trial delivered all 250,000 batches with 0 undelivered, about 1,033,000 records under the protocol's workload arithmetic. In every trial all five graded answers (4,138 rows) passed the frozen query oracle, the prober had 0 errors, and the server and simulator exited 0.

| Gate | Rule | Result |
| --- | --- | --- |
| Query | every graded answer passes the oracle, and p99 ≤ 2 s for each kind | passed in all four trials; largest p99 279 ms with segments, 1,284 ms journal-only |
| Freshness | p99 ≤ 5 s in each segment-mode trial | passed; largest p99 1.62 s |
| Comparison | both modes' latency per kind and live bytes are reported | reported above; the comparison selects nothing |

## Interpretation

With the registered four-CPU server placement, segment-backed queries answer about 4 to 5 times faster at p99 than decoding the journal (228–279 ms against 1,136–1,284 ms), and take less space at rest (242 MiB against 304 MiB). These are slightly faster than [run 01](history-run-01.md), which gave the server two CPUs under revision 2. Journal-only freshness (3.97 s p99) is not gated and not comparable, because the prober's own queries are slower there and it collected fewer samples. No token index is added: the gate passed with row-group statistics and exact scans, as [ADR-0020](../../decisions/ADR-0020-store-sealed-history-as-parquet-segments.md) records.

## Limits

- **Samples.** One trial per seed and 20 instances per query kind, so each p99 is the largest of 20 samples.
- **Freshness** is an upper bound that includes up to 1 s of waiting for the next batch and not a real node's one-second log poll.
- **Environment.** WSL2 on a laptop CPU, loopback TLS, a single process per role and no power-loss testing. Server VmHWM of 649–666 MiB with segments (891 MiB journal-only) is well inside the 2 GiB central gate; this run does not attribute it.
