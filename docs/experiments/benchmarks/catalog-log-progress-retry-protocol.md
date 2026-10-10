# Log progress and owned projection retry

This amendment registers a fresh execution of the unchanged hypotheses, fixtures,
timing boundaries and decision rules in the
[original protocol](catalog-log-progress-protocol.md).

`catalog-log-progress-round-01` stopped at projection-control compilation with
Rust exit 101: two new test `KeyValue` constructors omitted the pinned generated
type's `key_strindex` field. Its five progress controls and six existing collection
ownership controls exited 0. No projection control or timing executed. Preserve
that failed run and its exact sources; it is not a performance result. Supply
the two omitted default fields without changing expected data or assertions.

Root dispatches `catalog-log-progress-round-02`, preparation stage, 900-second
deadline, prospective 2 MiB reserve after coordinator snapshots and 1 MiB driver
output cap. Run all original controls and the full 24-arm comparison again. The
driver accepts this exact fresh ID; it copies both protocols and freezes the
corrected sources. Do not overwrite round 01 or change any acceptance threshold.

Before controls, use the existing exact preservation helper to reclaim only the
owned, inactive failure scratch of `catalog-log-progress-round-01`, recording
`catalog-log-progress-cleanup-01`. Confirm the producer has stopped, archive every
remaining file with manifest and byte readback, and remove only that owned root.
Preserve all original command/output receipts even if its fixture tree is empty.
Cleanup failure stops the retry. This operation shares the retry's containment,
deadline, evidence reserve and accounting; no separate frontier is granted.

The already registered `catalog-log-progress-checks-01` remains the final
600-second verification job: full fast profile and three manual documentation
gates, with a 1 MiB prospective reserve and 512 KiB driver output cap. Final
checks also parse the current round's Python drivers. Its temporary Bun copy
uses the existing exact archive-member reference before removal.

All descendants run through `tools/resource_group.py` on the data drive under
the existing 20 GiB maximum, 16 GiB high, zero swap and 30-minute outer deadline.
All prior frontier, stage and evidence consumption remains charged. No remote
work, oracle modification, qualification or service-rate claim is authorized by
this amendment.
