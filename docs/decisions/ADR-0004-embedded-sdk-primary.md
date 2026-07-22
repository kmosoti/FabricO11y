# ADR-0004: Make the embedded SDK primary and the daemon optional

Status: Accepted

## Context

Initial users need local Rust and Python embedding. A daemon-first design would add transport and
deployment semantics before the evidence kernel is stable and risk separate implementations.

## Decision

Expose one coarse `fabric-sdk::Engine` for admission, sealing, replay, validation, recovery, and
catalog lookup. `fabric-py` wraps that engine with PyO3, exchanging canonical JSON strings; the
Python package performs DTO conversion only. A daemon may later wrap the same facade.

The Rust engine has single-owner mutable operations and does not promise simultaneous processes on
one data root. The PyO3 class is unsendable and its calls remain serialized by Python ownership.
Every boundary call catches Rust unwinding and converts it to `FabricError` with category
`panic_contained`; ordinary failures preserve stable SDK categories in typed Python exceptions.

## Consequences

Rust and Python share reducers, storage, and recovery behavior. Wheel tests can compare Python
output exactly with the Rust CLI. Callers needing concurrent writers must add external
serialization or wait for a separately tested concurrency contract.

## Rejected alternatives

- Python reducers over a Rust storage layer: rejected because it creates a second semantic core.
- Require a daemon for Python: rejected because embedded use is the primary deployment posture.
- Expose SQLite and segment handles publicly: rejected because storage internals are not the SDK
  compatibility boundary.

## Supersession

Supersedes no prior decision.
