# ADR-0002: Return an event when the local buffer is full

## Status

Accepted for the single-threaded Stage 3 prototype. This does not establish a network or durable delivery guarantee.

## Context

The synthetic producer and the batch consumer currently run on one thread. A bounded `VecDeque` must state who owns an event when it has no room. Silent loss would hide whether later stages preserve observations.

## Decision

Use a positive event capacity and `try_push(event) -> Result<(), Event>`. On success, the buffer owns the event. On failure, it returns that same owned event to the caller without changing the queue. `take_batch` accepts a positive limit and transfers the oldest events to the caller. The demo drains a batch on rejection, then retries the returned event.

## Alternatives considered

- **Block until space exists:** In this one-thread execution path, the producer would wait while the only consumer cannot run. A full buffer after two pushes followed by a third push is a concrete non-progressing trace. A concurrent consumer would change that assumption and needs a new design.
- **Drop on full:** A full buffer followed by a new event would destroy that event before the caller could retry or account for it. An explicit loss policy may be useful later, but it does not preserve every generated event in this demonstration.
- **Return the event:** The caller can drain and retry, or deliberately choose another action. Rejection is visible in the `Result` and ownership remains explicit.

## Evidence and argument

Let capacity be `C > 0` and occupancy be `n`. Initially `n = 0`. A successful push is allowed only when `n < C`, so its result is at most `C`; rejection leaves `n` unchanged; taking a batch decreases `n`. By induction, `0 <= n <= C`. `push_back` and draining from the front preserve FIFO order. Positive batch size means draining a full buffer removes at least one event, so one returned event can then be retried successfully in this single-threaded path.

These claims concern the buffer API and the demo's retry path. A caller can still discard an `Err(event)`, the process can fail, and printed events are not durable. The logical event count is bounded; allocation size also depends on event contents and `VecDeque`'s allocation strategy.

The [buffer tests](../../tests/buffer.rs) check rejection ownership, FIFO batches, bounded occupancy, and one retry stream. They are executable checks of representative traces, not a proof of every Rust execution. No throughput comparison selected `VecDeque`; it is the initial standard-library FIFO baseline.

## Consequences

The producer must handle rejection. The demo uses a consumer call to make space before retrying. No background worker or blocking synchronization is required. The API does not silently delete a rejected event. `Result<(), Event>` is a large return type; Clippy flags it. The implementation keeps the event by value to avoid adding a new box allocation on every attempted push. This is a simple ownership baseline, not a measured performance winner over alternative API shapes.

## Validation

Run `cargo test --offline`. A counterexample would be a trace with occupancy above capacity, changed FIFO order, or a rejected event that cannot be recovered for retry. For a later concurrent or durable pipeline, revise the ownership and failure assumptions before reusing this argument.

## Related

- [Current system](../architecture/system.md)
- [Stage 3 learning path](../LEARNING_PATH.md)
- [Batch-size probe](../experiments/benchmarks/batch-size-stage3.md)
