# Rust Crates

Rust owns FabricO11y's domain semantics. Planned workspace crates are:

| Crate | Responsibility | First work package |
|---|---|---|
| `fabric-time` | Partial-order and multi-axis time primitives | FO02 |
| `fabric-schema` | Typed forms of normative envelopes | FO02 |
| `fabric-core` | Deterministic ordering, provenance, frontier, and correction semantics | FO02 |
| `fabric-segment` | Immutable segment encoding, integrity, sealing, and replay IO | FO03/FO04 |
| `fabric-catalog` | SQLite metadata, frontiers, relation edges, and maintained indexes | FO04 |
| `fabric-query` | Typed query plans and completeness-aware answers | FO07 |
| `fabric-ingest` | JSONL, OTLP, and CloudEvents-compatible admission adapters | FO06 |
| `fabric-export` | JSONL, OTLP, Arrow, and Parquet export adapters | FO10 |
| `fabric-service` | Optional local daemon and transport protocols | FO09 |
| `fabric-py` | Thin PyO3 conversion and exposure | FO05 |
| `fabric-sdk` | Coarse effectful embedded application facade and CLI | FO04/FO05 |

Dependencies point from `fabric-time` and `fabric-schema` through `fabric-core`, then into storage,
the application facade, and language adapters. Core crates are independently testable and contain
no filesystem, SQLite, compression, process, async-runtime, or PyO3 behavior.
