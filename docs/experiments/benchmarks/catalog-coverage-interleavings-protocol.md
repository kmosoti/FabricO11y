# Catalog mixed-source and numeric-bound additions

Status: pre-execution registration for two additional existing-implementation
fixtures. Extends [coverage protocol](catalog-coverage-protocol.md); historical
results remain attached to their captured source. No oracle or policy changes.
Scope: selected O1/O5 coverage, not full O2/O3 concurrency or typed-handle validity.
The separate publication/reclaim/shared-handle registration must precede its load.

## Frozen fixture

Use the same constructed seed `0xCA7A10A1`, eight server groups/three nodes,
late observation times, equal-time identities, exact pre-IO Batch ledger and
numbered independent Python oracle artifacts. Root executes serially after
source/protocol hash capture. Both plans remain under their existing contracts.

`simultaneous_active_pending_and_published_sources_cover_every_identity`:
append groups 1–2, rotate label 1 and publish via real bounded builder without
reclaim; append groups 3–4 and rotate label 3 without publication; append groups
5–8 in the active journal. Assert only Segment 1 exists while both sealed files
remain. Full chains at limits 1/2/3 over all three signals must match independent
ledger in fresh and primed reused Scan/Walk readers. Close FrameLog, open real
Store, run actual sealer pass: Segment 3 publishes and both sealed journals are
reclaimed; active groups remain visible. Fresh/reused answers and exact raw replay
with multiplicity must still match all 24 Batches. H1: three source kinds and
overlapping published/journal bytes preserve one logical coverage. H0: any custody,
row, ordering, envelope or transition assertion differs.

`absent_numeric_bounds_fall_back_and_lying_footer_bounds_are_rejected`:
build/publish/reclaim the same ledger. Rewrite only time-column row-group footer
statistics using Parquet's metadata writer while preserving encoded data pages.
Restore original bytes before every variant, update manifest file size/digest,
retain row count, and require row_group_bounds can open the table. Variants:

- Remove statistics: API must return conservative `(0,u64::MAX)` for all groups;
  Scan and Walk full chains must match the unchanged oracle (exit 0).
- Reversed bound `(3000000000000,0)`: deliberately malformed pruning evidence.
- Plausible lie `(3000000000000,3000000000100)`: bounds omit every actual row.

For both deliberate numeric lies, each plan's answer must be rejected by the
unchanged oracle with exit 1. This establishes checker sensitivity to bad pruning
evidence, not runtime semantic validation of authenticated footer contents.
H1: absent statistics remain conservative and the independent checker rejects
both defects; H0: absent bounds drop rows or either negative control is accepted.
No result may silently convert reader failure to successful empty data.

## Commands and containment

Use a fresh job ID and FABRIC_STORAGE_EVIDENCE directory for each selector:

```bash
python3 tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py \
  --id catalog-mixed-sources-01 --lab recovery --stage recovery --seconds 900 -- \
  env FABRIC_STORAGE_EVIDENCE=/home/kmosoti/Projects/moiric/FabricO11y/docs/experiments/benchmarks/data/lab-completion-run-01/recovery/catalog-mixed-sources-01 \
  cargo test --offline --locked -p fabric-server --test catalog_coverage simultaneous_active_pending_and_published_sources_cover_every_identity -- --exact --test-threads=1 --nocapture
```

For the second cell substitute ID `catalog-numeric-bounds-01` and selector
`absent_numeric_bounds_fall_back_and_lying_footer_bounds_are_rejected` throughout.
Mounted data-drive scratch; 16/20 GiB outer high/max, zero swap, 900-second cell,
1 GiB intended scratch, 16 GiB free reserve and 32 MiB evidence per cell.
Stop on failed control, unexpected reader/metadata-writer error, correctness
mismatch, OOM, deadline, storage limit or surviving descendants. Preserve owned
failure state; clean success scratch only after grading. Record actual commands,
exits, source/binary/protocol hashes, cgroup peak/events and evidence size.

Limits: no concurrent mutation schedule, crash, power loss, shared-handle API,
partial IO or table-specific metadata oracle. Numeric lies are intentional
negative controls against the independent checker; they do not assert that the
product accepts malformed bounds safely. Missing projection tables still need
separate trust-boundary design; the earlier failed fixture remains failed.
