---
id: D-0185
date: 2026-11-19
added_by: agent
human_ratified: false
---

# Durable batch reports use the existing Spindle generation

An adapter per terminal report consumes one retained generation per run even
after its connection closes. A confined exercise of the existing v3 wire
protocol admitted 130 different run IDs with one generation. The same
exercise reached the 128-generation metadata limit when requesting more
leases. This separates typed run identity from the transport generation.

The batch adapter owns one private outbox for one producer and persistent
Spindle origin. It records the exact bytes and sequence before submission
and stores the matching receipt before confirming the report to the caller.
A restart can replay the saved bytes and reconcile a lost reply through
Spindle's normal duplicate handling. The outbox stops automatic replay when
the lease expires and preserves unresolved data for explicit recovery.

Early retirement would change Spindle's replay and authorization boundary.
The client outbox keeps the current generation limit and server contract.
The client quota and retained receipt history are independent of accepted
Spindle custody and canonical projection.
