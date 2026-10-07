# Capacity evidence attribution closeout

Status: registered before final audit. This supplements the
[aggregate audit](catalog-resource-consolidation-protocol.md); it changes no cap,
performance rule, oracle, frozen driver or historical result.

The conservative capacity upper bound was inconclusive. Establish a mandatory
capacity lower bound using the top-level spill, builder-spill, borrowed,
many-segment and lifetime datasets, plus baseline-freeze as explicitly assigned
by the [capacity admission](catalog-evidence-capacity-admission-protocol.md).
Include the capacity CR3 failure archive and manifest under the legacy memory
directory. Deduplicate by device/inode. Other receipts and coordinator overhead
remain in the full aggregate inventory; this lower bound does not claim complete
exclusive ownership of all overhead. If this mandatory subset alone exceeds
768 MiB, report a confirmed allowance violation. Do not increase the allowance.

Controls must include baseline and failure archive/manifest paths, exclude
unassigned overhead from the lower bound, deduplicate an alias, and detect an
excess independently in file lengths or allocated blocks. Original conservative
bounds and aggregate controls remain in the output. Actual inventory must contain
the baseline and both CR3 archive/manifest files; missing evidence fails closed.

Run the unchanged resource wrapper with fresh identifier
`catalog-campaign-summary-02`, coordinator/verification, 120-second allowance,
and `python3 -B tools/bench/labs/catalog/campaign_summary.py`. Exit zero means
the audit completed; the capacity status field can and must report a violation.
This distinction preserves the observed resource failure without preventing its
measurement. No new native performance workload is admitted.
