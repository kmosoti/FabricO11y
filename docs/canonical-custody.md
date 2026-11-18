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
SSH tunnel, optionally over a VPN. A node credential permits ingestion and
original-byte reads for its origin. A reader can query only configured origins;
reader credentials cannot ingest. Neither the query CLI nor MCP accepts arbitrary
SQL, commands or file access.

## Connect multiple hosts

1. Start local-only Spindle on each host and record the origin returned by
   `spindle status "$XDG_RUNTIME_DIR/spindle/spindle.sock"`. Preserve its custody
   directory across deployments. Use a fresh store to enroll a distinct source.
2. Configure the receiver with each origin and a different credential file per
   origin. Give each host only the credential for its own origin. Store the
   matching credential in a private file on the host. Provision canonical
   and projection storage separately. The receiver accepts up to 32 configured
   origins and up to 32 concurrent connections.
3. Start the receiver using its private state and use `fabric-query progress`
   with the reader credential to obtain the persistent authority. Set the
   expected authority in each Spindle forwarding configuration before starting
   shadow forwarding. The coupled `install-stack` command configures one
   co-located origin. Use the explicit receiver and node configurations for
   multiple origins.
4. On each remote host, run an authenticated loopback tunnel, such as the
   following foreground SSH session. Set Spindle's forwarding endpoint to
   `127.0.0.1:48200`. The receiver in this example listens on
   `127.0.0.1:19115`.

```sh
ssh -NT -o ExitOnForwardFailure=yes \
  -L 127.0.0.1:48200:127.0.0.1:19115 operator@receiver.example
```

From the receiver host, or from an operator host with its own tunnel, use the
reader credential to inspect canonical progress. Substitute the local endpoint
and your owned credential path.

```sh
FABRIC_ENDPOINT=127.0.0.1:19115 \
  FABRIC_CREDENTIAL_FILE=/absolute/private/fabric/reader.key \
  fabric-query progress
```

Compare each host's `highest_record_id` and `canonical_through` with the matching
origin's receiver frontier. Then submit a [typed query](investigation.md) scoped
to the intended origins. Compare `coverage.snapshot`,
`coverage.canonical_frontier`, and `coverage.projection.frontiers` for each origin.
A complete result covers its requested source snapshot. It can still omit
records accepted only at a disconnected host. Check `coverage.observed_gaps`
and `coverage.truncated` alongside those frontiers.

The source identity is `(authority, origin, source ID)` across deployments.
A source ID by itself can repeat across hosts. Each collector's local projection
and the receiver's canonical projection may advance independently. A canonical
acknowledgment establishes custody. Query evidence comes from the projection
serving the requested snapshot.

## Resource pressure

Fabric Server bounds connections by the configured cap and the
[startup descriptor budget](collection.md#elastic-collector-memory).
It samples descriptors and cgroup v2 working-set bytes about every 100 ms.
Descriptor pressure starts at the spendable count or a resource error during
sampling. Memory pressure starts at the spendable limit for the working set
of the service cgroup or an ancestor. Missing cgroup limits or usage provide
no signal. The configured ceilings and request deadlines still apply.

During pressure, the server defers new analytical plans, queries, exact
projection-readiness checks, and background Parquet replay batches. It returns
a retryable error for plans and queries and reports projection evidence as
unavailable on a deferred readiness check. Work already admitted keeps its
lease through execution and response cleanup or completion of the projection
batch. Cancellation remains available to authorized readers and leaves the
query slot occupied until that cleanup. The server continues to accept
canonical custody and authenticated readback within the effective connection
cap. Once pressure clears, the Parquet projector resumes at its committed
checkpoint. Greptime-backed custody, including pending writes and verification,
continues under the existing backend contract.

## Storage, verification, and recovery

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

## Check a receipt at the canonical receiver

A Spindle receipt records local custody. Save the origin at submission and use
its exact receipt to check the committed analytical projection at the receiver.

```sh
fabric projection-ready --target canonical --authority "$FABRIC_AUTHORITY" \
  --origin "$SPINDLE_ORIGIN" --receipt receipt.json --wait 30s --json
```

Set the receiver address and private reader credential file with
`--server-endpoint` and `--reader-credential-file`, or use the existing custody
configuration defaults. Across hosts, use the local endpoint of an authenticated
tunnel. JSON distinguishes the per-origin custody frontier from the projection
frontier. Exit 0 requires a matching committed Parquet row. Exit 3 indicates
canonical custody with a projection still behind. Exit 4 indicates unavailable
evidence, including an unknown authority or origin, a receipt the receiver
cannot resolve, or a native engine backend without a separate Parquet witness.
