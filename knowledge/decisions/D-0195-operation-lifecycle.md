# Runtime-neutral operation lifecycles

The operation seam is a dependency-free `fabric-ops` crate rather than a
parallel transport or registry. It retains concrete request and response types
and models unary, notification, server-streaming, client-streaming, and
bidirectional calls through explicit transitions. A transition constructs a
fixed-capacity effect batch before committing state. An effect-cap failure
returns no batch and leaves the session unchanged. Message counts are capped;
adapters retain responsibility for payload byte bounds and delivery. The
state transition does not make external effects durable or transactional.

A local adapter establishes peer identity and locality and supplies a
monotonic clock domain. The context accepts a local duration and a separate
wire remaining-time cap. Clock ticks and durations use milliseconds. Both
durations default to 10 seconds and have a hard 30-second maximum, matching
the existing Fabric query budget. It never interprets wire claims as verified identity
or a remote clock reading as a local instant. Adapters must authenticate the
peer and supply consistent local ticks; the crate does not perform either task.

This seam does not define schema negotiation, operation registration, runtime
scheduling, cancellation propagation, or transport behavior. Those boundaries
remain separately owned. Revisit the shared lifecycle if an operation needs
half-close semantics or transactional effect ownership that these explicit
transitions cannot represent.

## Unix peer evidence boundary

Spindle retains a private local-peer witness minted from `SO_PEERCRED` on
the accepted Unix socket after a same-UID check. Request dispatch requires
this witness. Its UID is OS credential evidence in the server's namespace,
not a named application principal or effective authority; a same-UID process,
FD recipient, or proxy can share the connection. No application identity is
forwarded on that basis. The public runtime-neutral `VerifiedPeer` constructor
validates its inputs' shape only and cannot serve as an authorization proof.
Policy and authority revalidation at commit remain separate.

The canonical TCP adapter separately creates a private configured reader,
node-origin or decision-writer grant only after the credential proof and request MAC both pass.
The grant includes the server's canonical authority. It proves credential
possession and configured scope, not a human/application identity or end-client
locality; it does not replace policy and authority checks at commit.
