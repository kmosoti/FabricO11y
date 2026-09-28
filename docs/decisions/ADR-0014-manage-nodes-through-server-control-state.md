# ADR-0014: Manage nodes through one server control state and node polling

## Status

Accepted on 2026-09-28 for alpha phase 3, after the end-to-end control test and the [fleet trials](../experiments/benchmarks/alpha-phase3-fleet-run-01.md) passed.

## Context

Phase 3 requires enrollment, inventory, per-node source configuration and intervals, pause, resume and revoke, desired and applied revision tracking, validation before activation, nodes that keep their last valid configuration while the server is unreachable, and a healthy apply within 30 s at 10, 100 and 1,000 identities. Nodes connect outbound only. Admin credentials must be distinct from node credentials. The phase-2 static credentials file has no revoke or enrollment.

## Decision

- The server keeps one control state in `state_dir/control.json`, replaced by synced write-to-temp and rename on every administrative change. It holds, per node name: the SHA-256 of its bearer token, its status (active, paused or revoked), and its desired configuration with a revision number. The static credentials file is removed.
- An admin bearer token, read from a root-owned file named by `admin_token_file`, guards `/v1/admin/*`: enroll (returns the node token once), list, set configuration, pause, resume and revoke. Node tokens never authorize admin routes and the admin token never authorizes batch intake.
- Every change to a node bumps its revision. The node polls `GET /v1/config` every 5 s with its applied revision; the server answers 304 when nothing changed. The poll also reports the node's applied revision and any validation error, which the server keeps in memory and shows in the inventory.
- Remote configuration may set only the log paths and the metric interval. The spool ceiling and the server target stay local. The node validates a new configuration with the same `Config::validate` as a local file, writes it to `applied-config.json` in its spool by synced rename, then activates it. An invalid configuration is reported and not activated. On restart the node uses its last applied configuration, so it keeps working while the server is down.
- Pause stops collection and keeps delivering already committed batches. Resume records one gap for the paused interval. Revoke removes the token hash, so batch intake and polling answer 401 and the node keeps its spool.

## Alternatives considered

- Push configuration over a server-initiated connection: nodes connect outbound only.
- A database for control state: at most 1,000 small records; one atomically replaced file is enough and inspectable.
- Remote spool ceilings: they would let a remote admin exceed a host's local disk budget.

## Consequences

Apply latency is bounded by the 5 s poll plus one poll round trip. At 1,000 identities the server answers about 200 polls per second, mostly 304. Applied revision and errors are observations, not durable state, so after a server restart they are unknown until each node's next poll.

## Validation

Phase-3 tests: invalid and stale configurations are refused and reported; a node restarted while the server is down keeps its last applied configuration; a revoked token gets 401; pause and resume leave one gap; live reconfiguration takes effect; healthy apply is at most 30 s at each identity tier.

## Related

[Completion plan](../ALPHA-PLAN.md#5-phase-3-central-control), [ADR-0013](ADR-0013-deliver-batches-in-order-with-bounded-dedup.md), [alpha contract](../ALPHA.md).
