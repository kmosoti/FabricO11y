# Coupled labs: initial diagnostic admission

Status: registered before execution, 2026-10-07. The owner authorized launching
the [prepared labs](coupled-labs-plan.md). Original failures, performance rules,
oracles and frozen measurement identities remain unchanged.

## Prospective evidence allocation

Capacity's original 768 MiB allowance failed by 10,002,432 allocated bytes. Retain
that failure. For subsequent coupled jobs only, allocate capacity 832 MiB within
the unchanged 2 GiB catalog aggregate and unchanged 1 GiB query allowance.
This prospective amendment does not recertify earlier runs. Capacity assignment
is the mandatory capacity datasets/baseline/CR3 failure archive from the registered
attribution audit, plus new top-level `catalog-borrowed-attribution-*` datasets.
Other coordinator/recovery receipts and failures remain aggregate overhead.
Any new capacity failure archive is additionally charged to capacity. Report all
uncertain/shared categories conservatively; no evidence is deleted to pass.

Reserve at most 192 MiB incremental retained evidence for the new wave, including
32 MiB operations, 48 MiB capacity, 48 MiB query and 64 MiB failures/coordinator.
Every measured cell must fit its projected evidence and failure reserve before
dispatch. The full inventory includes datasets, failure archives/manifests,
launcher and cleanup receipts. The prior aggregate was 1,693,552,640 allocated
bytes; remeasure it. Enforce unique-inode lengths AND allocated blocks. Keep
16/20 GiB high/max, zero swap, 8 GiB scratch, 16 GiB free-drive reserve, serial
workloads and remaining original 14,400-second frontier budget. New IDs start
`catalog-coupled-`; no new four-hour allocation.

## O6 reproduction

Register the existing unchanged test command:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-coupled-o6-reproduce-01 --lab recovery --stage recovery --seconds 300 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 8 -- cargo test --offline --locked -p fabric-server --test completion_storage --all-features missing_query_tables_are_incomplete_with_independently_declared_unavailability -- --nocapture
```

Expected diagnostic: retained partial-table metadata differs from the independent
oracle's whole-record-unavailability model, despite zero expected/returned rows.
Observe the actual result. Exit 101 remains failed; neither exclusion nor envelope
assertions change. Preserve source archive, stderr, oracle fixtures and command.
This diagnostic does not resolve availability semantics or clear full fast.

Before execution the admission helper checks representative unique-length and
allocated-block excess and prospective capacity excess; bad reserves fail closed.
Its controls operate on numeric copies, not production evidence. It reports the
original allowance violation separately, checks prospective reserve against
unchanged aggregate and future capacity limit, and re-inventories after child
termination without replacing the child's failure status. Archive/byte-verify
failure scratch through the existing cleanup helper before removal.

## Remaining dispatch

O7 is an archived-ledger diagnostic, separately registered before its validator.
C4/Q4 require concrete frozen binaries, fixture/control commands and supplemental
protocols before native measurements. C5/O8 retain their prepared dependencies
and are not admitted by this initial diagnostic. Stop dependent work on a real
semantic defect; preserve nulls and explicit deferred outcomes. This launch is
not qualification, release, contract or oracle change.
