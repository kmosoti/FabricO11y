# Concepts

Product concepts:

- [Strand](strand.md): the ordered lineage that sequencing, deduplication and acknowledged progress are defined over.
- [Custody](custody.md): who is responsible for telemetry before and after a durable acknowledgment.

FOL2 demonstration concepts:

- [Event](event.md): identity, time, context, payload, and current ownership.
- [Event generator](generator.md): repeatable synthetic input, settings, and iterator ownership.
- [Event buffer](buffer.md): bounded FIFO occupancy, rejection ownership, and batches.
- [Event log](event-log.md): the local commit and replay boundary.
- [Delivery ownership](delivery-ownership.md): the target handoff, the local implementation, and how it differs from Rust value ownership.
- [Glossary](../glossary.md): precise meanings of the types already used.

Add separate concept pages when their semantics need explanation beyond the glossary. Product terms such as Spindle, Batch, Spool, Segment, completeness and freshness are defined in the glossary; they get a page when their semantics need more than a row.

Return to the [architecture documentation](../README.md).
