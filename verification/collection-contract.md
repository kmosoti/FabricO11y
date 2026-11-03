# Collection verification contract

The SDK must never wrap sequence identities, admit beyond its configured outstanding
record count, release capacity for a skipped/mismatched acknowledgement, or report
completion before the flush target is confirmed. Refusals preserve the accepted
frontier; retries retain identity and payload. Checks cover all integer inputs,
including invalid states and exhaustion, not just typical workloads.

Custody must preserve committed bytes across process death, give the same record ID
for identical retries, reject conflicts, and atomically commit records and source
checkpoints. Acknowledgement loss is an unknown result, never evidence of absence.
A mathematical transaction model may assume atomic durable commit; that assumption
must be distinguished from proofs of executable arithmetic and from fault tests of
SQLite, the operating system and storage hardware.

Two reproducible Rust programs must exercise the real SDK/collector: bounded
admission during a receiver outage followed by recovery, and subprocess death plus
retries, conflicting identities and checkpoint readback. Programs must fail on
violations, use private temporary paths, bound work and time, and clean their child
processes. Outputs identify workload parameters and measurement boundaries.

Resource comparisons report reserved payload bytes, observed RSS, admission latency,
durable completion time and storage separately. The byte-storage lower bound applies
to lossless distinct payload bytes retained in disjoint uncompressed storage; it is
not a global optimum for RSS, compressed representations, latency or throughput.
Full occupancy may attain the payload lower bound; partial occupancy can waste space.
Finite capacity cannot guarantee unlimited lossless admission during an outage.
No benchmark result establishes global optimality. Existing acceptance checks remain
required; proofs must reject false-property controls and audit Lean axiom dependencies.
