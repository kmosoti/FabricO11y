# Prospective storage heap scaling prediction

Supplement to the [storage sweep](cross-system-storage-sweep-proposal.md),
registered after its two 16 MiB jobs and **before** either 64 MiB job. This adds
a research-model falsification check, without changing any old workload,
semantic checker or acceptance outcome. The 64 MiB outcomes have not been read
because those jobs have not run. The frozen storage source/protocol stays intact.

For steady and adversarial separately, freeze model A as the mean peak across
the two 16 MiB seeds at each cap, independently of the other caps. Model B fits
ordinary least squares
`peak_live_heap_bytes = base + gamma * run_cap_bytes` using the three caps and
both completed 16 MiB seeds: six observations per shape. Predict the identical
peak for each cap when payload target grows to 64 MiB, for both existing seeds.
Record units, coefficients, raw training values, training residuals and receipt
hashes. Do not fit CPU, wall, throughput or IO from these noisy small pairs.

H1: a stable stage working set plus run-cap storage dominates the maximum, so
the model generalizes across input size. H0: another input-sized component,
table transition, merge level, writer/codec buffer or allocator-capacity boundary
changes the dominant peak. The linear model assumes the same live objects and
allocation regime overlap at the maximum, average resident row estimates remain
stable, and the row-count cap does not replace the byte cap differently. It is
not a worst-case bound or proof of fixed service memory.

Source gives concrete counterexamples: metrics have their own resident run,
and their increasing row count can approach the independent 32768-row limit;
more groups can add manifest/writer state, and spill counts can enter another
merge level. Group or table cardinality therefore can invalidate a
constant-base assumption even while logs stay byte capped. The 16 MiB bigrows
results already show identical cap16/cap32 allocator peaks within each seed.
Exclude bigrows from this linear fit and retain that plateau as an explicit
different-stage hypothesis. Exclusion is declared before holdout measurement;
the original three-shape semantic screen still applies to all cells.

These are deliberately strong challenge models. At 16 MiB, the metrics table
has about 14304 rows, below its independent 32768-row cap; the 64 MiB fixture
has roughly 57000 rows and can bind it. Initial log spill count may also cross
the sixteen-way fan-in boundary and add a merge pass. Three log run-cap knobs
cannot identify concurrent per-table occupancy or a unique memory owner.
Bounded memory need not already be flat at the smaller input. Falsifying these
models would be useful evidence about saturation or stage overlap, without
establishing a production memory defect by itself.

Decision, applied independently to A and B: all twelve held-out steady/adversarial cells must retain their existing
semantic gates, and `abs(observed - predicted) / observed` must be at most 5% in
**every** cell. Report maximum error, per-cell signed residuals and error by
shape/cap/seed. A failed/interrupted holdout is inconclusive for this check,
not a model success. No refit, seed exclusion or threshold widening after seeing
64 MiB results. A later model belongs to a separate prospective experiment.
Five percent is a falsifiable screening tolerance: the observed 16 MiB heap
values closely repeat and approximately follow a line, while a tolerance of
several percent permits small allocator fluctuations. It does not supply a
statistical coverage interval or a guaranteed system bound.

Run the [predictor](../../../tools/bench/labs/cross_system/storage_scaling_prediction.py)
under the existing resource group/coordinator before admission of either 64 MiB
job; it refuses if either holdout evidence directory exists. Save its immutable
JSON before starting holdout. The tiny command counts against the round budget:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/cross_system/run_sweep_job.py --id storage-prediction --lab memory --seconds 60 --reserve-mib 1 -- python3 -B tools/bench/labs/cross_system/storage_scaling_prediction.py --source docs/experiments/benchmarks/data/cross-system-sweep-01/memory --destination docs/experiments/benchmarks/data/cross-system-sweep-01/memory/scaling-predictions.json --protocol docs/experiments/benchmarks/cross-system-storage-scaling-prediction-proposal.md
```

Model acceptance supports only these finite offline working-set predictions.
It does not advance M1, establish reservations, nominate a production run size
or replace the separately registered fresh confirmation for a selected vector.
