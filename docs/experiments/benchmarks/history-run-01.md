# History queries and freshness, run 01

Status: all four trials registered in [revision 2](history-protocol-r2.md) of the [history protocol](alpha-phase4-history-protocol.md) ran on 2026-09-28 and passed every gate. The state is **Measured and passing under revision 2 on a four-CPU host**. It is not a target-profile qualification: revision 1, on the 12-CPU target host, has still not run.

## Method

As registered in revision 2. The history protocol's fixture, workload, seeds, queries, grading and decision rule are unchanged. The only difference is placement: the server runs on logical CPUs 0 and 1, and the simulator on CPUs 2 and 3. The prober and harness are not pinned.

The binaries, the `spindle_sim` and `server_dump` examples, and the harness were built with `cargo build --release --locked --workspace --bins --examples` at `63bbeaca2f4bccbfea8bcfc1332321c19a039bc2`, the registration commit. They were then copied into `target/alpha-p4r2-frozen`; their 23 SHA-256 values are in [hashes.txt](data/history-qualification/hashes.txt) and were checked again after the run with `sha256sum -c` (0 mismatches). The [erratum](history-protocol-r2.md#erratum-before-any-measurement) commit that followed changed only documentation.

Host: 4 logical CPUs, 15 GiB RAM, Ubuntu 24.04 in a Firecracker VM, ext4, Python 3.11.15, rustc 1.94.1. Nothing else ran on the host during the trials.

The four trials ran one after another. For each mode `M` and seed `N`:

```sh
python3 -B tools/qualification/runner.py --out target/alpha-p4r2-M-seedN --duration-s 1800 \
  --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
  python3 -B $PWD/target/alpha-p4r2-frozen/tools/history_tier.py --seed 0xA11FA00N --mode M \
  --bin-dir $PWD/target/alpha-p4r2-frozen --server-cpus 0-1 --sim-cpus 2-3
```

All four runner invocations exited `0` with `passed=true`, no stop reason and process-group cleanup confirmed. The start and end times of each trial are in [progress.txt](data/history-qualification/progress.txt). Each trial's summary, runner result and simulator summary are in [data/history-qualification](data/history-qualification/).

An earlier attempt at 15:07:49Z gave relative paths. The runner starts its child in the output directory, so all four invocations exited `2` within 0.03 s and measured nothing. Their runner results are kept under [failed-start](data/history-qualification/failed-start/). The erratum records this, and the trials above are the first to measure anything.

## Results

Query latencies are p99 over 20 instances each, in milliseconds, from request start to the full answer including every page. Freshness is in seconds after the 15 s warm-up. Live bytes are the journal, segments and checkpoint at rest after the queries.

| Mode | Seed | host logs | text search | metric history | fleet metrics | rate | Freshness p50 | Freshness p99 | Samples | Live MiB | Server VmHWM KiB | Trial s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| segment | 1 | 362 | 465 | 445 | 475 | 402 | 1.31 | 1.70 | 235 | 242.2 | 679,332 | 386 |
| segment | 2 | 481 | 468 | 443 | 350 | 446 | 1.21 | 1.78 | 235 | 242.2 | 678,964 | 389 |
| segment | 3 | 385 | 320 | 426 | 380 | 400 | 1.28 | 1.66 | 235 | 242.2 | 845,364 | 379 |
| journal only | 1 | 1,510 | 1,410 | 1,515 | 1,455 | 1,492 | 1.90 | 3.92 | 167 | 304.3 | 921,608 | 477 |

Each trial delivered all 250,000 batches with 0 undelivered, according to the simulator summary. Under the workload arithmetic registered in the protocol, that is about 1,033,000 records; the harness counts batches, not records.

In every trial:

- all five graded answers (4,138 rows) passed the frozen query oracle with no violations;
- the prober had 0 errors;
- the server and the simulator both exited 0.

| Gate | Rule | Result |
| --- | --- | --- |
| Query | every graded answer passes the oracle, and p99 ≤ 2 s for each kind | passed in all four trials; the largest p99 is 481 ms with segments and 1,515 ms journal-only |
| Freshness | p99 ≤ 5 s in each segment-mode trial | passed; the largest p99 is 1.78 s |
| Comparison | both modes' latency per kind and live bytes are reported | reported above; the comparison selects nothing |

## Interpretation

Answering from segments is about four times faster than decoding the journal at the median and about three times faster at p99 (3.0 to 4.2 times per kind against segment seed 1, the same seed). Median query latency is about 280–350 ms with segments and about 1.32–1.34 s journal-only, for every query kind. Segments also take less space: 242 MiB (194 MiB of segments, 48 MiB of unsealed journal tail and a 153 KiB checkpoint) against 304 MiB of journal. The journal-only trial still meets the 2 s query gate, with about 25% headroom at p99.

Freshness p99 in journal mode (3.92 s) is not gated, and it is not comparable to the segment trials. The prober's own queries take about 1.3 s there, so it collected only 167 samples, and each sample includes one slow query.

The table suggests that segment-mode latency has a floor of about 280 ms whatever the query. That floor is plausibly the fixed cost of `fabricctl`, TLS and reading 3 or 4 segment manifests. It was not separately measured, and this run does not attribute it.

The registered plan item 4.5 adds a token index only if the 1,000,000-record query gate fails. The gate did not fail, so no index is added: queries use row-group statistics and an exact scan, as [ADR-0020](../../decisions/ADR-0020-store-sealed-history-as-parquet-segments.md) records.

## Limits

- **Host.** This is a four-CPU host, not the target profile. The server had two CPUs instead of four, and the prober and harness shared CPUs with the server and the simulator. A pass here shows the gates hold on a smaller host; it does not qualify the target profile.
- **Samples.** One trial per seed and 20 instances per query kind, so each p99 is the largest of 20 samples.
- **Freshness.** It is an upper bound that includes up to 1 s of waiting for the next batch. It does not include a real node's one-second log poll, because the simulator stamps its batch creation time.
- **Environment.** Loopback TLS, a single process per role and no power-loss testing.
