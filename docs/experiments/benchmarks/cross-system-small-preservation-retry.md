# Small failed-tree preservation retry

The first locality-report cleanup attempt was rejected before preservation
because its default 192 MiB archive reservation did not fit the remaining query
evidence allocation. Its receipt and the original tree remain. The failed fast
check also leaves a launcher-owned tree; its workspace tests passed but Clippy
rejected constant feature assertions in two new cadence examples. The assertions
are replaced by an explicit feature guard before scratch creation. Original
benchmark binaries, source snapshots and results retain their measured identity.

This retry permits configuring the existing preserver with a **smaller** archive
ceiling, from 1 through the unchanged maximum of 192 MiB. Admission, bounded
writing, readback and the receipt use the same chosen ceiling. Raw input stays
capped at 512 MiB, and no evidence category grows. Positive boundary controls,
invalid 0/193 MiB controls and an actual over-cap writer rejection supplement
the unchanged payload/member/link controls. Complete preservation and two exact
readbacks still precede deletion; failure retains the original.

Run each known failed tree with `--archive-cap-mib 1`, a fresh coordinator ID
and a fresh evidence destination. Reserve 2 MiB per call for archive/metadata.
This covers locality-report-01, its rejected cleanup attempt, and final-fast-01.
No benchmark measurement, correctness threshold, native oracle or product
contract changes. Final verification will use fresh fast-02 receipts and preserve
the failed fast-01 outcome.
