# Local evidence prototype

This document specifies a single-host prototype, not a production service or an
OpenTelemetry implementation. Its fixtures model a service request and an agent
run. Observations carry a version, identifier, producer, timestamp, kind, text,
numeric value, optional vector and related identifiers. Text is supplied explicitly;
the prototype does not capture prompts or secrets automatically.

An SDK admission result reports whether an observation entered a bounded in-memory
queue. Admission is not durability. Invalid, oversized and full observations are
rejected and counted. Collection validates again before storing an observation.
The [concurrent admission API](admission.md) reserves fixed buffers before
encoding and keeps their capacity charged until consumer leases are dropped.
Contention, closure, poison and sequence exhaustion are distinct outcomes.
The store enforces a byte limit, and a successful flush means its file has been
synchronized locally; it does not promise replication or survive all hardware
failures. A query sees only records admitted by the store. A failed write or flush
must not be reported as durable.

The prototype offers bounded, local operations for structured filtering, time
series, lexical matching, vector similarity, relationship traversal, comparisons,
anomaly detection and a simple forecast. Results identify contributing observation
IDs. Forecasts are estimates, not observed facts. No semantic similarity, graph
relationship or temporal correlation establishes causation.

OpenTelemetry semantic conventions describe shared names and meanings, and
Weaver generates code and validates registries. They do not by themselves enforce
the prototype's runtime byte and custody limits. This prototype uses a small
handwritten model while its requirements are exercised; it does not claim OTel
compatibility or introduce a competing schema generator. Revisit Weaver before
adding a maintained schema registry.

References: [OpenTelemetry semantic conventions](https://opentelemetry.io/docs/specs/semconv/),
[Weaver code generation](https://opentelemetry.io/docs/specs/semconv/non-normative/code-generation/),
and [attribute limits](https://opentelemetry.io/docs/specs/otel/common/#attribute-limits).
