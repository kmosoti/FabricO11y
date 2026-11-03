# Local observation SDK contract, revision 2

Applications emit versioned typed observations for service requests, agent runs,
tool executions, log events and numeric counter/gauge samples. Every observation
identifies its producer, process generation, monotonically increasing sequence,
wall timestamp and registry descriptor. Identifiers and strings have finite byte
limits. Nonfinite numbers, unknown variants, extra fields, invalid versions and
excessive nesting are rejected. Reading the historical v1 prototype is explicit;
all new collection writes v2. Legacy analytics remain available.

Memory admission and durable custody are distinct outcomes. `try_record` performs
bounded encoding into preallocated storage, never waits for a network or disk,
and explicitly returns acceptance, invalid, oversized, full, contended or closed.
Accepted storage remains charged through delivery and retries until the receiver
confirms custody. A flush with a deadline waits for all observations accepted
before the call, returns their durable receipt frontier, or reports timeout with
unknown delivery outcome. Refusal does not consume a sequence. Retry uses the
same producer/generation/sequence and bytes. Same identity and payload is
idempotent; conflicting payload is rejected. Drop must not wait indefinitely.

A persistent executable exposes the same SDK through bounded JSONL commands and
structured JSONL replies. Diagnostics go to stderr. Rust and Python harness and
generated-tool examples demonstrate admission, flush and failure handling.

Registry validation and generated-artifact drift checks use a pinned maintained
Weaver release. Generated vocabulary plus handwritten types and generated types
are compared against frozen independent valid/invalid wire fixtures. The chosen
representation must preserve bounded decoding and explicit failure semantics.
No OTLP or Prometheus transport is required.

Acceptance includes adversarial decoding, full and contended buffers, receiver
outages, duplicate/conflict retries, reconnect after acknowledgement loss,
deadline expiry, shutdown, adapter framing and end-to-end readback. Existing
admission kernel guarantees and their limited proof scope must remain intact.
