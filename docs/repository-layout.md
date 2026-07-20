# Repository Layout

FabricO11y uses a Rust workspace with isolated effect boundaries and a thin future Python surface.
Directories are added when their owning work package begins; an absent planned directory is not an
alternate implementation location.

```text
FabricO11y/
├── crates/
│   ├── fabric-core/       deterministic evidence semantics
│   ├── fabric-time/       partial-order and time primitives
│   ├── fabric-schema/     typed normative envelopes
│   ├── fabric-segment/    immutable segment encoding and replay IO
│   ├── fabric-catalog/    SQLite catalog and maintained indexes
│   ├── fabric-query/      typed queries and answer envelopes
│   ├── fabric-ingest/     intake adapters
│   ├── fabric-export/     export adapters
│   ├── fabric-service/    optional daemon transport
│   └── fabric-py/         PyO3 conversion and exposure
├── python/
│   └── fabrico11y/        typed Python API with no domain reducers
├── schemas/               canonical versioned JSON Schemas
├── fixtures/              generated and sealed conformance evidence
├── benchmarks/            correctness and performance harnesses
├── docs/
│   ├── decisions/         architecture decision records
│   ├── requirements/      functional and quality requirements
│   ├── research/          source reviews and rejected options
│   └── evaluation/        methods, fixtures, and results
└── Cargo.toml             workspace boundary
```

## Ownership rules

- Domain meaning belongs in `fabric-core`, with ordering primitives in `fabric-time` and contract
  representations in `fabric-schema`.
- Core crates do not perform filesystem, database, network, process, or Python effects.
- Storage crates implement typed ports defined by the semantic layer; they do not reinterpret
  evidence.
- Query code consumes the same correction and frontier semantics used during replay.
- Ingestion, export, service, and Python crates are replaceable adapters.
- JSON Schemas are public contracts. Rust types, Python types, fixtures, and documentation must
  remain aligned with them.
- Golden fixtures are immutable once referenced by a compatibility test. Corrections require a new
  fixture version rather than overwriting prior evidence.

## Dependency posture

Dependencies point from adapters toward stable inner contracts. The Rust core must never depend on
PyO3, SQLite, Zstandard, an async runtime, an HTTP framework, Blackcell, or PraxisLedger.

Third-party dependencies are introduced only with the work package that needs them and only after
their boundary benefit is documented. Workspace-wide version policy, minimum Rust version, license,
and release profile remain open decisions until their owning ADRs are written.

## Generated and local state

Build output, virtual environments, benchmark output, generated exports, and research inputs are
not source contracts. Generated files must identify their source of truth and regeneration command.
Ignored planning inputs under `tmp/` are read-only inputs; accepted decisions move into tracked
documentation or schemas.
