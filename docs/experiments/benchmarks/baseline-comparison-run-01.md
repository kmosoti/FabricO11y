# Baseline comparison, run 01

Status: the registered [baseline comparison](baseline-comparison-protocol.md) ran on 2026-09-29 on the 12-CPU target host. Fabric, ClickHouse and Elasticsearch answered all 80 queries exactly in the first pass. The commercial log platform's first pass was inexact on two query kinds because its adapter had a tie-ordering defect. After the defect was fixed, its rerun answered all 80 exactly. The comparison is reported, not gated, and selects nothing. Following the protocol, the commercial platform appears only under its generic category.

## Method

As registered: the Segment-mode fixture of [history run 02](history-run-02.md), seed `0xA11FA001`, read with `server_dump --records` from the frozen binaries (`e68d6ce`) and decoded by the frozen query oracle into 500,000 log rows and 544,000 metric points. The same rows were loaded into every baseline. Fabric served the fixture's own state, copied, through the frozen `fabric-server`. Each system ran alone on logical CPUs 0 to 3, bound to loopback, with the schema in the protocol. Each answered 20 seeded instances of four query kinds with one request for the first `limit` rows, and each answer was compared with the oracle's expected rows. The [host](data/target-qualification/alpha-q1-host.txt) is the target-qualification host, and nothing else heavy ran; the idle commercial platform stayed pinned to CPU 11 while the other systems ran.

The harness is [`baseline_compare.py`](../../../tools/qualification/baseline_compare.py) as committed in `baf90b8`, with SHA-256 `29cdc58e2c137a0a15fe694d64f9c0329dc9e9bf0beef78fa4908dd84b220599`. It is not run under the qualification runner, because ClickHouse keeps symlinks in its data directory, which the runner's owned-tree scan refuses. The commercial platform's adapter and unredacted results are kept outside the repository.

```sh
python3 -B tools/qualification/baseline_compare.py --out target/alpha-cmp-01 \
  --fixture $PWD/target/alpha-q1-history-segment-seed1 --bin-dir $PWD/target/alpha-q1-frozen \
  --baselines <baselines> --systems fabric,clickhouse,elasticsearch,commercial \
  --private-adapter <private adapter> --private-out <private results>
```

The command exited `0` after 266 s. After the adapter fix, the same command ran with `--systems commercial --out target/alpha-cmp-02` and exited `0` after 119 s ([progress.txt](data/baseline-comparison/progress.txt)). The public summaries are in [run-01](data/baseline-comparison/run-01/comparison-summary.json) and [commercial-rerun](data/baseline-comparison/commercial-rerun/comparison-summary.json).

## Results

Latency is milliseconds from request start to the parsed answer in the Python client. Each cell shows exact answers out of 20, then p50 / p99. For the commercial platform the table shows the rerun, and the first pass is shown below it.

| Query kind (limit) | Fabric | ClickHouse | Elasticsearch | Commercial log platform |
| --- | --- | --- | --- | --- |
| host logs (1,000) | 20 · 220.1 / 316.6 | 20 · 13.1 / 18.3 | 20 · 29.7 / 148.2 | 20 · 201.2 / 348.3 |
| text search (100) | 20 · 219.0 / 254.7 | 20 · 81.7 / 124.6 | 20 · 11.9 / 78.1 | 20 · 994.1 / 1,182.8 |
| metric history (1,000) | 20 · 240.7 / 359.0 | 20 · 13.0 / 19.2 | 20 · 6.3 / 9.9 | 20 · 112.2 / 235.9 |
| fleet metrics (10,000) | 20 · 216.7 / 236.3 | 20 · 43.9 / 58.8 | 20 · 72.0 / 215.3 | 20 · 469.5 / 894.1 |

| System | Load s | Bytes at rest MiB | Peak RSS during queries MiB |
| --- | ---: | ---: | ---: |
| Fabric | not measured (loaded through its durable delivery path by the fixture run) | 242.6 | 165.6 |
| ClickHouse | 9.2 | 216.2 | 1,029.7 |
| Elasticsearch | 75.8 | 548.0 | 8,667.3 |
| Commercial log platform | 64.4 | 860.3 | 1,182.2 |

**The commercial platform's first pass** was exact on text search and metric history, and 0 of 20 on host logs and fleet metrics. It loaded in 64.2 s, used 788.7 MiB at rest and peaked at 1,261.6 MiB RSS. A diagnostic run showed that the returned rows were exactly the expected set, with no duplicates, but ties were ordered differently. The adapter's sort read digit-led identity strings as numbers, and one sort field's name collided with a reserved field name. Only answers with ties on time failed, which is why the two kinds with small answers passed. The fix sorts on one fixed-width string key built from the registered order `(time, node identity, sequence, index)`, which is consistent with the protocol's fixed-width time comparison. With the fix, the first instance of each kind matched the oracle, and then the full rerun was exact.

## Interpretation

Every system can answer these questions exactly, and on this host both ClickHouse and Elasticsearch answer most of them an order of magnitude faster than Fabric. Fabric's latency is nearly the same, about 220 ms at p50, for every kind, including a text search that returns one row. That suggests a fixed per-request cost in the query path rather than scan work; it was not investigated. The history run's p99s have the same floor.

Fabric used the least memory of the four, about 166 MiB against 1 GiB or more for the others. Its bytes at rest were close to ClickHouse's, although Fabric's include the unsealed journal tail and batch records. Elasticsearch's memory is mostly its default JVM heap, which its log reports as 7.7 GB. These are single-node defaults with no tuning, so none of them says how the systems compare when tuned.

## Limits

- **Setup.** One host, one fixture and 20 instances per kind, so each p99 is the largest of 20 samples.
- **Durability and loading.** The baselines were bulk-loaded after the fixture existed, with their default durability. Fabric committed every batch with its two-sync rule while ingesting, so load times are not comparable, and Fabric's is not measured.
- **Client placement.** The protocol places the loader and query client on the other CPUs, but the harness does not pin itself, so the client may have shared CPUs 0 to 3 with the system under test. Latency also includes each client's parsing of the answer.
- **Memory sampling.** Peak RSS is sampled once after each query, not continuously.
- **Rerun.** The commercial rerun ran after, not alongside, the other systems, on the same idle host. Its first pass is kept above, not replaced.
