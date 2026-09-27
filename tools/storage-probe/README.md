# Storage/query research probe

This standalone Rust package depends on the current Fabric event types, generator and log. It does not add an application query API or change FOL2. The [research agenda](../../docs/experiments/ablation/observability-storage-research.md) explains its boundary; [S1's preregistration](../../docs/experiments/ablation/storage-query-s1-protocol.md) fixes the corpus and metrics.

`Snapshot` owns replayed events and private optional block summaries. `Query` selects an inclusive time range, optional exact tenant and optional exact case-sensitive whitespace token in Log bodies. Results are input positions, so repeated event IDs remain separate. `Mode::Scan` evaluates every row. `Mode::Pruned` rejects blocks only through actual time bounds or negative Bloom membership, then applies the same exact predicate. Missing summaries scan. `disable_summaries` is a fallback probe, not a disk-cache recovery mechanism.

The S1 snapshot cannot mutate rows or load external indexes. Bloom saturation increases work without changing results. Logical summary bytes exclude container/allocator overhead; this is not a storage-amplification measurement. The later disk and retry modules have separate contracts below.

From the repository root:

```sh
cargo test --offline --locked --manifest-path tools/storage-probe/Cargo.toml
python3 -B tools/bench/test_storage_s1.py
python3 -B tools/bench/run_storage_s1.py target/storage-s1/run-02
```

Use a fresh output directory. The runner builds release mode, writes/reopens 12 logs, checks every replayed event and query against its source/reference, and preserves raw CSV, environment, hashes and summary. The logs are reproducible scratch artifacts; preserved [run 01](../../docs/experiments/ablation/storage-query-s1-run-01.md) keeps CSV evidence and hashes. Queries run over in-memory rows after replay: no reported scan reduction measures physical disk savings.

The runner currently requires Linux with `/proc`, `findmnt`, and Python `os.wait4` for host and child-process resource accounting. The Rust query contract itself does not use those interfaces.

## E1R coverage prototype

The [coverage result](../../docs/experiments/ablation/coverage-e1-run-01.md) preserves the executed checks. The [registered coverage experiment](../../docs/experiments/ablation/coverage-e1-protocol.md) has a
separate `coverage` module and [fixed API](COVERAGE_API.md). It validates exact-set summaries,
authenticates complete block metadata with SHA-256, and verifies query-bound receipts against
an independently retained anchor. `Complete`, `Incomplete` and an error are distinct outcomes.
Availability is modeled in memory; it is not detected disk loss. The verifier has no row access,
and a scanned marker relies on a trusted exact evaluator. It cannot expose a faulty trusted
builder merely by checking hashes. The tests retain that negative control.

Run the contract suite with the package test command above. Preserve the fixed corpus output:

```sh
python3 -B tools/bench/run_coverage_e1.py target/coverage-e1/run-02
```

Use a fresh directory. The runner records command exits, source hashes, tracked diff and
toolchain. This is a finite correctness experiment, with no latency or memory winner.
Its exact summary sets differ from S1's Bloom filters. FOL2 and application dependencies
are unchanged; `sha2` belongs only to this research package. See the [query view](../../docs/architecture/query.md)
and [experimental decision](../../docs/decisions/ADR-0007-experiment-with-coverage-receipts.md).

## Local lifecycle prototype

The separate `fabric-research` binary composes [strict offline OTLP/JSON Logs adaptation](COLLECT_API.md), bounded `EventBuffer` ingestion into unchanged FOL2, [S2 immutable disk publication](DISK_API.md), and [E3 root-bound query resume](RESUME_API.md). It accepts caller-owned version-1 Event JSON as well as generated sample input. The OTLP adapter supports one resource group and a deliberately restricted scalar Log profile; it is not a network receiver. `ingest` checks an existing log against the exact input prefix before appending and retains rejected buffer values for retry. A storage I/O error requires rebuilding from an independent source on healthy storage.

`publish` writes immutable JSON blocks and authenticated metadata, then the caller saves a `Publication` outside the snapshot directory. Query and resume need that independently retained publication; resume also needs the previous checkpoint's independently retained SHA-256 digest. A missing or corrupt candidate block leaves the answer incomplete, including when an availability mask says it should be present. A valid local `cold/` copy can serve a candidate. Authenticated metadata may safely exclude a block without reading its raw rows. Residual work and checkpoints remain bound to one snapshot root and query; late arrivals require a successor snapshot. `rebuild` reconstructs a missing manifest only if available raw copies reproduce the same trusted root. These are local process and filesystem contracts, not a physical power-loss result or a chosen storage format. The [S2 result](../../docs/experiments/ablation/durable-snapshot-s2-run-01.md) records the completed disk correctness checks; the [full E3 corpus](../../docs/experiments/ablation/resume-e3-run-01.md) and [cost comparisons](../../docs/experiments/benchmarks/research-costs-run-01.md) have separate completed records.

From the repository root, use an existing durable parent and fresh output names:

```sh
mkdir -p target
cargo run --release --offline --locked --manifest-path tools/storage-probe/Cargo.toml --bin fabric-research -- generate 42 3 target/research-input.json
cargo run --release --offline --locked --manifest-path tools/storage-probe/Cargo.toml --bin fabric-research -- ingest target/research-input.json target/research-events.fol 2 2
cargo run --release --offline --locked --manifest-path tools/storage-probe/Cargo.toml --bin fabric-research -- publish target/research-events.fol target/research-snapshot target/research-publication.json 2 1
python3 -B tools/bench/run_prototype.py target/prototype-demo
```

The runner creates its own fresh directory and invokes separate processes for adaptation, ingest, publish, query, resume, verify and rebuild. Its single lifecycle and timing observation are an illustration, not a performance comparison. For quick tests, skip only the registered long E3 product; run that full corpus separately, which can take over an hour:

```sh
cargo test --offline --locked --manifest-path tools/storage-probe/Cargo.toml -- --skip registered_e3_full_mask_query_product
cargo test --release --offline --locked --manifest-path tools/storage-probe/Cargo.toml --test resume_e3 registered_e3_full_mask_query_product
```

The [prototype architecture](../../docs/architecture/research-prototype.md) explains the boundaries and trust inputs.

The commands above use the local Cargo cache. On a fresh machine, fetch the locked
dependencies once before using offline mode:

```sh
cargo fetch --locked --manifest-path tools/storage-probe/Cargo.toml
```
