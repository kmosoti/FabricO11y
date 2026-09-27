# Research cost evidence, run 01

[The result record](../../research-costs-run-01.md) interprets the formal run.
[Metadata](raw/metadata.json) records the actual commands, all 490 exits, host/load,
build mode and measured source hashes. [The root exit receipt](run-01.exit.json)
confirms exit 0. [The summary](raw/summary.json) excludes warmups; all warmup results
remain in `raw/`. [Descriptive metrics](metrics.json) add phase CPU/wall, percentile,
family and resource tables without changing registered gates.

The original full output was validated before archival. To keep Git manageable,
[the omission manifest](artifact-retention.json) lists 2,840 regenerable source,
JSON-block and Parquet files (208,131,918 bytes). It retains their hashes and sizes.
The original [SHA-256 manifest](raw/SHA256SUMS.json) includes those omitted files:
it is not a claim that every listed file is present in this archive. All 1,597 other
original files (35,991,827 bytes), including result samples, commands, metadata,
anchors, postings and hints, are preserved. Reports and source hashes identify the
exact measured working tree, whose base Git revision was `079120f`.

Reproduce into a fresh directory from the repository root:

```sh
python3 -B tools/bench/run_research_costs.py target/research-costs/new-run --formal
python3 -B docs/experiments/benchmarks/data/research-costs-run-01/derive_metrics.py target/research-costs/new-run target/research-costs/new-metrics.json
```

The formal run took about 46 minutes here. New timings will differ; this reproduces
the registered procedure, not the original wall-clock readings. Locked Cargo
packages must be available for offline builds. The shorter harness exercise is:

```sh
python3 -B tools/bench/run_research_costs.py target/research-costs/new-smoke --smoke
python3 -B tools/bench/test_research_costs.py target/research-costs/new-smoke
```

Smoke is not formal performance evidence. [Review records](review/) preserve initial
counterexamples and repaired cross-family approvals. The [independent formal auditor](review/audit.py)
uses the complete original output, including data files omitted here. Run it against
a complete new output after confirming that run's actual exit, rather than against
this reduced archive. [Final checks](final-checks.json) record format, layout-test
and documentation exits. Historical review commands may contain the scratch paths where
they actually ran; no live Claude process or external service is needed to read them.
