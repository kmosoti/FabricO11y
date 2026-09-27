# Storage/query research probe

This standalone Rust package depends on the current Fabric event types, generator and log. It does not add an application query API or change FOL2. The [research agenda](../../docs/experiments/ablation/observability-storage-research.md) explains its boundary; [S1's preregistration](../../docs/experiments/ablation/storage-query-s1-protocol.md) fixes the corpus and metrics.

`Snapshot` owns replayed events and private optional block summaries. `Query` selects an inclusive time range, optional exact tenant and optional exact case-sensitive whitespace token in Log bodies. Results are input positions, so repeated event IDs remain separate. `Mode::Scan` evaluates every row. `Mode::Pruned` rejects blocks only through actual time bounds or negative Bloom membership, then applies the same exact predicate. Missing summaries scan. `disable_summaries` is a fallback probe, not a disk-cache recovery mechanism.

The library cannot mutate rows or load external indexes. Persisted index corruption, generation mismatch, concurrent append and incomplete raw storage are future publication-contract work. Bloom saturation increases work without changing the results. Logical summary bytes exclude container/allocator overhead; this is not a storage-amplification measurement.

From the repository root:

```sh
cargo test --offline --locked --manifest-path tools/storage-probe/Cargo.toml
python3 -B tools/bench/test_storage_s1.py
python3 -B tools/bench/run_storage_s1.py target/storage-s1/run-02
```

Use a fresh output directory. The runner builds release mode, writes/reopens 12 logs, checks every replayed event and query against its source/reference, and preserves raw CSV, environment, hashes and summary. The logs are reproducible scratch artifacts; preserved [run 01](../../docs/experiments/ablation/storage-query-s1-run-01.md) keeps CSV evidence and hashes. Queries run over in-memory rows after replay: no reported scan reduction measures physical disk savings.

The runner currently requires Linux with `/proc`, `findmnt`, and Python `os.wait4` for host and child-process resource accounting. The Rust query contract itself does not use those interfaces.
