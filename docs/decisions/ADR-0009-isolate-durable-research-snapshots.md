# ADR-0009: Isolate durable research snapshots

## Status

Experimental. Implemented in the separate storage-probe package; the scoped E3,
lifecycle and cost gates are complete in the [completion contract](../experiments/ablation/end-to-end-prototype.md).
This does not accept a production storage-format migration.

## Context

Coverage receipts need an immutable dataset and an independently trusted root.
Testing restart, real missing blocks and checkpoint persistence requires a concrete
publication boundary. The existing FOL2 journal provides the retained event source;
its commit semantics have separate evidence and filesystem assumptions. The new
experiments need to compare layouts without prematurely choosing an application format.

## Decision

Publish fresh versioned JSON blocks and authenticated metadata through a staging
directory, file/directory syncs and rename. Return a Publication only after the
parent sync succeeds. The caller retains that Publication outside the snapshot.
Use an exact Event codec with hexadecimal float bits, ordered attributes and physical
positions. Later arrivals receive a new snapshot identity.

Persist page history in fresh checkpoints with a caller-retained digest. Loading
replays the validated accumulator transitions against the expected root and query;
serialized internal progress cannot authorize itself. Missing candidate data remains
incomplete. Rebuild must reproduce the original root from retained raw data.

Keep this lifecycle in `fabric-research`, with FOL2 as its journal and JSON as the
explicit research baseline. Compare the hybrid Parquet layout in a separate package.
The collection entry point is a documented offline Logs profile. Network receivers,
distributed publication and production format migration require further decisions.

## Alternatives considered

- Replace FOL2 with Parquet now: this would combine new persistence semantics with
  a layout decision before comparable costs or recovery evidence exist.
- Persist only a residual bitmap: it would omit the checked evidence supporting
  progress and could turn altered state into a false completion claim.
- Trust roots embedded in snapshots/checkpoints: replaceable data could authorize
  its own coherent modification, defeating the intended integrity boundary.

## Evidence and consequences

The [S2 result](../experiments/ablation/durable-snapshot-s2-run-01.md) records exact
round trips, real restarts, concurrent publication, missing/corrupt blocks and
read-error counterexamples. The [API](../../tools/storage-probe/DISK_API.md) states
filesystem assumptions and the cold-copy rebuild limitation. JSON is inspectable
and easy to fault-inject, but duplicates metadata and is not a selected compression
or query-performance winner. External trusted files and retained source remain
caller responsibilities; the prototype does not establish malicious-executor
correctness, raw retention or physical power-loss safety.

## Validation

Run the frozen disk, restart, retry and process-lifecycle suites; inject changed
roots, missing raw data, invalid checkpoint digests and partial read errors. Require
exact final positions and Event digests after restart. Measure receipt overhead,
retained bytes and durable publication costs against the registered layout alternatives.

## Related

- [Local prototype architecture](../architecture/research-prototype.md)
- [Snapshot-bound residuals](ADR-0008-bind-residuals-to-immutable-snapshots.md)
- [Unchanged journal contract](ADR-0006-use-framed-local-log.md)
