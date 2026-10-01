# Collection to query latency, run 01

Status: **Exploratory.** This run measured where time goes between a log line being written on a host and a query returning it. No protocol was registered before it ran, so it is not **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense and decides no gate. It ran on 2026-10-01 with binaries built from the code at `58b694d` (verification tooling merged; the commits above it on the branch change only documents).

The question: for one line, how long until a query can return it, and which stage takes the time?

## Method

One real `fabric-server` (CPUs 0 and 1) and one real `fabric-node` (CPUs 2 and 3), over TLS on loopback, on this 4-CPU container's ext4 disk. The node is enrolled with one watched log file and a 15 s metric interval. The harness appends 150 lines, each carrying a unique tag, at intervals drawn from 1.3 to 2.7 s so that they do not align with the node's 1 s log poll. A query thread asks every 10 ms, over one keep-alive HTTPS connection, for log rows of the probe node containing the tag prefix, over a window of the last two minutes.

Six timestamps per line, all from the host's `CLOCK_REALTIME`:

| Point | Taken from | Meaning |
| --- | --- | --- |
| t0 | the harness, after `flush` | the line is in the file |
| t1 | `observed_ns` in the query row | the Spindle cycle that read the line began |
| t2 | the node's `batch=` stdout line, timestamped on arrival | the Batch is committed to the Spool |
| t3 | `received_ns` in the server journal (`server_dump --records`) | the server's commit group took the Batch |
| t4 | the node's `delivery ... status=ack` stdout line | the ACK reached the node |
| t5 | the harness | the first query whose answer held the line was sent |

Stages are differences: collection wait (t0 to t1), Spool commit (t1 to t2), transit and group window (t2 to t3), server commit and ACK (t3 to t4), visible (t3 to t5, which includes up to 10 ms of polling and the query's own run) and total (t0 to t5). Stdout is line-buffered, and the node prints each line before persisting the state it reports.

Two scenarios: **idle**, nothing but the probe node; **loaded**, the same probe while [spindle_sim](../../../examples/spindle_sim.rs) drives 100 identities at the soak workload (two 512-byte log lines per identity per second, 32 metric points every 15 s) on CPUs 2 and 3, after a 20 s warm-up.

## Results

Idle, 150 of 150 lines returned, 161 Batches committed and acknowledged, milliseconds:

| Stage | p50 | p90 | p99 | max |
| --- | ---: | ---: | ---: | ---: |
| Collection wait | 442.3 | 885.1 | 983.2 | 998.0 |
| Spool commit | 2.8 | 4.0 | 11.2 | 442.1 |
| Transit and group window | 50.7 | 50.9 | 52.6 | 68.7 |
| Server commit and ACK | 3.2 | 4.8 | 21.0 | 37.5 |
| Visible to a query | 9.5 | 14.6 | 24.3 | 45.6 |
| **Total** | **516.5** | **950.5** | **1,042.4** | **1,060.0** |

Loaded, 150 of 150 lines returned, 161 Batches committed and acknowledged, milliseconds:

| Stage | p50 | p90 | p99 | max |
| --- | ---: | ---: | ---: | ---: |
| Collection wait | 442.3 | 883.0 | 980.4 | 997.7 |
| Spool commit | 2.8 | 4.8 | 10.8 | 12.4 |
| Transit and group window | 50.8 | 51.1 | 53.2 | 53.7 |
| Server commit and ACK | 4.5 | 7.1 | 10.4 | 12.9 |
| Visible to a query | 45.4 | 105.9 | 129.7 | 153.0 |
| **Total** | **548.4** | **980.7** | **1,089.4** | **1,130.4** |

The node's own request round trip (`elapsed_us` in its delivery line): p50 53.9 ms, p99 79.7 ms idle; p50 54.7 ms, p99 60.6 ms loaded.

The polling query itself (keep-alive, logs of one node, two-minute window): idle, 22,738 queries, p50 2.81 ms, p90 3.76 ms, p99 5.6 ms, max 27.3 ms; loaded, 4,231 queries, p50 49.6 ms, p90 119.7 ms, p99 149.6 ms, max 316.3 ms. The server state was 164 KiB at the end of the idle run and 40 MiB, all unsealed journal, at the end of the loaded run.

## Findings

- **The 1 s log poll is about 85 % of the median.** `LOG_POLL` in [fabric-node](../../../src/bin/fabric-node.rs) is one second, so a line waits 0 to 1 s, 442 ms at the median in both scenarios. No other stage is within an order of magnitude.
- **The 50 ms group window is a fixed cost.** The server's grouped commit holds every group open for 50 ms ([store.rs](../../../crates/fabric-server/src/store.rs), `CommitMode::GROUPED`), so transit and grouping took 50.7 ms at the median whether one Batch or a hundred was waiting. The transit itself is under 1 ms on loopback.
- **A query's cost follows the size of the unsealed journal.** Idle, with a few hundred kilobytes of journal, the probe query took 2.8 ms; loaded, with 40 MiB of unsealed journal, 50 ms at the median and 150 ms at p99, and the "visible" stage grew by the same amount. [Query attribution run 01](query-attribution-run-01.md) measures this cost on its own.
- **Spool, network and journal commit are small.** Together they take under 10 ms at the median. Batches that carried host metrics committed to the Spool in 2.8 to 10.8 ms, no slower than log-only Batches.
- **One outlier.** The idle run's second Batch took 442 ms to commit to the Spool, during the node's start-up; no other Spool commit exceeded 12.4 ms.
- **Nothing was lost or delayed by load.** Every line arrived in both scenarios; the stages up to the ACK did not change under load, only the query did.

## Limits

- One host and loopback: no real network delay, and the 50 ms window is the whole transit cost here.
- Ext4 on a VM disk; fsync cost on the target profile is unmeasured.
- 150 lines per scenario, so a p99 is close to the second-largest sample.
- The probe's t0 is the harness's own write; an application that buffers its log would add its own delay before t0.
- The loaded scenario shares CPUs 2 and 3 between the probe node and the simulator; the collection stages did not change, so the sharing did not matter at this rate.

## Reproduce

The harness is kept as text beside the data: [e2e_latency.py](data/query-latency/e2e_latency.py.txt) and [analyze.py](data/query-latency/analyze.py.txt). It reuses `make_certs` and `free_port` from [delivery_faults.py](../../../tools/qualification/delivery_faults.py).

```sh
cargo build --release --locked --workspace --bins --examples
python3 -B e2e_latency.py --out target/e2e-idle --lines 150
python3 -B e2e_latency.py --out target/e2e-loaded --lines 150 --load 100
python3 -B analyze.py idle loaded
```

Per-line stage times and the query percentiles are in [e2e-idle.json](data/query-latency/e2e-idle.json) and [e2e-loaded.json](data/query-latency/e2e-loaded.json).
