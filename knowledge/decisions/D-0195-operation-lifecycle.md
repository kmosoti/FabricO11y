---
id: D-0195
date: 2026-11-27
added_by: agent
human_ratified: false
---

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
wire remaining-time cap. It never interprets wire claims as verified identity
or a remote clock reading as a local instant. Adapters must authenticate the
peer and supply consistent local ticks; the crate does not perform either task.

This seam does not define schema negotiation, operation registration, runtime
scheduling, cancellation propagation, or transport behavior. Those boundaries
remain separately owned. Revisit the shared lifecycle if an operation needs
half-close semantics or transactional effect ownership that these explicit
transitions cannot represent.
