# Repository Scope

## Owned behavior

FabricO11y owns:

- admission of versioned operational evidence;
- validation and normalization at the domain boundary;
- explicit observation, correction, frontier, and answer envelopes;
- partial-order and producer-local ordering semantics;
- deterministic correction application and replay;
- immutable segment and dictionary metadata;
- catalog-backed lookup, maintained projections, and completeness-aware queries;
- embedded Rust and Python access to the same engine;
- optional ingestion, export, and local-service adapters.

The repository may contain test fixtures, benchmarks, schemas, migration logic, and operational
documentation needed to prove those behaviors.

## Excluded authority

FabricO11y does not own:

- execution authority, tool authorization, workflow scheduling, or policy enforcement;
- knowledge inference, capability inference, or canonical semantic truth;
- application business state;
- a hosted SaaS control plane, tenant platform, or billing system;
- a dashboard-first general observability experience;
- external producers' retention, sampling, or delivery guarantees;
- the correctness of an imported interpretation.

An observation can be authoritative evidence that a producer emitted a claim without making that
claim authoritative as a conclusion.

## Component boundary

The intended dependency direction is:

```text
versioned envelope
    -> validation and typed domain model
    -> deterministic semantic operations
    -> effectful storage or query adapters
    -> embedded SDK or optional transport
```

Storage, transport, Python, and external adapters call coarse semantic operations. They must not
reimplement ordering, provenance, correction, frontier, or answer logic.

## Integration boundary

Blackcell and PraxisLedger are optional protocol peers:

- A Blackcell adapter may convert runtime events into `fabric.observation/v1` records while
  preserving the original schema and authority metadata.
- A PraxisLedger adapter may consume a `fabric.answer/v1` result as non-canonical evidence with its
  completeness and provenance intact.
- FabricO11y may also accept or emit OTLP, CloudEvents-compatible, JSONL, Arrow, or Parquet
  representations through adapters.

No integration may import another repository's core domain objects into the FabricO11y semantic
core. Optional adapters depend inward on public FabricO11y contracts; the core never depends
outward on an integration.

## Deployment boundary

The embedded SDK is the primary product surface. A local daemon may add isolation and
multi-producer transport, but it wraps the embedded engine and must preserve SDK behavior. Hosted
operation is possible for downstream adopters but is not a repository assumption or an initial
product commitment.

## Data boundary

Admission must classify and redact data before durable persistence. Payload bodies and searchable
metadata may have different retention and authorization policies. Corrections and deletions leave
auditable records; they must not silently rewrite historical evidence.

## Deferred work

The following remain outside the foundation work package:

- normative JSON Schemas and golden examples;
- Rust semantic and time crates;
- segment, dictionary, and SQLite catalog implementation;
- Python bindings and transport adapters;
- query algebra, maintained projections, and full-text search;
- benchmark automation, CI, release packaging, and compatibility guarantees;
- active evidence acquisition.
