# FabricO11y Charter

## Thesis

FabricO11y is a Rust-first evidence runtime for operational systems. It ingests observations,
preserves ordering and provenance, applies explicit corrections, and returns derived answers that
state both their support and their limits.

Ordinary telemetry can show what a producer emitted. FabricO11y exists to preserve enough context
to answer harder questions:

- Which records and producers support this conclusion?
- Which events are causally related when clocks disagree?
- What changed after a correction or late arrival?
- Which expected sources are missing or incomplete?
- How should a prior answer be revised without erasing history?

## Target users

FabricO11y is for engineers and operators building autonomous, asynchronous, local-first, edge, or
policy-sensitive systems where replayability and evidence quality matter more than a dashboard
catalog. Initial users are expected to embed the engine in Rust or Python applications or run a
small local service around the same engine.

## Product commitments

FabricO11y will:

1. Preserve observations separately from interpretations.
2. Model corrections as first-class append-only evidence.
3. Retain distinct source, observation, producer-order, logical-order, and admission-time signals.
4. Attach provenance and completeness metadata to every derived answer.
5. Keep core semantics deterministic and independent from storage and transport effects.
6. Use immutable compressed segments for archival payloads and maintained indexes for ordinary
   reads.
7. Provide embedded access first and make any daemon a wrapper around the same engine.
8. Integrate with external systems through versioned envelopes rather than shared domain objects.

## Success criteria

The 0.1 kernel is successful when:

- canonical observation, correction, frontier, and answer contracts validate and round-trip;
- repeated replay of the same evidence produces the same canonical state and answers;
- retractions, replacements, qualifications, and deduplication propagate correctly;
- query answers identify provenance, conflicts, assumptions, source cutoffs, and incomplete
  coverage;
- Rust and Python APIs exercise the same domain implementation;
- corrupt or unreadable storage fails deterministically without silent evidence loss;
- maintained indexes answer hot-path queries without scanning all archived segments.

Performance work is accepted only when it preserves those properties against a full-rebuild oracle.

## Non-goals

FabricO11y does not:

- decide, authorize, schedule, or execute actions;
- infer canonical knowledge, capabilities, or long-lived organizational truth;
- begin as a dashboard-first replacement for general observability platforms;
- assume a hosted SaaS deployment or control plane;
- make vector search, autonomous probes, or knowledge-graph traversal part of the stable 0.1 core;
- treat an imported claim as authoritative merely because it was observed;
- use full archive decompression as its primary search path.

## Release posture

The project starts embedded-first and local-first. Contracts and replay behavior stabilize before
transport breadth or throughput optimization. The optional daemon, interoperability adapters, and
active evidence acquisition remain downstream work with separate compatibility and security gates.
