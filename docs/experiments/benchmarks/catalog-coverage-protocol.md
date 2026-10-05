# Catalog coverage over existing storage transitions

Status: registered before execution; **no outcome claimed**. Owner scope is the
queued catalog operations labs. This is a correctness fixture using the existing
implementation, not implementation of a catalog or release qualification.
Related [planning packet](catalog-labs-operations.md),
[history contract](../../architecture/retained-history.md),
[HIST-1–9](../../formal/verification-matrix.md) and
[target](../../../crates/fabric-server/tests/catalog_coverage.rs) apply.
No product contract, oracle, wire format or durability order changes.

## Frozen fixture and expectations

Fixture seed `0xCA7A10A1` contributes a fixed timestamp offset. This is a
deterministic constructed fixture, not randomized coverage: eight server groups,
three nodes per group, eight Batches per node. Each Batch has six logs, one metric
and three spans. Append node order is descending while the contract sorts node
identity ascending; observation times decrease with sequence and tie across
nodes. Receive times increase. Expected ledger bytes are written before IO from
the constructed Groups, never derived from Segment output or replay.

Both plans use full chains at limits 1/2/3 for logs (unfiltered, needle and
no-match), metrics and spans. Each oracle invocation writes a unique numbered
directory with ledger, query, complete answer, context, stdout/stderr and exit.
`FABRIC_STORAGE_EVIDENCE` copies these directories on fixture success; failures
preserve owned scratch. Source/binary/command/environment hashes come from the
coordinator receipt. Never treat an oracle exit 2 as a rejected negative control.

## Cells and decision rules

1. `active_pending_published_and_reclaimed_sources_cover_late_ties_exactly`:
   append four groups to the real FrameLog; collect first pages in both plans;
   append the remaining four and continue old snapshots with newest=8. The
   continuation must still match the first-four ledger despite late new rows.
   Grade active history, rotate to a pending journal, call actual bounded builder,
   grade published Segment plus its unreclaimed journal, then use Store and actual
   sealer pass to checkpoint/reclaim. Grade again and compare exact raw replay
   identities/bytes including multiplicity against the original ledger. Fresh
   History instances cover every state; reused Scan/Walk instances are primed
   before rotation and graded across pending/publication/reclaim. H1: no missing,
   duplicated or reordered row/custody identity, stable old snapshot. H0: any
   mismatch. Controls remove a row and append a duplicate; oracle must exit 1.
2. `retained_descriptors_do_not_resurrect_expired_page_snapshots`:
   independently partition ancient/current receive times into two real journals;
   actual sealer pass publishes/reclaims both. Keep History instances and old
   tokens alive, apply actual age retention, require Segment 1 removed, Segment 5
   retained and both old continuations Gone. Fresh queries match retained ledger.
   H1: live Rust object ownership does not resurrect logically expired history.
   H0: old token succeeds, retention partition differs or retained answer mismatches.
3. `optional_filter_fallback_is_exact_and_authenticated_lies_are_detected`:
   publish/reclaim eight groups; remove filter, substitute malformed bytes, then
   zero bitmap bits without updating digest. Both plans must remain exact. Forge
   the manifest digest for those zero bits: authenticated filter is accepted but
   Walk hides matching needle rows; unchanged independent oracle must reject it
   with exit 1. H1: fallback exactness and checker sensitivity. H0: fallback loses
   rows or the checker accepts the deliberate false negative. This control proves
   digest verification alone cannot establish semantic index conservatism.

Cuts are deterministic **sequential states**, not concurrent schedules or owned
process crashes. O1 and selected O2/O3 properties are exercised through real
publication/reclaim; no new catalog publication mechanism is tested. O4 tests
History lifetime, not a new Arc guard. O5 covers malformed filters and semantic
filter lies, not malformed numeric bounds. Pending and published bytes overlap;
active plus pending plus published coexistence in one state remains a separate
fixture. Partial read/write faults, catalog completeness under concurrent
retention, handle-drop/descriptors budgets, bound overflow/false negatives and
power loss remain untested here. No full O1–O5 gate claim follows.

## Admission and execution

Root admits execution serially after protocol review, under mounted data-drive
scratch and unchanged 16/20 GiB high/max, zero swap and 1800-second launcher.
Per-cell coordinator deadline 900 seconds, owned scratch ceiling 1 GiB for this
fixture (the coordinator's existing 8 GiB ceiling remains an outer guard), 16 GiB
free reserve and at most 32 MiB compact artifacts. Stop on failed oracle control,
unexpected exit, custody/query mismatch, OOM, deadline, storage budget or cleanup
survivor; preserve the seed/stage/inputs/outputs before cleanup. Freeze source and
protocol hashes before each run. Inspect receipts for actual cgroup and storage
usage; no CPU or memory performance conclusion is an acceptance criterion.

Example full-target invocation from repository root, using a fresh coordinator ID:

```bash
python3 tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py \
  --id catalog-coverage-01 --lab recovery --stage recovery --seconds 900 -- \
  env FABRIC_STORAGE_EVIDENCE=/home/kmosoti/Projects/moiric/FabricO11y/docs/experiments/benchmarks/data/lab-completion-run-01/recovery/catalog-coverage-01 \
  cargo test --offline --locked -p fabric-server --test catalog_coverage -- --test-threads=1 --nocapture
```

For an individual cell, add its exact name before `--`, then `--exact` after it.
Metrics: oracle invocation exits, row/custody mismatches, cut assertions, Gone,
command duration, cgroup peak/events, scratch and evidence bytes, cleanup outcome.

The missing-projection-table counterexample in `frontier-storage-01` remains
failed. The frozen oracle excludes whole records when declared unavailable and
cannot express surviving metadata with one missing table. Table-specific
availability needs a separate trust-boundary design/registration; no oracle edit,
envelope normalization or retrospective pass is part of this protocol.
