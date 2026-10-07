# Completion regression reconciliation

Fast01 exited1: the legacy raw-custody replay test still routed its five projection
queries through the whole-record oracle with no unavailability declaration,
expecting `complete=true`. The new production behavior correctly returns the
same48 rows with incomplete/unavailable metadata. Preserve that failed fixture,
original verification receipts and command outcome. Route ALL five original
queries in BOTH plans through the already registered availability companion with
raw_available=false, preserving the raw replay error assertion and every query.
This is a separate trust-boundary fixture correction under the accepted O6
availability decision; no original oracle or product promise is relaxed.

Clippy also rejects a nested condition in the new overlap test; collapse it with
unchanged assertions in a separate implementation change. Register
catalog-coupled-completion-fast-02 (coordinator/verification,600s) with a2MiB
projected additional evidence reserve. One contained serial helper first copies
Fast01's exact existing verification receipts, then invokes catalog cleanup for
Fast01, formats Rust and reruns the unchanged full fast profile. Combining these
steps avoids another repeated1.4MiB source/protocol snapshot while retaining each
command/exit and byte-verifying failure archives. No resource cap is increased.
Final manual documentation checks and resource inventory follow separately.
