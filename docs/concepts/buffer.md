# Event buffer

The [buffer module](../../src/buffer.rs) is a local FIFO queue of owned [events](event.md). It is the first place where events can wait after generation. It has no worker thread or disk backing.

## Inputs, outputs, and ownership

`EventBuffer::new` requires a positive event capacity. `try_push(event)` accepts ownership if there is room. When full, it returns `Err(event)` with the same owned value; the queue is unchanged. `take_batch(max_events)` requires a positive limit and returns up to that many oldest events in a `Vec<Event>`. Once taken, those events belong to the caller.

The [demo loop](../../src/main.rs) uses capacity `2` and batch size `2`. On rejection it prints one batch, then retries the returned event. At generator exhaustion it drains any short final batch. See the [control flow](../architecture/ingestion.md).

The `Err(event)` result is a local backpressure signal: the demo stops taking new generator events while it makes room and retries. No other thread is blocked, and this does not regulate a network sender.

## Invariants and limits

Logical occupancy remains `0..=capacity`; accepting pushes only below capacity and draining from the front preserves FIFO order. [ADR-0002](../decisions/ADR-0002-reject-full-buffer.md) records the assumptions and argument. [Tests](../../tests/buffer.rs) exercise rejection ownership, order, partial batches, and one retry stream.

The bound counts events, not bytes. A large event can still consume much memory, and `VecDeque` may allocate beyond its current length. A caller may deliberately discard a rejected event; printing a batch does not make it durable. The [current batch-size measurements](../experiments/benchmarks/generator-library-stage3.md) describe this particular synthetic pipeline, not a general queue benchmark.
