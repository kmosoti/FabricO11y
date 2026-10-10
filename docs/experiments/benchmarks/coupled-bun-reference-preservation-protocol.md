# Exact shared runtime preservation for failed closeout

Status: registered before reference-preservation execution. Cleanup02 exited1:
archiving the failed closeout01's copied Bun runtime again exceeded prospective
query evidence admission. No source was deleted. All current caps remain.

Read-only inspection found the sole failed-scratch file is the79500640-byte Bun
runtime copied for manual docs. Its exact bytes already exist in the retained
archive `lab-completion-run-01/coordinator/failure-catalog-docs-01.tar.gz`, member
`coordinator/catalog-docs-01/tmp/bun`. The archive SHA256 is
`60707809e50853d3690c36218f9941f48cf8adf5d0bf81213f2d35b9cd4cfede`;
the decoded member SHA256 is
`a83d263767d839e4d2649ca8e35d07159c7afc99afdc96d731ced29e056dda0c`.
The existing manifest names that same regular member and size/hash. Inspection
is not a substitute for the contained preservation command below.

The new narrow helper authorizes only closeout01's exact retained root obtained
from its launcher receipt. Reject extra files, symlinks, different runtime bytes,
missing/duplicate/nonregular canonical members, or any archive/hash drift.
Compare complete source/member streams byte for byte, verify hashes/lengths,
and persist a fsynced mapping from the complete failed-scratch source manifest
to this retained canonical member BEFORE deletion. Rehash source and archive,
require inactive units, then remove only that exact owned root. Keep the old
archive/manifest and all failures/results. Inject changed source and missing
canonical member using the same checker and require rejection.

Reference preservation is lossless storage sharing of existing identical bytes;
it does not regenerate unknown state or discard runtime identity. The closeout
independently rechecks the archive identity/member/manifest against the declared
source manifest. Record command, exit, cgroup resources, data-drive location,
cleanup and observations. Root runs one job at a time under the same budgets.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-coupled-bun-preserve-01 --lab coordinator --stage verification --seconds 60 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 4 -- python3 -B tools/bench/labs/catalog/coupled_bun_preserve.py
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-coupled-continuation-cleanup-03 --lab coordinator --stage verification --seconds 60 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 4 -- python3 -B tools/bench/labs/catalog/coupled_cleanup.py --id catalog-coupled-continuation-cleanup-03 --only-job catalog-coupled-continuation-cleanup-02
```

Cleanup03 archives only cleanup02's retained scratch using the unchanged generic
helper. Its admission failure and original receipts stay failed. Then the already
registered closeout02 runs with preserved protocol link context and recognizes
the separately registered exact member-reference receipt. This adds no native
experiment, qualification, cap increase or historical regrading.
