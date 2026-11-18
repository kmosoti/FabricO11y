# Canonical custody

Fabric is the retained authority after a Spindle handoff. A receiver authenticates
one private credential per origin and a separate read credential. Spindle keeps
its original bytes until Fabric confirms a contiguous source prefix. The receipt
binds the authority, persistent origin and source prefix; SDK queue acceptance and
local custody remain separate results.

Build and install the executables with the [collection instructions](collection.md).
The installer also copies `fabric-server`, `fabric-query` and `fabric-mcp`; it
preserves conflicting files and does not activate a receiver or change MCP host
configuration.

Create an owned private state directory and private ASCII credentials of 16–256
bytes. Obtain each Spindle's persistent origin from `spindle status SOCKET`.
Give each origin a different credential. Configure the receiver explicitly:

```json
{
  "listen": "127.0.0.1:19115",
  "storage": "/absolute/private/fabric/control",
  "projection": "/absolute/private/fabric/projection",
  "quota_bytes": 2147483648,
  "projection_quota_bytes": 2147483648,
  "reader_credential_file": "/absolute/private/fabric/reader.key",
  "nodes": [
    {"origin": "SPINDLE_ORIGIN", "credential_file": "/absolute/private/fabric/node.key"}
  ],
  "engine_endpoint": null,
  "max_connections": 16
}
```

Run `fabric-server serve CONFIG.json`. It prints a bounded readiness response and
serves authenticated requests on loopback. For another host, use an authenticated
SSH or VPN tunnel terminating at loopback. A node credential permits ingestion and
original-byte reads for its origin. A reader can query only configured origins;
reader credentials cannot ingest. Neither the query CLI nor MCP accepts arbitrary
SQL, commands or file access.

The SQLite mode stores canonical bytes in a synchronous transactional database.
Its Arrow/Parquet/DataFusion projection is rebuildable and has its own quota and
checkpoint. A failed analytical publication stops queries with an explicit error
while canonical admission and readback continue. Rebuild an incompatible or
corrupt projection from canonical history with the receiver stopped. Preserve the
canonical directory when rebuilding.

An engine-led alternative uses GreptimeDB 1.2.1 and an
explicit loopback `engine_endpoint`, such as `http://127.0.0.1:14600`. Require
synchronous raft-engine WAL writes, disable unused listeners and automatic history
expiry, and enforce the receiver group's memory budget. The engine must be private
and have one Fabric writer. Its first binding requires an empty canonical table;
subsequent starts verify the persisted authority and acknowledged source prefixes.
The HTTP connection is an internal receiver-to-engine link, not the node protocol.

Engine mode reserves IDs, identity and exact bytes in a durable control transaction
before submitting a bounded batch. A timeout retains that reservation. Recovery
retries the same bytes and IDs, verifies exact engine readback, then publishes the
source prefix and removes local pending payload bytes. A late retry cannot reuse
an ID for another payload. Point reads compare retained length and SHA256 digest;
queries reject missing rows in an acknowledged source prefix. The engine owns all
acknowledged raw history; the control database retains coordination metadata.
Back up both with the receiver and engine stopped. A control-only backup cannot
restore reclaimed history. Changing backends requires an explicit migration.

Enable forwarding with `reclaim_confirmed: false` first. Backfill existing custody,
compare original bytes and query results at fixed source prefixes, and verify
recovery before enabling reclamation. Turning reclamation off again retains newly
confirmed local payloads; reads of previously reclaimed payloads still require
canonical access. Keep that access during rollback. Removing forwarding after
reclamation is refused. Canonical history has no automatic expiry.

Use the [typed investigation interface](investigation.md) for source-fenced
answers and supporting evidence. Its metadata states coverage, schema identity,
projection lag, observed gaps, budgets and truncation. Temporal proximity and a
shared run UUID establish correlation, not causation.

The receiver's SQLite ledger uses a persistent rollback journal with EXTRA
synchronization. Quota accounting includes the retained journal and recovery
reserve. Baseline reads and writes share a fair admission queue and one SQLite
connection; Parquet publication runs after releasing it. Native engine reads use a
separate read connection so an engine write does not hold their application lock.
The ledger quota bounds its local files. Provision native engine data and WAL
capacity separately when using `engine_endpoint`.

## Fabric Server self-observability

For Fabric Server to export its own metrics, events, logs, and sampled traces,
configure a dedicated Spindle telemetry store and grant its origin access to the
canonical receiver. Set these environment variables on the Fabric Server process:

```sh
FABRIC_SELF_TELEMETRY_SOCKET=/data/fabrico11y/telemetry/spindle.sock
FABRIC_SELF_TELEMETRY_ORIGINS='telemetry-origin,other-node-telemetry-origin'
```

The socket must be on this host in a private owned directory. The origins list
must contain every telemetry origin routed to this receiver, using their
configured grant identifiers. Workload origins should be kept separate from
telemetry origins. The dedicated telemetry Spindle service has self-export
disabled and can forward through the same authenticated tunnel as another
Spindle origin. Keep server self-export diagnostics out of workload journal
sources to avoid collecting a second copy under a workload origin. For a server forwarding to itself through that service, the
origin guard excludes telemetry commits, their projected rows, and queries
that include a telemetry origin from the server's own measurements. These
operations remain available through the normal custody and query APIs.

The server reports workload commit and query counts and failures, projected
row counts and failures, the maximum committed workload projection lag when
a local projection snapshot is available, and the self-telemetry queue and
drop counts. Each one-minute report includes at most one completed commit and
one completed query trace, with corresponding duration gauges in nanoseconds.
A query has to name an explicit workload-only scope to be measured. Projection fault transitions produce an event and a redacted
log. Native engine mode omits the local Parquet projection lag metric.

A full emitter queue drops new samples before Spindle assigns record IDs.
Optional exporter configuration errors and delivery pressure are reported to
the local service log. The exporter does not retry in the server's request
handling or projection worker, and telemetry failure does not invalidate
canonical custody. The usual canonical retention and quota rules apply to
accepted telemetry records.
