# ADR-0004: Use `fake` for synthetic workload values

## Status

Accepted for the current local learning workload. This is a generator choice, not a claim that its output resembles production telemetry or is the fastest available option.

## Context

The Stage 2 generator had a hand-written linear congruential recurrence. The project needs reproducible input for tests and experiments, while the event model's IDs, timestamps, ownership, and payload shape must remain explicit. A crates.io lookup on 2026-09-26 found no Rust crate named `synthdatagen`; the similarly named Python package is not a Rust dependency.

## Decision

Use [`fake` 5.1.0](https://docs.rs/fake/5.1.0/fake/) with its default features disabled. Its `Fake::fake_with_rng` range samplers provide tenant and duration values. Use the named `ChaCha8Rng` exposed through `fake`'s `rand` re-export, with the public `u64` seed placed in the first eight bytes of a 32-byte little-endian seed. Keep a small [Fabric adapter](../../src/generator.rs) to assign sequential IDs, fixed synthetic times, the source and resource IDs, attribute key, and Gauge payload. Pin the crate version in [Cargo.toml](../../Cargo.toml) and dependency resolution in [Cargo.lock](../../Cargo.lock).

## Alternatives considered

- **Keep the hand-written recurrence:** It has no dependency and remains the [archived benchmark input](../experiments/benchmarks/fixtures/stage3_lcg_generator.rs). Maintaining an ad hoc generator does not teach use of a reusable data-generation library, and its distribution and update policy would remain ours to own.
- **Use a domain-specific event generator crate:** For example, [`spate-datagen`](https://docs.rs/spate-datagen/0.2.0/spate_datagen/) provides a fixed storefront dataset, rather than configurable Fabric telemetry. Mapping its records into `Event` would add a larger adapter. Such a source may be useful when the project defines a realistic traffic scenario; none is defined yet.
- **Use `fake` to derive arbitrary `Event` values:** The domain types have constraints that general fake values cannot infer, including sequential IDs, positive tenant range, ordered synthetic timestamps, and finite Gauge values. Explicit construction keeps those rules reviewable.

## Evidence

- [`fake`'s API](https://docs.rs/fake/5.1.0/fake/) documents seeded `fake_with_rng` generation and re-exports its `rand` dependency. The [Rand reproducibility guide](https://rust-random.github.io/book/crate-reprod.html) recommends a named ChaCha RNG for reproducible streams rather than `StdRng`.
- [Generator tests](../../tests/generator.rs) check replay, a pinned seed-42 sequence, prefix stability, count bounds, and seed variation. [CLI tests](../../tests/cli.rs) and [buffer tests](../../tests/buffer.rs) check the downstream path after the sequence change.
- The original [Stage 3 batch-size probe](../experiments/benchmarks/batch-size-stage3.md) used the old generator and is retained as historical evidence. The [generator library ablation](../experiments/benchmarks/generator-library-stage3.md) compares both variants on the same pipeline; its measured medians are 4.6–11.8% higher with `fake`, with overlapping trial ranges.

## Consequences

Fabric no longer maintains a pseudorandom recurrence. The adapter remains because a library cannot know Fabric's event semantics. The Rust package now has a third-party dependency and a different seed-42 sequence. The fixed service, source, resource, and Gauge shape remain. Reproducibility across dependency changes is checked by the golden test; a dependency update may intentionally require a new baseline and benchmark. The dependency is used for synthetic input, outside the event type definitions.

## Validation

Run `cargo test --offline`, `cargo clippy --offline --all-targets -- -D warnings`, and the [current probe](../experiments/benchmarks/generator-library-stage3.md). A failure to replay the same config, a value outside the declared ranges, a changed event count, or a regression in the buffer path would falsify this version's contract. A representative production workload and a performance budget are still needed before claiming that this generator is suitable for production load testing.

## Related

- [Generator concept](../concepts/generator.md)
- [System architecture](../architecture/system.md)
- [ADR-0001: Event domain boundary](ADR-0001-keep-domain-independent.md)
