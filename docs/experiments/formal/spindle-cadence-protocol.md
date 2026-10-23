# Spindle cadence and visibility observations

## Retained failure and diagnostic boundary

Split smoke `split-debian-fedora-smoke-15` uses product `aaca52d`, packaged
snapshot `c0cc668` and harness `5365eaf`. Its complete 19-gate record has exact
custody, all query phases and independent offer counts passing, but offered-edge
ACK p99 upper bound 1.016655693 s exceeds 1 s. The five visibility p99 upper
bounds span 6.591375325–9.413178984 s, above 5 s. The edge's longest source-debt
busy period is 25.089556484 s, exceeding the registered five-second clearing
bound. Both host shutdowns were confirmed. No release acceptance is credited.

Use a contained read-only diagnostic on the original stopped evidence before
choosing a fix. Separate measured ACK distributions by record kind and compare
creation-to-ACK with actual successful-attempt time. Inventory visibility sweep
clock widths, per-node polling intervals and the age of each query's declared
upper bound at completion. Retain all rows, original gate outcomes and hashes.
Diagnostic statistics cannot turn an upper bound into an exact visibility time,
produce inferred timestamps or promote the failed run.

Leading hypotheses: idle trace polling and one-second log polling contribute
source waiting, while grouped clock brackets and aging per-name query windows
contribute visibility uncertainty. Compare these against actual transport time
and preserved query/clock witnesses. Keep all independent oracles and original
thresholds. Register any repair separately before new execution; product changes
require successor packages, while observation-only repairs require fresh affected
controls and cells.
