# Benchmarks

Benchmarks measure both correctness and cost. A faster result is not valid if replayed state,
correction propagation, frontier coverage, or answer provenance differs from the canonical oracle.

The future harness will report:

- replay determinism and throughput;
- correction propagation latency versus full rebuild;
- compression ratio, CPU cost, and dictionary hit rate by payload family;
- p50, p95, and p99 latency for indexed query classes;
- frontier false-certainty rate;
- recovery behavior under interrupted writes and corrupt inputs.

Benchmark results must record fixture digests, Rust and SQLite versions, Zstandard version and
level, dictionary identity, hardware, CPU policy, and cache state.
