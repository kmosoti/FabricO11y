# ADR-0003: Put domain semantics in a pure Rust core

Status: Accepted

## Context

Replay, ordering, corrections, provenance, and frontier meaning must stay identical across storage,
CLI, daemon, and language adapters.

## Decision

Place typed contracts and ordering primitives in `fabric-schema` and `fabric-time`, with
deterministic reducers in `fabric-core`. These crates perform no filesystem, database, network,
process, compression, async-runtime, or PyO3 effects. Complete-history replay uses ordered
collections and declarative correction aggregation so equivalent histories yield equivalent state.

## Consequences

Effectful adapters pass typed records inward and receive typed state or errors. Core behavior is
cheap to test, while adapters must explicitly manage clocks, persistence, and transactions.

## Rejected alternatives

- Reimplement reducers in each adapter: rejected because parity would be unprovable.
- Let storage order define semantic order: rejected because physical arrival is not causal truth.

## Supersession

Supersedes no prior decision.
