# Concepts

- [Event](event.md): identity, time, context, payload, and current ownership.
- [Event generator](generator.md): repeatable synthetic input, settings, and iterator ownership.
- [Event buffer](buffer.md): bounded FIFO occupancy, rejection ownership, and batches.
- [Event log](event-log.md): the local commit and replay boundary.
- [Delivery ownership](delivery-ownership.md): the target handoff, the local implementation, and how it differs from Rust value ownership.
- [Glossary](../glossary.md): precise meanings of the types already used.

Add separate concept pages when their semantics need explanation beyond the glossary. The local log implements the receiver-side commit boundary; a network delivery protocol remains in the [proposal](../architecture.md).

Return to the [architecture documentation](../README.md).
