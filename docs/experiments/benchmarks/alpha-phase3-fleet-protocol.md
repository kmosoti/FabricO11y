# Registered phase-3 fleet measurement: simulated identities

Status: registered on 2026-09-28 before any fleet measurement (plan steps 3.4 and 3.5). Results go in a separate run record.

## Question

With one real `fabric-server` and 10, 100 and 1,000 enrolled node identities driven by one simulator process, does the server keep up with the frozen open-loop workload, and does a configuration change reach every identity within 30 s?

## Workload

- The [simulator](../../../examples/spindle_sim.rs) enrolls nothing itself. The [harness](../../../tools/qualification/fleet_tier.py) enrolls each identity through the admin API and hands the simulator the tokens. Each identity has its own random node identity, TLS connection and bearer token.
- Per identity per second: two log records with 512-byte bodies, alternating `R` × 512 and the phase-0 `entropy_body(seed, identity, tick)`. Every 15 s: 32 gauge points with values derived from SHA-256, as in the phase-0 workload. Each second's records form one batch, so an identity offers one batch per second. The schedule is open loop: batches are created on time whether or not earlier ones were acknowledged.
- Seeds `0xA11FA001`, `0xA11FA002`, `0xA11FA003`, one trial per seed and tier, 15 s warmup plus 120 s measured, trials sequential. Grouped commit.
- The server is pinned to logical CPUs 0 to 3 with `taskset`; the simulator uses the remaining CPUs. Each trial runs under the frozen [runner](../../../tools/qualification/runner.py) with a 900 s duration limit, 5 GiB live-data limit and 1 MiB evidence limit.
- At 60 s the harness changes every identity's configuration (interval 30 s) and records when each change was accepted.

The simulator keeps unacknowledged batches in memory, not in a synced spool. Node-side crash behavior is covered by the phase-2 real-process runs; this measurement is about the server. The simulator does not represent 1,000 deployed hosts or networks.

## Metrics

- **Durable ACK latency:** request start to `ack`, for attempts started in the measured window; p50 and p99.
- **Delivery latency:** batch creation to its `ack`, which includes queueing behind earlier batches; p50 and p99.
- **Backlog:** batches created and not yet acknowledged, across all identities, every 5 s of the measured window.
- **Apply latency:** per identity, from its accepted change to the simulator's poll that applied it; p50, p99 and maximum. The inventory's count of identities whose applied revision equals the desired one is also reported.
- **Correctness:** the frozen [delivery oracle](../../../tools/qualification/DELIVERY_ORACLE.md) over the simulator's transcript and `server_dump`, with every batch's bytes replaced on both sides by their SHA-256. That projection preserves byte equality under the usual collision assumption and keeps the transcript within the live-data budget.
- **Resources:** server VmHWM and sampled RSS, server CPU seconds over the measured window, all live bytes at the end, and the enrollment time.

## Decision rule

A trial passes when the oracle passes, the simulator and server exit 0, ACK p99 is at most 1 s, the last backlog sample is no more than one batch per identity above the first, every identity applied the change within 30 s, and server VmHWM is at most 2 GiB. The observation-to-query freshness and query latency gates need the phase-4 query path and are not measured here. A budget stop or an unrun trial is a failure.
