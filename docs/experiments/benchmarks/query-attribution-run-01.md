# Query attribution, run 01

Status: **Exploratory.** This run splits a query's time between the unsealed journal tail, the Segment scan and the command-line client, on one state. No protocol was registered before it ran, so it is not **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense and decides no gate. It ran on 2026-10-01 with binaries built from the code at `58b694d`.

The question: when a query is slow, which part of the [retained-history](../../architecture/retained-history.md) read path is slow, and does the answer depend on what the query asks for?

Origin: [history run 01](history-run-01.md) saw a floor of about 280 ms whatever the query and did not attribute it; [collection to query latency run 01](e2e-latency-run-01.md) saw the same query go from 2.8 ms to 50 ms as the unsealed journal grew to 40 MiB.

## Method

The state is the one the loaded end-to-end run left behind: a 40,905,199-byte unsealed journal holding about 7.5 minutes from 100 simulated identities and the probe node, 63,950 log rows and 71,300 metric points, and no Segment.

Two servers, each on its own copy of that state, each on CPUs 0 and 1:

- **A, tail.** Default configuration (64 MiB journal files). The whole state is unsealed tail, which [`History::sources`](../../../crates/fabric-server/src/query.rs) decodes in full on every query.
- **B, sealed.** `journal_file_bytes=65536`. One appended Batch (a two-second `spindle_sim` run with one identity) rotated the 40 MiB file, and the sealer built one Segment from it: 30,501,514 bytes (`batches.parquet` 15.98 MB, `logs.parquet` 13.76 MB in 8 row groups, `metrics.parquet` 0.75 MB in 9 row groups, `gaps.parquet` 754 B). The tail left was 3,833 bytes.

Eight query shapes, each 12 times over one keep-alive HTTPS connection from an unpinned client, reporting the median; then `fabricctl admin ... query` five times for two of them. The window `[a, b]` spans the whole state. The time to the first successful query after `fabric-server` started is also recorded; it is the journal replay.

| Shape | Query |
| --- | --- |
| empty window | logs, `from_ns` 1 to 2: nothing can match |
| host logs | logs of `sim0007`, limit 1,000 |
| logs limit 50 | logs of every node, limit 50 |
| text, rare | logs containing `zq9`, limit 100 |
| text, common | logs containing `RRRRRRRR`, limit 100 (half of all bodies match) |
| metric history | metrics of `sim0003` named `sim.metric.7`, limit 1,000 |
| fleet metrics | metrics named `sim.metric.3` from every node, limit 10,000 |
| rate | rate of `sim.metric.0`; the simulator's metrics are gauges, so no rows are produced, but the scan runs |

## Results

Median milliseconds per query, and the rows each returned:

| Shape | A: 40 MiB unsealed tail | B: one Segment | Rows |
| --- | ---: | ---: | ---: |
| empty window | 126.0 | 1.1 | 0 |
| host logs | 129.3 | 43.7 | 638 |
| logs limit 50 | 139.5 | 39.4 | 50 |
| text, rare | 136.7 | 40.0 | 33 |
| text, common | 133.0 | 42.6 | 100 |
| metric history | 124.6 | 17.0 | 22 |
| fleet metrics | 151.9 | 33.1 | 2,200 |
| rate | 126.1 | 16.8 | 0 |

Through `fabricctl` (a process start and a TLS handshake per query): host logs 151.5 ms on A and 50.6 ms on B; the empty window 149.9 ms and 48.0 ms.

Time to the first successful query after start: 1.6 s on A (40 MiB replay), 0.5 s on B.

Row-group statistics of the Segment's logs table (`sealbench verify`, [sealer study](sealer-study-run-01.md) harness): 8 row groups with disjoint time ranges; a 60 s window reads 1.64 rows per row returned, a 10 s window 4.94.

## Findings

- **With an unsealed tail, every query costs the tail, whatever it asks.** The empty window, which can match nothing, took 126 ms; the most selective and the least selective shapes took 125 to 152 ms. The read path loads every frame of every journal file, decodes every Group and every OTLP payload, and only then filters. That is about 3.2 ms per MiB of tail on this host. A tail grows to the 64 MiB file size before each seal, so this fixed cost would reach about 200 ms per query at the end of every sealing cycle.
- **Sealing removes the fixed cost.** The empty window fell from 126 ms to 1.1 ms: the manifests and `gaps.parquet` are all a query reads when no row group's time range overlaps its window.
- **Inside a Segment, the cost is the rows materialised, not the rows returned.** Over the whole window, logs queries took about 40 ms whether they returned 50 or 638 rows, and whether the node filter kept 1 % or the text filter kept half; the scan builds every column of every row in the 8 surviving row groups, including the 512-byte body and the attribute map, before the filter runs. Metric queries took 17 ms over 71,300 points in 0.75 MB. Limit 50 cost the same as limit 1,000, because the scan does not stop when the bounded heap can no longer change.
- **The CLI adds about 25 to 47 ms.** That is a process start and a TLS handshake; the engine is not involved.
- **Consistent with history run 01.** Its 48 MiB tail, its `fabricctl` client and its three or four Segments account for a floor of a few hundred milliseconds whatever the query; this run did not reproduce that fixture, so this is consistency, not attribution.

The [query-engine direction review](../../research/query-engine-direction.md) uses these numbers.

## Limits

- One state of 40 MiB from 100 identities, and one host. The per-row costs were not profiled below the query level.
- A 60 s window on this state is read-amplification 1.64; the estimates of what late materialisation and early termination would save, in the direction review, are arithmetic over these row-group statistics, not measurements.
- The first attempt at scenario A failed its readiness probe after 30 s although the server had reported listening; the second attempt was ready in 1.6 s. The cause was not found; both attempts are in the harness log.

## Reproduce

[qattr.py](data/query-latency/qattr.py.txt) takes the source state directory and an output root; results are in [query-attribution.json](data/query-latency/query-attribution.json).

```sh
python3 -B qattr.py target/e2e-loaded/server-state target/qattr
```
