# FabricO11y

FabricO11y is a local-first, evidence-aware observability runtime for autonomous, asynchronous, and
policy-sensitive systems. It is designed to preserve what was observed, how records relate, what
was corrected, which sources support an answer, and what remains incomplete.

> Status: pre-0.1 foundation. There is no supported runtime, SDK, storage format, or public API yet.

## Product boundary

FabricO11y will:

- admit versioned operational observations;
- preserve multiple time axes, producer ordering, causal links, and provenance;
- record corrections and retractions without rewriting history;
- return derived answers with coverage, missing-source, assumption, and conflict metadata;
- expose one Rust semantic core through embedded Rust and Python APIs, with an optional daemon.

FabricO11y will not own execution authority, policy decisions, knowledge inference, a hosted SaaS
control plane, or a dashboard-first observability platform.

Blackcell and PraxisLedger may exchange versioned envelopes with FabricO11y through optional
adapters. Neither system's domain model is part of the FabricO11y core.

## Start here

- [Charter](docs/charter.md): thesis, users, commitments, and success criteria
- [Scope](docs/scope.md): owned behavior, exclusions, and integration boundaries
- [Glossary](docs/glossary.md): canonical evidence and time terminology
- [Repository layout](docs/repository-layout.md): planned component ownership and dependency rules
- [Decision index](docs/decisions/README.md): initial ADR backlog

The repository currently contains an empty Rust workspace so later crates share one dependency and
verification boundary:

```bash
cargo metadata --format-version 1 --no-deps
```

Implementation proceeds in dependency order: normative contracts first, then the pure semantic
core and storage format, followed by replay, SDKs, queries, adapters, evaluation, and release
stabilization.
