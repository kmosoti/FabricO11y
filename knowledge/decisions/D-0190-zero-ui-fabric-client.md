---
id: D-0190
title: Zero-UI Typed Consumer Crate (fabric-client)
type: decision
status: accepted
date: 2026-11-21
added_by: agent
human_ratified: false
related:
  - D-0002
  - D-0010
  - D-0181
---

# D-0190 · Zero-UI Typed Consumer Crate (`fabric-client`)

## Decision

Publish an upstream, zero-UI Rust client crate (`fabric-client`) inside the `FabricO11y` workspace rather than duplicating `fabric.cli.v1` JSON deserializers and subprocess security wrappers across downstream repositories:

1. **Minimal Dependency Footprint:**
   - Depend exclusively on `serde`, `serde_json`, and optional `tokio` (behind `default = ["cli"]`).
   - Prohibit UI/diagram crates, `arrow`, `datafusion`, and direct `redb`/SQLite dependencies in `fabric-client` (enforced via `verification/architecture.toml`).
2. **Subprocess Security & Auth Boundaries:**
   - `FabricClient` invokes `fabric ... --json` concurrently with bounded timeouts (`3s` live probes, `9s` Parquet queries), `kill_on_drop(true)`, and partial-failure preservation in `FabricBundle::warnings`.
   - `FabricQueryClient` enforces `FABRIC_CREDENTIAL_FILE`-only authentication (rejecting missing, symlinked, empty, or oversized files), strips `FABRIC_CREDENTIAL` via `.env_remove("FABRIC_CREDENTIAL")` so `fabric::query::connection_from_env()` never rejects dual-env collisions or leaks raw tokens into `/proc/<pid>/environ`, enables `kill_on_drop(true)`, and redacts token bytes from error messages.
3. **Null-Safe `fabric.cli.v1` Deserialization:**
   - Handle nullable string fields emitted by `fabric snapshot --json` (`AgentSessionRecord.unit`) and `fabric custody --json` (`CustodyStatusReport.spindle_socket`) cleanly while preserving strong schema validation (`EXPECTED_SCHEMA_VERSION = "fabric.cli.v1"`).

## Alternatives Considered

- **Depending on `fabric-core` directly in external clients:** Rejected because `fabric-core` pulls in `arrow` and `chrono`, bloating downstream compile graphs.

## Revisit When

- `fabric.cli.v1` graduates to `fabric.cli.v2` or adds streaming IPC endpoints.

## Relations

- *aligned-with*: [D-0002](D-0002-cli-pivot-and-converged-telemetry-engine.md)
- *aligned-with*: [D-0010](D-0010-canonical-receipt-projection.md)
