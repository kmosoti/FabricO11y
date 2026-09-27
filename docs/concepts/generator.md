# Event generator

The [synthetic generator](../../src/generator.rs) turns a [workload config](../glossary.md) into a sequence of owned [events](event.md). It exists to give later pipeline stages repeatable input, not to simulate a production traffic distribution.

## Contract

- The config contains a `u64` seed and `u32` event count. A count of zero yields no events.
- Each call to `next` produces at most one event; exactly the configured count are produced if fully consumed. Earlier events are not retained by the generator.
- IDs start at 1. Event times start at a fixed Unix nanosecond value and increase by one millisecond. Observed time is ten milliseconds later.
- Every event is a `Gauge` for `request.duration` in `ms`, with the `service.name=checkout` attribute, one fixed source and resource, and a tenant in `1..=4`.
- [`fake`](https://docs.rs/fake/5.1.0/fake/) samples the tenant in `1..=4` and an integer in `0..1000` for the duration. Dividing that integer by four makes a value in `0..250` ms in exact quarter-millisecond steps. The named ChaCha8 RNG comes through `fake`'s `rand` re-export. Fabric maps the public `u64` seed into its 32-byte seed in little-endian order and keeps the RNG in the iterator.
- Replaying a seed requires the locked versions in [Cargo.lock](../../Cargo.lock). The [seed-42 test](../../tests/generator.rs) pins the first three values to expose an accidental sequence change. The workload is synthetic; its values and fixed service name are not evidence of production traffic.

The generator owns its small state. A returned `Event` owns its attribute vector and strings. The caller may move it into the [local buffer](buffer.md), keep it, or drop it. Buffering exists in memory; no durable delivery guarantee exists yet.

## Check it

Run `cargo run --offline -- 42 3` twice and compare the lines. Then run `cargo test --offline`; the [generator tests](../../tests/generator.rs) check replay, a pinned seed-42 sequence, seed variation, count boundaries, prefix stability, and the first event of a large stream. The [CLI tests](../../tests/cli.rs) check defaults and invalid settings. A sequence mismatch for the same config would falsify the repeatability contract.

See the [system path](../architecture/system.md), [Stage 2 lesson](../LEARNING_PATH.md), and [generator decision](../decisions/ADR-0004-use-fake-for-synthetic-values.md).
