# ADR-0012: Add the Fabric Server as a workspace crate with a synchronous node client

## Status

Accepted on 2026-09-27 for alpha phase 2 (plan decision D3).

## Context

Phase 2 needs a TLS server that accepts batches from many nodes and commits them durably, and a node-side sender. [ADR-0001](ADR-0001-keep-domain-independent.md) keeps the domain types free of runtime and web frameworks. The native node has a 64 MiB peak RSS gate and today runs one synchronous loop. The research packages under `tools/` are separate Cargo packages that depend on the root by path and declare no workspace of their own.

## Decision

- Make the repository root a Cargo workspace with one new member, `crates/fabric-server`, and `exclude = ["tools"]` so the research packages keep their own lockfiles and commands. The root package is a member because it declares the workspace; listing `"."` in `members` would override the exclusion.
- The root package keeps the domain types, FOL2, the demo, the `alpha` module, and the `fabric-node` and `fabricctl` binaries. The shared `FAB1` frame code lives in the root library so node and server use one implementation.
- The server uses tokio, axum and axum-server with rustls. Durable commits run on one dedicated thread, not on async tasks.
- The node and `fabricctl` use the synchronous ureq client with rustls. The node gains no async runtime.
- rustls uses the `ring` provider everywhere, which avoids a cmake-built crypto backend. Every version is pinned exactly and was already in the local cargo cache.
- No SQLite and no certificate-generation crate. TLS material is operator-provided PEM; tests and qualification generate throwaway PEM with the `openssl` command under ignored `target/`.

## Alternatives considered

- Put the server in the root package as a third binary. Simpler manifest, but tokio and axum would become dependencies of the domain library's package and every build.
- An async node client (reqwest). It adds a runtime and threads to the RSS-gated process for one request in flight.
- SQLite for dedup and control state. The dedup state is small and rebuilt by journal replay; control state fits one atomically replaced file.

## Evidence

This is a structural decision. The 64 MiB node gate and the 2 GiB server gate are measured in phase 2 step 2.6 and phase 5.

## Consequences

The server crate can depend on infrastructure freely; the root library stays free of tokio and axum. The workspace shares one `Cargo.lock`. CI builds both members. Research packages are unaffected.

## Validation

`cargo tree -p fabric_o11y` shows no tokio, axum or hyper. Research package commands still run from their manifests. Node RSS stays under the gate with the sender enabled.

## Related

[Completion plan](../ALPHA-PLAN.md), [ADR-0013](ADR-0013-deliver-batches-in-order-with-bounded-dedup.md), [alpha contract](../ALPHA.md).
