# ADR-0001: Keep the event domain independent of infrastructure

## Status

Accepted for the initial learning prototype. This records the existing library contract and blueprint principle; it does not select a storage representation or assert a performance result.

## Context

The project intends to learn how observability mechanisms behave by comparing them under explicit contracts. Allowing a storage or wire format to define event meaning would make those comparisons harder to interpret.

## Decision

Describe event meaning with Rust domain types whose definition does not depend on a transport, persistence library, async runtime, or solver. Keep synthetic generation, buffering, and output in separate modules or the executable. The current package has library and binary targets; separate crates are not required for this initial boundary. Stage 1 constructed one example directly in the executable. Stage 2 moved that construction into `generator`; Stage 3 added `buffer`. Neither module changes the event type definitions.

## Alternatives considered

The blueprint names protocol, storage, and universal JSON representations as possible mechanisms and argues for separating them from semantics. No implemented comparison or prior decision history is present. This record makes no claim that those alternatives have been benchmarked or rejected for future adapters.

## Evidence

- [src/lib.rs](../../src/lib.rs) defines event types independently of the [generator](../../src/generator.rs) and [buffer](../../src/buffer.rs) modules.
- The event type definitions in [src/lib.rs](../../src/lib.rs) do not use the `fake` dependency declared in [Cargo.toml](../../Cargo.toml); [src/generator.rs](../../src/generator.rs) uses it to construct test input, and [src/main.rs](../../src/main.rs) orchestrates the local example.
- The [original blueprint](../architecture.md), section 1.1, states the intended separation of semantics and mechanisms.

## Consequences

The event's semantic roles can be discussed before infrastructure is selected. Future adapters will need explicit mappings into and out of the domain. A single crate does not mechanically enforce every future module dependency, so boundary changes require review.

This decision leaves ID width, attribute layout, schema rules, encoding, and persistence open. The current `u64` IDs are an implementation fact, not a measured recommendation.

## Validation

Inspect the library imports and run `cargo metadata --offline --no-deps --format-version 1` to inspect dependencies. If a later implementation ties the event definition to a backend or runtime, either restore this boundary or revise the decision explicitly. A successful build alone does not prove the boundary will survive future changes.

## Related

- [System architecture](../architecture/system.md)
- [Event concept](../concepts/event.md)
- [Experiment status](../experiments/README.md)
