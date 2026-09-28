# Strand

A **Strand** is one ordered telemetry lineage produced by one Spindle generation: `StrandId = (SpindleId, generation)`. Its Batches carry sequences 1, 2, 3, ... with no holes.

## Why it exists

Every delivery rule needs a scope. "Is this a retry?", "is something missing?", "how far is this acknowledged?" only have answers relative to an ordered lineage whose positions are stable across retries and restarts. The Strand is that scope. Making it explicit keeps it from being confused with a TCP connection, an HTTP session, a Rust thread, a log file, a journal file, a Parquet Segment or a query result, none of which has those properties.

## Who owns it

The Spindle creates a Strand when it first writes its identity file (generation 1) and assigns sequences as it commits Batches to its Spool. The server never assigns sequences; it records, per Strand, the last committed sequence and the digest of that Batch's bytes.

## Rules

- A Batch's bytes are fixed once committed to the Spool; a retry sends the same bytes.
- On the server, sequence `last + 1` commits; `last` with equal bytes is a duplicate; `last` with different bytes is a conflict; below `last` is stale; above `last + 1` is a gap ([ADR-0013](../decisions/ADR-0013-deliver-batches-in-order-with-bounded-dedup.md)).
- No sequence follows `u64::MAX`; an exhausted Strand accepts nothing new.
- A new generation is an independent Strand that starts at sequence 1.
- Generation 0 and sequence 0 do not exist; the core's types cannot express them.

## Where it is realized

`SpindleId`, `StrandId` and `next_sequence` in [fabric-core](../../crates/fabric-core/src/strand.rs); the decision in [`fabric_core::delivery`](../../crates/fabric-core/src/delivery.rs); the envelope fields `node_id`, `generation`, `sequence` in [fabric-frame](../../crates/fabric-frame/src/envelope.rs); the server's replayed Strand table and `streams.json` checkpoint in the [store](../../crates/fabric-server/src/store.rs).

## Related

[Custody](custody.md), [delivery view](../architecture/delivery.md), [glossary](../glossary.md).
