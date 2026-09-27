# Hybrid layout research probe

This separate Rust package implements the [S3/S4 contract](API.md) with pinned Arrow/Parquet 60.0.0. It uses the storage probe's lossless Event codec and full-row digest. The root application's FOL2 log and the S2 JSON snapshot remain unchanged.

One Parquet table contains predicate columns (`event_time`, `tenant`, payload kind and Log body), a complete raw Event column, and a row digest. This hybrid layout duplicates data so a projected query can avoid decoding raw Events after identifying candidates. `query_full` decodes rows; `query_projected` reads predicate and digest columns; an optional exact-token postings file narrows registered-token candidates. Invalid or absent postings fall back to projection. Every query authenticates the **whole Parquet file** against an independently retained table anchor before reading, so projection still makes a full byte pass. A trusted builder and correctly retained anchors are assumptions; this package adds no durable publication API.

The code compares exact positional results, including duplicate IDs, Unicode-whitespace Log tokens, and floating-point bits. The [registered comparison](../../docs/experiments/benchmarks/columnar-selective-s3-s4-protocol.md) has [measured results](../../docs/experiments/benchmarks/research-costs-run-01.md): compressed layouts pass the synthetic-workload gates, without selecting a production format. The raw Event column and whole-file authentication must be included in any size or read-cost interpretation.

From the repository root:

```sh
cargo test --offline --locked --manifest-path tools/layout-probe/Cargo.toml
```

See the [prototype architecture](../../docs/architecture/research-prototype.md) and [query boundary](../../docs/architecture/query.md).

The commands above use the local Cargo cache. On a fresh machine, fetch the locked
dependencies once before using offline mode:

```sh
cargo fetch --locked --manifest-path tools/layout-probe/Cargo.toml
```
