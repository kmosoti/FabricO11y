# Custody

**Custody** is responsibility for preserving a piece of telemetry. At any moment some component must be able to recover it after the failures the contract names, or the loss must be recorded as a gap.

## The custody chain

1. A source read builds an owned candidate Batch in the Spindle. Custody of the source position stays with the source until the Batch commits: the cursor moves only in the same Spool commit as the collected lines.
2. The Spool's two-sync commit makes the Spindle the durable custodian. From here a retry always has the exact bytes.
3. Delivery offers the oldest unacknowledged Batch. The server's commit thread appends it with data and marker syncs; only then does it answer `ack`.
4. The ACK transfers custody. The Spindle records its ACK cursor by synced rename and may then reclaim whole Spool files at or below it.
5. The server journal, then Segments, hold custody of committed history until retention deletes it; a checkpoint preserves Strand state before journal files are reclaimed.

An ACK sent before step 3 completes would let the Spindle forget a Batch the server can still lose; the TLA+ model's `EarlyAck` configuration shows that counterexample, and the semantic mutant `M-DEL-ACK` checks that the code rejects it.

## Relation to Rust ownership

Custody is a protocol responsibility, not Rust value ownership. A `Batch` value can be dropped while its custody is held by a file on disk, and a value can be owned in memory by a component that holds no durable custody at all (see [delivery ownership](delivery-ownership.md) for the FOL2 demonstration).

## Where it is realized

The `commit_group` use case in [fabric-app](../../crates/fabric-app/src/delivery.rs) (no ACK without a durable commit), the Spool in [src/spindle/spool.rs](../../src/spindle/spool.rs), and the frame log in [fabric-frame](../../crates/fabric-frame/src/frame.rs). The claims are rows DEL-1 and SPOOL-1 to SPOOL-3 of the [verification matrix](../formal/verification-matrix.md).
