# FabricO11y

FabricO11y is a local-first evidence runtime for operational systems. It admits versioned
observations, preserves partial ordering and provenance, records append-only corrections, and
replays the same deterministic state through Rust and Python.

> Status: pre-0.1 MVP kernel (FO00–FO05). The normative contracts, pure Rust core, immutable
> segment format, SQLite catalog, recovery path, CLI, and embedded Rust/Python SDK are implemented.
> Ingestion adapters, completeness-aware query algebra, daemon transport, and release hardening
> remain downstream work.

## Try it

No credentials or external service are needed. From the repository root:

```bash
data_root="$(mktemp -d)"
cargo run -p fabric-sdk --bin fabricctl -- "$data_root" admit fixtures/golden/contracts/valid/observation.json
cargo run -p fabric-sdk --bin fabricctl -- "$data_root" admit fixtures/golden/contracts/valid/correction.json
cargo run -p fabric-sdk --bin fabricctl -- "$data_root" seal
cargo run -p fabric-sdk --bin fabricctl -- "$data_root" validate
```

The final command returns a report shaped like:

```json
{
  "archived_record_count": 2,
  "correction_count": 1,
  "frontier_count": 0,
  "observation_count": 1,
  "pending_record_count": 0,
  "segment_count": 1
}
```

Use `fabricctl "$data_root" replay` for canonical semantic state or `locate obs-0001` for the
catalog-backed record location. Admission assigns `recorded_at`; it does not trust the caller's
value.

## Embedded APIs

Rust callers use `fabric_sdk::Engine` for coarse `admit`, `seal`, `replay`, `validate`, and
`locate` operations. Python exposes the same engine through `fabrico11y.Engine`; its package only
converts JSON and typed DTOs. Build a real wheel, install it into a disposable environment, and
compare Python replay with the Rust CLI using:

```bash
python scripts/verify_python_sdk.py
```

The engine has a single-owner write contract. Multiple processes must externally serialize access
to one data root. Every record must already have a classification; applications with sensitive
payloads supply an `AdmissionPolicy` that classifies and redacts before durable staging. Full
security policy enforcement remains deferred.

## Architecture boundary

FabricO11y owns evidence admission, explicit observation/correction/frontier/answer contracts,
partial ordering, deterministic correction replay, immutable archives, and maintained metadata.
It does not own action execution, policy decisions, knowledge inference, a hosted control plane, or
a dashboard-first observability platform.

Start with the [charter](docs/charter.md), [scope](docs/scope.md),
[evidence semantics](docs/standard/evidence-semantics.md),
[segment format](docs/architecture/segment-format.md), and
[repository layout](docs/repository-layout.md). The schemas in `schemas/` are normative.

## Verification

The maintained project gate runs schema and sealed-fixture validation, formatting, strict Clippy,
all Rust tests and examples, compression correctness, a clean wheel install, and Rust/Python
parity:

```bash
python scripts/verify.py
```
