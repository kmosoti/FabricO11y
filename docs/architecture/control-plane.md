# Central control

Status: implemented and tested end to end; the [fleet protocol](../experiments/benchmarks/alpha-phase3-fleet-protocol.md) at 10, 100 and 1,000 identities passed on an earlier revision ([run 01](../experiments/benchmarks/alpha-phase3-fleet-run-01.md)). What a request means (revocation is terminal, revisions advance by one and never wrap, name and shape limits) is decided by [`fabric_core::control`](../../crates/fabric-core/src/control.rs); the server persists it. "Node" in routes, JSON and state files is the persisted name of a Spindle enrollment. [ADR-0014](../decisions/ADR-0014-manage-nodes-through-server-control-state.md) records the design.

## Responsibilities

The expanded release replaces shared administrator authority with
[local passkeys and scoped principals](identity-access.md), as decided in
[ADR-0027](../decisions/ADR-0027-ship-a-scoped-passkey-pwa-console.md).
Normal serving implements this boundary through `/v1/console`; explicit legacy
serving retains the older admin routes for migration. Browser and scoped-access
acceptance remain separate from historical control tests.

The [Fabric Server](../../crates/fabric-server/src/control.rs) owns the node inventory: each node's name, the SHA-256 of its bearer token, its status (active, paused or revoked) and its desired configuration with a revision number. That state lives in `state_dir/control.json`, replaced by synced rename on every administrative change. The server keeps each node's last poll time, applied revision and configuration error in memory only.

Each [Spindle](spindle.md) owns what it actually runs: its local file (spool, spool ceiling, server target) plus the last configuration it validated and applied, stored in `applied-config.json` in its spool.

## Flows

- **Enroll:** `fabricctl access <ACCESS_CONFIG> node add <NAME> [--log PATH]... [--interval S]` calls `POST /v1/console/nodes` with a scoped workload token. The answer contains the node token once; the server stores only its hash.
- **Configure:** `node config` calls `PUT /v1/console/nodes/<name>/config`, which checks scope and shape and bumps the revision.
- **Poll:** every 5 s, `fabric-node run` calls `GET /v1/config` with its node token, its applied revision and any validation error. An unchanged revision gets 304. A new view is validated with the same rules as a local file, stored, then activated; after activating, the node polls again at once so the server sees the new applied revision. An invalid view is reported and not activated.
- **Pause and resume:** a paused node stops collecting but keeps delivering what it already committed. Pausing writes the coverage-unknown marker, so the first batch after resume carries one gap for the paused interval.
- **Revoke:** the token hash is removed from the active authentication map; the revoked enrollment remains persisted. Batch intake and polling answer 401; the node keeps its spool and its last configuration.
- **Inventory:** `node list` shows status, desired and applied revisions, last poll time and any configuration error.

## Boundaries and invariants

- Node tokens never authorize console routes; human/workload credentials never authorize batch intake. Production serving rejects legacy master-admin routes.
- Remote configuration can change only log paths and the metric interval. The spool ceiling and the server target stay local.
- A configuration is active on a node only after it passed local validation and was stored durably.
- While the server is unreachable, a node, including one that restarts, runs its last applied configuration.

The complete serialized inventory must fit the same 16 MiB limit enforced on
reopen, including JSON escaping. Oversized updates are rejected before publication.
If rename succeeds but the directory sync fails, the live control instance refuses
authentication, configuration polls and mutations until reopened; restoring an
in-memory record cannot safely restore authority after uncertain publication.

## Limits

Applied revisions and errors are observations: after a server restart they read as unknown until each node polls. Removing a log path and adding it back reads the file again from the start if the node restarted in between. There is no push path; the worst-case apply time is one poll interval plus a round trip, when the node is healthy.

Authentication precedes queueing: requests already admitted before revocation
can still commit. Immediate cancellation of in-flight custody is not established.
