# Explicit transcript selectors for catalog experiments

This separate trust-boundary registration records the existing completion
[transcript adapter](../../../tools/bench/labs/completion/profile.py) and its
new explicit selector/page-limit parameters before new measurements. The prior
untracked adapter is preserved in the `catalog-baseline-freeze-01` coordinator
source archive; historical receipts remain attached to that source.

Defaults retain the old out-of-window empty query and limit 10000. An explicit
`empty_text=True` selects an in-window absent substring; an explicit `limit`
selects 1 through 10000. The adapter checks the complete query object against
these declared parameters. It derives the safety bound on pagination from that
limit and still grades every page with the unchanged independent Python oracle.
There is no first-page-only shortcut, changed availability declaration or altered
oracle expectation. Unknown shapes and invalid parameter types are rejected.

The adapter's controls must reject changed selectors and page limits for legacy,
absent-text and small-page variants, alongside its existing ledger, first-page,
continuation, snapshot and lossless archival controls. New comparison protocols
must explicitly declare nondefault parameters. Parameterization permits the CR2
and Q3 comparisons without silently changing the older registered workload.

This registration claims no execution result. Root runs the controls under the
resource launcher and records their actual exit before admitting measurements.
