# O6 availability verification

Registered before fresh verification, under the owner's accepted completion plan.
[Decision](coupled-availability-decision.md) defines the companion's narrow model;
the original whole-record oracle is unchanged. Checker/fixture routing is its
own trust-boundary commit. Production raw-source correction is a later change.

Serial inner commands (all prefixed with resource_group, run_job and
coupled_admit with4MiB total reserve, zero capacity reserve):

| Job suffix under `catalog-coupled-` | Stage; timeout | Command |
| --- | --- | --- |
| o6-checker-01 | recovery;90s | `python3 -B tools/qualification/test_projection_availability_oracle.py` |
| o6-projection-02 | recovery;180s | `env FABRIC_STORAGE_EVIDENCE=docs/experiments/benchmarks/data/catalog-coupled-availability-01/partial cargo test --offline --locked -p fabric-server --test completion_storage --all-features missing_query_tables_are_incomplete_with_independently_declared_unavailability -- --exact --nocapture` |
| o6-raw-red-01 | recovery;180s | `env FABRIC_STORAGE_EVIDENCE=docs/experiments/benchmarks/data/catalog-coupled-availability-01/raw-red cargo test --offline --locked -p fabric-server --test completion_storage --all-features missing_raw_batches_preserve_projection_rows_but_mark_incomplete -- --exact --nocapture` |
| o6-raw-fixed-01 | recovery;180s | `env FABRIC_STORAGE_EVIDENCE=docs/experiments/benchmarks/data/catalog-coupled-availability-01/raw-fixed cargo test --offline --locked -p fabric-server --test completion_storage --all-features missing_raw_batches_preserve_projection_rows_but_mark_incomplete -- --exact --nocapture` |

Tests resolve paths against the repository root. All transient fixtures use
FABRIC_SCRATCH_ROOT on the data drive. Successful oracle calls retain exact
producer input, query, complete pages, availability declaration and verdict.
Failed fixture remains for byte-verified cleanup. Source archive/hash capture is
per job. The Rust jobs require relevant sources frozen during compilation/run.

Expected diagnostic: raw-red rejects current complete:true with missing raw table.
Do not alter its assertions after observing the outcome. A later production fix
may check raw table size/schema/footer row count using the existing checked open;
full raw page decode/hash remains outside that narrowly stated coverage. Run the
unchanged raw regression, complete storage tests and fast profile after correction.
If independent controls fail, record failure and repair in a fresh named run before
using their verdicts. No historical O6/C2 result or qualification is recertified.
