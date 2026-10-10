# Retained bigrows filter-layout validation

Registered before execution. Original M2 remains failed; scaling remains blocked.
H1: the mismatch is physical index cardinality caused by permitted row-group
splitting, with correct per-group filters. H0: retained manifests differ from
original pair, a file fails authentication, group/filter cardinalities disagree,
a data row count differs, or any actual body trigram is excluded by its own
filter. Neither outcome rewrites M2 or modifies its checker.

Read-only input is the M2 archived scratch named by resource receipt
`target/resource-containment/runs/fabric-work-2fb7a2061ead41d5a832d7b844006f6a.json`:
`/run/media/kmosoti/data/FabricO11y/evidence/fabric-work-2fb7a2061ead41d5a832d7b844006f6a/memory/m2-shapes/memory-shapes`.
Require each manifest to equal its original pair's manifest; authenticate every
file hash; inspect logs Parquet metadata; require sum rows=4096 and table
manifest rows=4096; require groups=decoded FTF1 filters=filter manifest rows.
Scan each explicit row group and require every body byte trigram to be admitted
by its corresponding filter. Count scanned rows and compare each group metadata.
Keep no decoded full table: one scan batch/group at a time. Body trigrams repeat;
exhaustive repetitions use bounded memory and need no growing set.

Controls run without changing retained files: truncate one decoded filter
(count check must reject); replace group0's filter with an empty built filter
(actual nonempty body trigram must be rejected). Intentionally differently
grouped valid files must pass their own physical consistency checks. Header,
row-drop and heap controls remain in original screen; this validator adds no
new global acceptance policy.

Command arrays, coordinator wrapper required:
`["cargo","build","--offline","--locked","--release","-p","fabric-server","--example","readiness_filter_review"]`
then binary `$CARGO_TARGET_DIR/release/examples/readiness_filter_review` with
`["<retained-root>","docs/experiments/benchmarks/data/readiness-labs-run-01/memory/shapes/bigrows-64/pair.json","docs/experiments/benchmarks/data/readiness-labs-run-01/memory/filter-review.json"]`.
Exit0 plus output containing both verified variants,4096 rows,zero rejected
trigrams, positive exhaustive checks and both rejected controls supports H1.
Any nonzero exit/missing evidence is failed/inconclusive, never acceptance.
Budget600s,20GiB max16GiB high0swap,4GiB owned scratch; no writes to retained
inputs, no scratch creation and no cleanup of retained evidence. Output refuses
existing destination. Spill-byte/pruning/query equivalence and spans-filter
trigram checks remain outside this diagnostic command.
