# Isolated artifact regression controls

Status: prospective preparation; root registers before execution. This supplement
adds deterministic counterexamples for actual lab scripts, not Fabric behavior.
It changes no production source, existing checker, workload protocol or receipt.
[artifact_regression_controls.py](../../../tools/bench/labs/cross_system/artifact_regression_controls.py)
imports the real query sweep, source repair and coordinator implementations.

Three scoped controls run in unique launcher-owned data-drive scratch:

1. Invoke actual `query_sweep.retained_copy` on a valid tiny file, then inject
   same-size altered bytes and a short copy through `shutil.copy2`. The real
   byte/hash comparison must accept the positive and reject both faults.
2. Invoke actual `source_repair.main` with isolated ROOT/CATALOG. Fake a tiny
   complete source response and census; inject a preservation copier that writes
   only a prefix while returning success. Actual digest checking must reject it,
   write a failed receipt, retain the complete original and partial-copy hash,
   and avoid allocating a duplicate `failed-download.tar.gz`. No networking or
   upstream execution occurs. Actual resource limits and disk reserve checks stay
   active; only source/census, isolated paths and the copy fault are mocked.
3. Invoke actual `run_sweep_job.main` with an isolated ledger/readiness lock,
   mocked admission observation and descendant termination. A real tiny child
   runs under the outer resource cgroup. The positive empty child must exit0 and
   clean its empty scratch. The second child exits0 but leaves `owned-raw`; actual
   coordinator logic first rejects leftover scratch, then an injected final
   footprint exception must still produce statefailed/exit1, child_exit0, the
   exact census error and retained raw bytes. Mock termination never kills any
   unrelated root process.

The third control exercises receipt finalization of an existing leftover-scratch
failure. It does not assert that a final-census-only fault preserves an already
removed empty directory: current coordinator cleanup precedes its final census.
Record this boundary explicitly. Neither the fake source census nor the mocked
admission is evidence about upstream correctness or resource admission strength.
The outer helper and nested actual entry points still require enforced cgroup
limits. The tiny child executes only fixed standard-Python fixture code.

Allow60 seconds overall,50 seconds inside the helper,3MiB complete raw fixture
and4MiB retained evidence. Snapshot source hashes and retain the full generated
tree with exact archive readback before cleanup. Expected failure receipts are
preserved as controlled counterexamples; remove their generated originals only
after assertions and archival succeed. On unexpected failure, keep the complete
owned fixture and emit a new failed helper receipt. Original failed campaign
evidence remains unchanged.

Proposed serialized command after registration:

```sh
python3 tools/resource_group.py -- python3 tools/bench/labs/cross_system/run_sweep_job.py --id artifact-regression-01 --lab coordinator --seconds 60 --reserve-mib 4 -- python3 -B tools/bench/labs/cross_system/artifact_regression_controls.py --out docs/experiments/benchmarks/data/cross-system-sweep-01/coordinator/artifact-regression-controls-01 --proposal docs/experiments/benchmarks/cross-system-artifact-regression-proposal.md
```

The helper is prepared only. No controls, source downloads, builds, workload
executions or cleanup have been run by its author.
