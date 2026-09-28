# Registered phase-4 measurement: history queries and freshness

Status: registered on 2026-09-28 before any phase-4 measurement (plan steps 4.5 and 4.6). Results go in a separate run record.

## Questions

1. Over a fixture of at least 1,000,000 telemetry records, do the registered queries answer exactly and with p99 at most 2 s?
2. At 1,000 identities, is observation-to-query freshness p99 at most 5 s?
3. How do journal-only and segment-backed reads compare in latency and live bytes for the same data and queries?

## Fixture and workload

The fleet [simulator](../../../examples/spindle_sim.rs) drives 1,000 enrolled identities with the frozen workload (two 512-byte log records per identity per second and 32 metric points every 15 s) for 250 s, about 1,033,000 records in 250,000 batches. Seeds `0xA11FA001` to `0xA11FA003`, one trial each, in **segment** mode (journal files sealed at 64 MiB into Zstd Parquet segments). One further trial with seed `0xA11FA001` runs in **journal-only** mode (journal file size above the fixture, so nothing is sealed). The server is pinned to logical CPUs 0 to 3 with `taskset`, the simulator to the rest. Each trial runs under the frozen [runner](../../../tools/qualification/runner.py) with a 1,800 s duration limit, 5 GiB live-data limit and 1 MiB evidence limit.

## Freshness

While the simulator runs, a prober queries one random identity every second: logs from that node in the last 10 s, limit 10. For each answer, the sample is the wall-clock time at receipt minus that node's `freshness` value. Each identity offers a batch every second, so a sample includes up to about 1 s of waiting for the next batch; the measure is an upper bound on observation-to-query latency. p50 and p99 over samples after the 15 s warmup.

## Registered queries

After the simulator ends and, in segment mode, after every sealed journal file has become a segment, 20 instances of each query run in a seeded random order, with node and 60 s windows drawn from the fixture's range:

- **host logs:** logs from one node in a 60 s window, limit 1,000;
- **text search:** logs containing a fixed 12-character substring of one high-entropy body, across all nodes, in a 60 s window, limit 100;
- **metric history:** one metric name from one node over the whole fixture, limit 1,000;
- **fleet metrics:** one metric name across all nodes in a 60 s window, limit 10,000, following every page;
- **rate:** the gauge metric name used by the simulator, which has no monotonic sums, so the answer is empty and the cost is the scan.

Latency is request start to full answer, including every page of a paginated query. Correctness: one instance of each query is graded by the frozen [query oracle](../../../tools/qualification/QUERY_ORACLE.md) over the server's retained records.

## Decision rule

The query gate passes when every graded answer passes the oracle and the p99 latency of each query kind is at most 2 s. The freshness gate passes when freshness p99 is at most 5 s in each segment-mode trial. The comparison reports both modes' latency per kind and their live bytes (journal files, segments and checkpoint); it selects nothing on its own. A budget stop or unrun trial is a failure.

## Limits

One host; the simulator shares the host with the server. The simulator's `observed_time_unix_nano` is its batch creation time, so freshness includes delivery and commit but not the time a real log line waits for a node's one-second poll.
