# AGENTS.md

## Repository identity

FabricO11y is a local-first, evidence-aware observability runtime. It admits operational
observations, preserves ordering and provenance, records corrections, and produces
completeness-aware answers.

Do not turn this repository into an execution runtime, policy engine, knowledge graph, hosted
observability suite, or dashboard-first product.

## Architecture boundaries

- `crates/fabric-core/` owns deterministic domain semantics. It must not perform filesystem,
  database, network, process, or PyO3 work.
- `crates/fabric-time/` owns ordering primitives and comparisons between distinct time axes.
- `crates/fabric-schema/` owns typed representations of the normative contracts in `schemas/`.
- `crates/fabric-segment/` and `crates/fabric-catalog/` own storage effects behind typed
  interfaces; they do not redefine domain semantics.
- `crates/fabric-query/` owns typed query planning and completeness-aware answer construction.
- `crates/fabric-ingest/`, `crates/fabric-export/`, and `crates/fabric-service/` are adapters around
  the same engine.
- `crates/fabric-py/` and `python/fabrico11y/` expose coarse operations only. Python must not
  reimplement reducers, correction handling, ordering, or query semantics.
- `schemas/` contains canonical versioned contracts. Contract changes require fixtures,
  compatibility analysis, and explicit versioning.
- Blackcell and PraxisLedger integrations remain optional, protocol-based adapters. Their core
  domain objects must not become FabricO11y dependencies.

## Semantic invariants

- Preserve observations separately from derived conclusions, correlations, and assumptions.
- Preserve `observed_at`, `observed_by_at`, producer-local sequence, logical ordering, and
  `recorded_at` as distinct signals. Do not infer total order from wall-clock time alone.
- Represent corrections, retractions, replacements, qualifications, and deduplication explicitly.
  Never rewrite admitted history to simulate a correction.
- Derived answers must report provenance, source cutoff, coverage, missing sources, assumptions,
  and conflicts when applicable.
- Immutable segment data is archival evidence. Maintained catalog indexes are the primary query
  path; repeated archive decompression is not a search architecture.
- Classification and redaction happen before persistence.
- Deterministic replay and correction propagation take precedence over throughput optimization.

## Change workflow

Read the relevant plan node, upstream contracts, branch, status, and current diff before editing.
Keep one work-package concern per change. Structural and normative work precedes adapters and
optimization.

Prefer typed boundaries, explicit errors, deterministic ordering, isolated effects, and black-box
tests. Do not add a dependency without a concrete boundary benefit. Keep behavior changes separate
from refactors when practical.

Do not edit ignored research or planning inputs under `tmp/`. Promote decisions into tracked
contracts, ADRs, or requirements instead.

## Verification

Run focused checks first, followed by the maintained repository gate once one exists. Until the
first executable packages establish that gate, foundation-only changes must at least run:

```text
cargo metadata --format-version 1 --no-deps
git diff --check
```

Behavior changes require tests. Do not commit, push, tag, publish, deploy, or mutate remote metadata
unless the user explicitly requests it.
