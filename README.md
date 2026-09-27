# Fabric O11y

Fabric O11y is an experimental observability system built in Rust. Its goal is to collect observations about running systems, preserve them through failures, and make them available for investigation. It is also a learning project: each step should leave a runnable system whose behavior and trade-offs can be explained.

## The idea

An observation has an identity, a time, a value, and context. As it moves from a collector toward storage, someone must remain responsible for preserving it. A full queue, a crashed process, or an uncertain write should have an explicit outcome.

Fabric starts with those contracts. Rust types and ownership make responsibilities visible in the code; tests and small formal models challenge correctness claims; measured experiments compare the cost of different mechanisms. Collection protocols, queues, encodings, and storage engines are choices to investigate as the system grows.

The guiding question for every component is: **what does it promise, what happens when it fails, and what evidence supports that promise?**

## What runs today

The current prototype is a single-process local pipeline:

```text
synthetic events → bounded FIFO buffer → batches → local event log → replay
```

It has typed events, repeatable generated input, a buffer that returns rejected events to their caller, and a checksummed append-only log with process-restart recovery. A successful append follows separate event-data and commit-marker syncs, subject to the [storage assumptions](docs/architecture/storage.md).

This is an early research prototype. Real collectors, network ingestion, a query engine, an API, and a UI remain future work. Separate transport simulations explore scheduling trade-offs; they are research tools, not the application's transport. Collector and storage strategy experiments are the next research focus.

## Try it

With a recent Rust toolchain, run the repeatable three-event demo:

```sh
cargo run -- 42 3
```

The demo shows a full buffer returning an event, draining a batch, and retrying. To write the same input to a fresh local log and replay it in a separate process:

```sh
cargo run -- write target/learning-events.log 42 3
cargo run -- replay target/learning-events.log
```

Repeating `write` with the same seed and count verifies the existing prefix and appends nothing after an ordinary process exit. Use a fresh path for a different workload; see the [storage guide](docs/architecture/storage.md) before retrying after a storage I/O error.

Run the application checks with `cargo test --locked`.

## Explore the project

- [Architecture documentation](docs/README.md) — how the implemented pieces fit together.
- [Current project state](docs/CURRENT.md) — progress, assumptions, and unresolved questions.
- [Learning path](docs/LEARNING_PATH.md) — follow the Rust concepts and system contracts step by step.
- [Experiments and evidence](docs/experiments/README.md) — workloads, measurements, model checks, and their limits.
- [Contributing](docs/CONTRIBUTING.md) — repository workflows and validation.
- [Long-term blueprint](docs/architecture.md) — the broader proposal and research agenda.
