# Recovery audit boundary

The dev-small cells are diagnostic custody and query-transition observations.
They do not exercise the stage-specific disk failures and process-kill matrix
specified by R1 in `docs/experiments/benchmarks/dev-small-lab-plan.md`, and do
not qualify power-loss durability. Keep those conclusions separate.

For seeded history, partition custody explicitly. The old-history seed ledger
must identify each committed Batch by Strand label, sequence, encoded Batch
SHA-256 and byte length, and retain source tag/body-hash membership. Compare
that ledger with recovered history by identity and bytes. Compare live cycle,
ACK and recovery observations as a separate population; do not demand that
live cycle observations account for the deliberate seed prefix. H0 still needs
an explicit empty seed ledger and seed summary so an absent prefix is evidenced.

The audit should record hashes for its source snapshot and relevant frozen
oracle/checker inputs, derive custody and phase metrics from compact raw files,
and retain unsupported measurements as null with a reason. The query oracle's
provenance should include the module hash, invocation/source revision and the
retained verdict plus negative-control outcomes. A summary's `passed` field is
not independent evidence. Do not retain bulk decoded telemetry solely to make
the audit possible if compact Batch and query-verdict evidence suffices.

Clock validity is computed from raw integer realtime, monotonic and boottime
samples. A missing phase or boottime sample makes its timing claim unavailable;
it must not become zero CPU. The five-second observation-to-query target only
applies to its registered workload. A diagnostic visibility horizon and any
deliberate burst are reported as observations, without transferring steady
latency gates to those conditions.
