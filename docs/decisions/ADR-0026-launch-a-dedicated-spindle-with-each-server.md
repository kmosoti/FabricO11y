# ADR-0026: Launch a dedicated Spindle with each server

## Status

Accepted on 2026-10-09 at the repository owner's request. The
[product contract](../PRODUCT-CONTRACT.md#linux-installation-contract) records
the capability separately from implementation and verification results.

## Context

The server needs to observe its own operation through the same custody path as
edge telemetry. Requiring an operator to configure a separate collector leaves
that capability absent from ordinary server launches. Writing each delivery or
collection result back into a collected log would also create a feedback loop.

The existing Spindle provides bounded source reads, persistent Strand identity,
Spool custody, authenticated HTTPS delivery and process metering. Reusing that
path preserves its wire format and durable ordering. The service's installed
resource allowance must cover any additional process.

## Decision

The production `fabric-server serve CONFIG_PATH` composition root owns exactly
one dedicated `fabric-node run` child. It provisions the ordinary control-plane
node `fabric-server-self`, publishes a private persistent credential under
`state_dir/self-spindle/token`, and uses `state_dir/self-spindle/spool` for the
child's persistent Strand identity and custody. Restart reuses the credential
and Spool. An existing wrong or revoked identity fails startup rather than being
replaced or reactivated. A process lease excludes duplicate supervisors for
the same server state.

The child starts after the HTTPS listener binds and sends to that listener.
Wildcard listeners map to their corresponding loopback address. TLS verification
stays enabled; the server certificate is the default trust input. Operators can
set `self_spindle_ca` for an issuing CA and `self_spindle_url` for `localhost`
when its certificate SAN requires that name. An override must address the same
listener and port; it cannot redirect the dedicated child to another server.
`self_spindle_executable` overrides the default sibling `fabric-node` binary.
The lower-level [`fabric_server::serve`](../../crates/fabric-server/src/lib.rs)
library primitive does not launch children, allowing explicit embedding and
isolated server tests.

The managed child's `--server-log` startup performs a verified, authenticated
configuration poll before entering its run loop. TLS, authentication or
configuration-handshake errors terminate the child; the supervisor observes that
exit and stops serving. Ordinary edge Spindles retain their existing retry
behavior during disconnection. A printed listening address and child PID mean
the listener bound and the process was spawned, not that telemetry reached
queryable storage. Queried diagnostic bodies and process samples supply that
evidence.

Each production process writes fixed state events and fifteen-second process
samples into a private bounded operational log. The server uses
`state_dir/diagnostics/server.log`; every Spindle uses
`spool_dir/diagnostics/spindle.log`. Each writer retains an active file and one
rotation, at most 256 KiB each. The child pins both diagnostic sources through
the normal log-reader/Spool/ACK path, even when remote configuration replaces
other log paths. The two sources count against the existing sixteen-source cap.
Its managed configuration sets a 64 MiB Spool, a 64 KiB/s delivery cap and a
fifteen-second metric interval. No event is emitted per Batch or ACK. Rotation
and source failures retain their existing gap semantics; diagnostics before
Spool commit are best effort, and final shutdown markers can await a restart.
Administrator pause remains effective: it stops collection, while bounded local
process samples continue at their fixed interval. New diagnostic records await
collection after resume; already committed backlog retains existing delivery
behavior. Pinning prevents source removal, not administrative pause.

The supervisor stops serving if its child exits unexpectedly. Normal shutdown
signals the child first and keeps HTTP available while it exits, allowing up to
twelve seconds before forced reap, then up to ten seconds for HTTP shutdown.
Linux parent-death signaling asks the child to stop after an abrupt parent exit.
The server unit uses `KillMode=mixed`: initial SIGTERM reaches the supervisor;
the existing thirty-second timeout retains a cgroup-wide kill fallback.

The child inherits the server service's cgroup. Existing server and aggregate
memory/task limits remain unchanged and include the child. The standalone
`fabrico11y-node.service` remains for edge collection; it collects its own
diagnostics and does not launch another Spindle. Neither daemon manages cgroups
or adds privilege.

## Alternatives considered

- An operator-managed additional node service would preserve the delivery path,
  but would require extra setup and separate lifecycle ownership for every server.
- A direct server self-ingestion path would avoid a child process, but would
  duplicate source collection and bypass the established Spool custody boundary.
- Implicit process launch inside the library serving primitive would make
  embedding and isolated tests own process effects they did not request.

## Evidence

Implementation boundaries are visible in the
[server supervisor](../../crates/fabric-server/src/main.rs),
[companion provisioning and process ownership](../../crates/fabric-server/src/companion.rs),
[control enrollment](../../crates/fabric-server/src/control.rs),
[Spindle CLI](../../src/bin/fabric-node.rs),
[pinned runtime sources](../../src/spindle/runtime.rs) and
[bounded Linux diagnostic writer](../../crates/fabric-adapter-linux/src/operational_log.rs).
These sources establish the implemented mechanisms, not qualification or
performance results. Verification is registered in the
[self-observation protocol](../experiments/formal/server-self-observation-protocol.md).

## Consequences

Ordinary server launches own persistent local observation and its lifecycle.
Packaging must install both executables, provide a certificate valid for the
local destination, and budget server and child together. Diagnostic files remain
bounded while delivery is disconnected; their rotation can lose uncollected
evidence, so complete shutdown-log delivery is not guaranteed.

## Validation

The registered protocol checks exact queried diagnostic bodies and process
samples through a real companion, source pinning across remote changes,
credential/Spool reuse, duplicate launch refusal, unsafe storage rejection,
child failure propagation, shutdown and abrupt parent exit. Existing delivery,
query and rate oracles retain their meanings. No overhead or deployment
qualification claim follows from this decision.

## Related

- [Linux deployment](../architecture/deployment.md)
- [Spindle collection](../architecture/spindle.md)
- [ADR-0010: Static systemd services](ADR-0010-use-static-systemd-services-for-alpha.md)
- [ADR-0013: Ordered delivery and bounded deduplication](ADR-0013-deliver-batches-in-order-with-bounded-dedup.md)
- [ADR-0014: Server control state](ADR-0014-manage-nodes-through-server-control-state.md)
