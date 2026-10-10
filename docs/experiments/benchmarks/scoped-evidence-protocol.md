# Scoped Segment evidence reuse

Registered 2026-10-10 before measuring the proposed repair. The first complete
million-row Segment query cell exposed repeated raw-Batch scanning while
constructing authorized freshness and receive bounds. Its original latency
result remains a failure under the [release query gate](../formal/release-service-query-protocol.md).

## Hypothesis and invariant

The repeated raw scan, rather than selective row retrieval or bearer hashing,
dominates warm scoped-query latency. Reusing a validated, bounded summary of a
fully included immutable Segment should remove that repeated cost. The null is
that the summary does not materially reduce measured complete-query time.

Keep authorization, exact rows, signal-specific freshness, receive bounds,
snapshot pagination, raw availability, retention and incomplete answers unchanged.
No persistent format, custody or publication order changes. A partial snapshot
must retain its exact scan; a cached full-Segment aggregate cannot describe it.

## Procedure

Use the failed cell's original synthetic state and registered query coordinates
for a diagnostic probe of baseline and repaired native libraries. Record exact
source/binary identity, state inventory, command and exit status. Open a fresh
History for each cold repetition; report cold and subsequent warm queries
separately, including every page. Compare complete answers and the unchanged
independent query oracle, not just row counts. Do not include compilation or
oracle execution in the query timer. Measure at most three paired repetitions.
Report wall time, CPU, RSS and cgroup peak; this local diagnostic is not package
acceptance and is subject to concurrent-host interference.

Run under the resource launcher, at most 6 GiB, no swap, two query CPU equivalents,
1,800 seconds and 5 GiB owned scratch on the data drive. Coordinate the 20 GiB
aggregate allowance and 100 GB disk ceiling. Preserve failed evidence before
cleanup. Do not change active campaign inputs.

## Counterexamples and decision

Exercise scope narrowing and widening, every signal, partial snapshots, raw-file
replacement/corruption/removal, malformed excluded records, cache admission and
eviction. Finite native controls must reject a stale identity or future snapshot
aggregate. Report the resident and staging charges separately from actual RSS.
File identity/change stamps supplement the immutable-file assumption; they do
not establish detection of silent hardware corruption that leaves those stamps
unchanged.

Retain the repair only if exactness controls pass and the measured repeated cost
falls. Rebuild the package and rerun all four registered million-row query cells
on the repaired candidate; the existing 2-second p99 ceiling and all correctness
and resource gates still apply. Cold-start cost remains measured. Results are
published in the [research wiki](https://github.com/kmosoti/FabricO11y/wiki).
