# Fabric O11y

A Rust learning project for building an observability pipeline from small, explicit pieces. The goal is to understand the design as it grows: first the data model, then buffering and durability, then measurement and formal reasoning.

Current maturity: typed events, a deterministic synthetic generator, a bounded in-memory FIFO buffer, and an optional local append-only log with replay. A separate [TLA+ delivery model](formal/delivery/README.md) checks the acknowledgement rule. There is no network ingestion, external ACK protocol, or query engine yet.

The first [local-log baseline](docs/experiments/benchmarks/local-log-stage6.md) now records append latency, throughput, memory, bytes, and recovery for one synthetic workload on WSL2/ext4.

[Architecture documentation](docs/README.md) describes the implementation. The [learning path](docs/LEARNING_PATH.md) gives the sequence, and the [original blueprint](docs/architecture.md) records longer-term proposals.

## Start here

You need a recent Rust toolchain. From this directory, run:

```sh
cargo run
```

This generates three events with seed `42`. The third event finds the two-slot buffer full, so the demo prints a batch, retries that event, then prints the final short batch. To choose the seed and count, run `cargo run -- 42 3`. Repeating the command produces the same output. The generator uses the `fake` crate for seeded values and a small adapter for Fabric's event IDs and times. Read the [event types](src/lib.rs), [generator](src/generator.rs), [buffer](src/buffer.rs), and [demo loop](src/main.rs) in that order. This output is for inspection, not a benchmark.

To commit the same synthetic workload locally and inspect it in a new process:

```sh
cargo run --offline -- write target/learning-events.log 42 3
cargo run --offline -- replay target/learning-events.log
```

Repeat `write` with the same seed and count to see prefix recovery without duplicate appends after an ordinary process exit. A successful write line follows an event-data sync and a separate commit-marker sync; the [storage view](docs/architecture/storage.md) explains the storage assumptions and what to do after an I/O error. Use a fresh path for a different workload.

For agent skills, documentation checks, hooks, and optional editor tools, see the [contributor guide](docs/CONTRIBUTING.md).

## How we will build it

Each lesson adds one idea, explains the trade-off, and leaves the project runnable. The Rust prototype now has a bounded buffer and local log; the first TLA+ model asks when the sender may release an event. Async, Arrow, and a UI will come in when earlier steps give us a concrete question to answer.

The first system goal is a small pipeline. Its local parts now run:

```text
generated events → bounded buffer → batches → local log → replay
```

We will track correctness and resource costs together: throughput, latency, memory, allocations, disk use, loss, and recovery time. “Faster” only counts when the workload and trade-offs are stated.

The [Homa/SIRD transport research track](docs/experiments/ablation/receiver-driven-transport.md) has [receiver-credit](docs/experiments/ablation/receiver-credit-h1-run-01.md) and [one-packet-prefix](docs/experiments/ablation/unscheduled-prefix-m2-run-01.md) packet-slot results. The first lowers modeled switch peaks by moving waiting to senders; the second lowers tiny-message tail latency while increasing incast switch occupancy. These models are separate from the application; no network ingestion or real-host transport result exists yet.
