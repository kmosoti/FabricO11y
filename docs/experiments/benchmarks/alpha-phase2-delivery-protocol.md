# Registered phase-2 delivery measurement: ten real nodes

Status: registered on 2026-09-27 before any measurement (plan step 2.6). Results go in a separate run record.

## Question

With ten real `fabric-node` processes and one `fabric-server` on one host, under the registered per-node source shape, does delivery meet the phase-2 gates, and how do grouped and individual commits compare under identical durability?

## Workload

- Ten node processes, each with its own spool and its own log file. Each log is offered 2 lines/s of exactly 512 body bytes, alternating `R` × 512 and the phase-0 `entropy_body(seed, node, tick)`. Metrics are sampled every 15 s from the real host; the actual point count is reported, not the fleet fixture's 32. The node reads its log every second and commits a logs-only batch when new lines exist. This cadence was added to the node before any trial ran; it is recorded here as a protocol amendment, not a change after results.
- Seeds `0xA11FA001`, `0xA11FA002`, `0xA11FA003`, one trial per seed and commit mode, 15 s warmup plus 120 s measured, trials sequential.
- Commit modes, both through the same shared frame log with a data sync and a marker sync per frame: **grouped** (one frame per 50 ms or 1 MiB of batches) and **individual** (one frame per batch).
- TLS on loopback with throwaway certificates. Each trial runs under the frozen [runner](../../../tools/qualification/runner.py) with a 300 s duration limit, 1 GiB live-data limit and 1 MiB evidence limit.

## Metrics

- **ACK latency:** per acknowledged attempt, request start to answer as measured by the node (`elapsed_us`), which includes the server's durable commit. p50 and p99 over attempts started in the measured window.
- **Backlog:** every 5 s in the measured window, each node's committed sequence minus its acknowledged sequence, from its delivery lines and `fabricctl inspect`. Reported as the maximum per sample.
- **Correctness:** at the end, the [delivery oracle](../../../tools/qualification/DELIVERY_ORACLE.md) grades a transcript built as in the [fault runs](../formal/alpha-phase2-delivery-faults.md).
- **Resources:** server and maximum node VmHWM; server and total node CPU seconds over the measured window from `/proc/<pid>/stat`; all live bytes in server state, spools and source logs at the end.
- **Commit cost:** frames written per mode, from the server journal.

## Decision rule

A trial passes when the oracle passes, every process exits 0, the runner reports `passed=true`, ACK p99 is at most 1 s, and the maximum backlog in the last sample is no higher than in the first measured sample plus one batch. The native node gate stays 64 MiB VmHWM. The comparison reports both modes' ACK p50/p99, CPU and frames; it does not select a winner from one host. A budget stop or unrun trial is a failure, never a pass.

## Limits

Ten processes on one host share CPU and disk; this is not the 10/100/1000 identity fleet gate, which uses a simulated-identity process in phase 3. The load is about ten logs-only batches per second plus ten metric batches every 15 s, so grouping has little to group; the comparison is expected to show cost per frame rather than throughput.
