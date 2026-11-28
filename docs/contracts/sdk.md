# Observation SDK contract, revisions 2 and 3

Applications emit versioned typed observations for service requests, agent runs,
tool executions, log events and numeric counter/gauge samples. Every observation
identifies its producer, process generation, monotonically increasing sequence,
wall timestamp and registry descriptor. Identifiers and strings have finite byte
limits. Nonfinite numbers, unknown variants, extra fields, invalid versions and
excessive nesting are rejected. Reading the historical v1 prototype is explicit;
the original `Client` and collector sources write v2. `RunClient` writes v3 with a
required global `RunContext`. Legacy analytics remain available.

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


Revision 3 uses a lowercase nonzero UUID `context.run_id` shared unchanged across
Rust clients and adapter processes. Agent-run and tool-execution `data.run_id`
must equal that global ID. V2 run labels remain local historical labels and do
not imply cross-origin correlation. The v2 descriptor and strict wire shape are
unchanged; v3 has a separate retained descriptor.

An explicit trace producer can call `fabric::observation::encode_v4` and submit
the resulting bytes through the bounded Spindle transport with a valid
producer lease. The `fabrico11y/4` envelope adds `span` to the v3 context and
retains the existing request, agent-run and tool-execution data kinds. The
span has a 32-character lowercase hexadecimal trace ID, a 16-character
lowercase hexadecimal span ID and an optional 16-character parent span ID.
IDs are nonzero and the parent differs from the child. JSON ID strings use
plain hex without escapes. A service request's data trace ID, when present,
equals the envelope trace ID. The ordinary Client and RunClient still emit
v2 and v3 respectively.

`RunClient::new(config, context)` performs bounded lease and progress control
exchanges, each limited by `config.delivery_timeout` (at most five seconds).
`try_record` performs no registration or transport work. An authenticated,
producer-bound generation normally lasts 24 hours and carries its expiry in its
HMAC-protected token. Expiry refuses new records and retries, including attempts
to use a leased generation in a v2 envelope. Previously stored payloads remain
forwardable. Renewal requires a quiescent client with local custody for every
accepted record. The daemon persists a lease clock floor and refuses lease
decisions when wall time moves behind that floor. Admission can return `Expired`.

`RunClient::flush_local(deadline)` confirms local Spindle custody.
`flush_to_fabric(deadline)` additionally waits for the configured canonical
receiver's contiguous prefix, bound to its authority and the Spindle origin.
Renewal preserves the previous local custody target for a later Fabric flush.
The original `Client::flush` retains its local-custody semantics.

The adapter's optional `--run-context JSON` selects v3. Context mode also accepts
`context`, `renew`, and `flush_to_fabric` with `timeout_ms`. A default adapter
continues to accept the original v2 commands. Finite buffers and lease slots can
refuse admission; timeout never implies loss or successful canonical handoff.


Fabric confirmation does not imply local payload deletion. Spindle's forwarding
configuration defaults to shadow collection, retaining payloads until backfill and
shadow comparison are verified and `reclaim_confirmed` is explicitly enabled.
Local and Fabric flush receipts retain their custody meanings in either mode.
