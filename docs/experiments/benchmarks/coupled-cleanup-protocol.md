# Prospective catalog-coupled failure cleanup

Status: **prepared for separate registration; not executed**. Authorizes only
explicit completed `catalog-*` job IDs supplied by root. New
[`coupled_cleanup.py`](../../../tools/bench/labs/catalog/coupled_cleanup.py) does
not modify the older completion cleanup helper, historical caps or old outcomes.
No native workload/build, SDK removal, shared-cache removal or blanket cleanup.

The older helper's256MiB-per-legacy-directory check belongs to its original
campaign. Catalog's separately registered allocations assign coordinator and
recovery provenance to aggregate overhead; coordinator has approximately249.35MiB
logical bytes and will cross that older guard without violating the new assignment.
This prospective helper uses existing `coupled_admit.observe`: catalog2GiB,
coupled incremental ceiling **1,894,879,232bytes**, capacity832MiB, query1GiB.
It also retains that helper's conservative query reservation rule. This is no new
allocation and does not retroactively regrade old cleanup or capacity failures.

## Ordering and ownership

Require resource launcher, private data-drive scratch and16GiB drive reserve.
`--id` and repeated `--only-job` must be exact catalog IDs; duplicates/self-selection
fail. Selected coordinator receipts must be completed and match their IDs. Copy
original outer launcher receipts without changing bytes. Systemd must report
inactive/failed or explicitly not-found; unknown/active state fails closed.
Retained root must be exactly `STORAGE/evidence/<original fabric-work-unit>`,
not a symlink. Reject linked/special source files and linked receipts/destinations.

1. Observe current budgets and reserve1MiB report/provenance margin. Record all
   admission/error events in a fresh root cleanup JSON and coordinator stdout.
2. Inventory every regular retained file's size/SHA-256. Stage compressed archive
   on owned scratch, or verify an existing archive. No extraction into workspace.
3. Admit measured prospective archive/manifest bytes, rounded conservatively for
   allocation plus receipt/report margin, BEFORE persistent copy or deletion;
   capacity failure archives also reserve capacity. Overbudget leaves originals.
4. Copy and fsync archive/manifest/launcher receipts. Verify exact member names,
   no duplicates, regular types, lengths and SHA against originals. Rehash source
   to reject movement; observe actual persisted budgets before deletion and
   recheck inactive unit. Only then remove that exact retained root.
5. Observe after each job and completion; preserve original job exits, immutable
   archives and success/failure report. Keep all original coordinator logs.

Controls run inside the helper on owned scratch: an intact three-byte archive
must verify; altered payload, duplicate member and missing member must reject.
No control deletes an original fixture. Byte verification plus fsync is a local
filesystem procedure, not a tested physical-power-loss guarantee. Directory sync
and raw hardware failure are outside this cleanup's established evidence.

Exact prospective command pattern (root replaces IDs from current failed receipts;
all budgets/source/protocol hashes captured before execution):

```sh
python3 tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-coupled-failure-cleanup-02 --lab coordinator --stage verification --seconds 120 -- python3 -B tools/bench/labs/catalog/coupled_cleanup.py --id catalog-coupled-failure-cleanup-02 --only-job catalog-coupled-o6-raw-red-01
```

The example selected ID is a placeholder unless it matches the real receipt;
root must use the exact recorded raw-red job ID. Add further `--only-job ID`
arguments only for reviewed completed failures. Fresh report required. Empty
roots still receive complete empty archives; already removed roots are recorded,
not recreated or reinterpreted. Resource helper observations and archive admission
must succeed; failure stays failed. Processing is serial, not transactional:
a later failure can follow earlier verified deletion. Each event records that
progress, so never infer 'no deletion' from overall exit failure. Source staging
cleans on normal exceptions; outer launcher preserves interrupted failure scratch.
Final output/receipt bytes occur after the inventory snapshot and are accounted
with margin and root's subsequent audit. No qualification or semantic claim follows.
