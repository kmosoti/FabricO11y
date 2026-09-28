# Registered phase-5 measurement: burst, rejection and concurrent management

Status: registered on 2026-09-28 before any stress run. Results go in a separate run record.

## Question

Under 1,000 simulated identities, with a 5× burst, rejected credentials, malformed batches and concurrent management and queries, does the server stay correct and bounded, and does the backlog recover after the burst?

## Method

[`stress_tier.py`](../../../tools/qualification/stress_tier.py) runs one `fabric-server` pinned to CPUs 0 to 3 and the fleet [simulator](../../../examples/spindle_sim.rs) on the other CPUs for 180 s with the frozen workload, except that seconds 60 to 79 offer five times the log rate. Concurrently:

- **rejection:** 200 times, one valid batch under a revoked node token, the same batch under an unknown token, and a malformed body under a valid token, all with fresh node identities;
- **management:** every 0.5 s, an inventory listing, a configuration change for a random identity, and a 30 s log query for a random node.

Seeds `0xA11FA001` to `0xA11FA003`, one trial each, under the frozen [runner](../../../tools/qualification/runner.py) with a 1,500 s duration limit, 5 GiB live-data limit and 1 MiB evidence limit.

## Decision rule

A trial passes when the delivery oracle passes over the simulator's transcript and `server_dump` (bytes projected to SHA-256 on both sides); revoked and unknown tokens always get 401 and malformed bodies 400 or 413; no batch from a rejected identity is recovered; the backlog 5 s before the end is at most one batch per identity above the backlog 5 s before the burst; server VmHWM is at most 2 GiB; every management call succeeds; and both processes exit 0. Latency during the burst is reported, not gated.

## Limits

One host and loopback TLS. The queue-full answer (503) is covered by its unit behaviour, not forced here.
