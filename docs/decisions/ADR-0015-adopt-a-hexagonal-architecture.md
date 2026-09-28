# ADR-0015: Adopt a hexagonal architecture with a checked dependency rule

## Status

Accepted on 2026-09-28. The owner authorized this direction in the architecture-foundation milestone instruction. The migration of existing code into the layers is incremental; [the milestone record](../milestones/architecture-foundation.md) lists what moved and what did not.

## Context

At the base (`9b3a2b4`) the root package held the event model, the FOL2 log, the node runtime, the frame log and the node binaries, and `fabric-server` depended on the whole root package to reach the frame log and the batch envelope. Delivery decisions, journal writes, clock reads and HTTP answers sat in the same functions. Nothing prevented a dependency in any direction, so a diagram was the only statement of the architecture.

## Decision

Domain semantics point inward; effects point outward. The workspace has these layers:

| Layer | Owns | May depend on (workspace crates) |
| --- | --- | --- |
| core | pure domain decisions: identities, Strand sequencing, delivery and deduplication, and later control, query, retention and collection rules | core only |
| ports | effect contracts that application use cases need, such as the durable journal and the clock | core |
| app | use-case orchestration: calls core decisions and ports, never a concrete adapter | core, ports |
| adapter-support | infrastructure shared by several adapters, such as the `FAB1` frame log and the batch envelope codec | core |
| adapter | Linux, filesystem, HTTP/TLS, Parquet and clock implementations of ports | core, ports, adapter-support |
| composition-root | binaries and the libraries that wire adapters into use cases | everything above, not another composition root |
| tooling | repository checks (`xtask`) | none |

[`docs/architecture/layers.json`](../architecture/layers.json) assigns every workspace member to a layer and lists documented exceptions. `cargo xtask check-layers` reads declared metadata (`cargo metadata --no-deps`: every normal, build and development dependency, optional or not, for any target, under any rename) and resolved metadata (edges that leave the workspace and come back). Any forbidden edge fails with a stable category.

Crates follow real dependency boundaries, not nouns. Ports exist only where an effect crosses a semantically meaningful boundary; there are no generic repository traits, service locators, plugin systems or dependency-injection frameworks. A crate that still mixes adapters with wiring is classified as a composition root and its split is listed as remaining work rather than hidden.

## Alternatives considered

- Module visibility inside one package. Rust privacy cannot stop a module from importing another public module in the same crate, and nothing checks direction.
- A crate per concept (`fabric-strand`, `fabric-spindle-core`). More manifests and no additional enforceable boundary.
- `cargo-deny` bans or an external architecture tool. They check registry crates well but do not express a layer relation between workspace members; the repository rule is a few hundred lines over `cargo metadata`.

## Evidence

The gate ships with fixture workspaces in [`xtask/fixtures`](../../xtask/fixtures/workspace/Cargo.toml) and tests in [`xtask/tests/gates.rs`](../../xtask/tests/gates.rs): the valid workspace passes; core to adapter, core to app, ports to adapter, app to adapter, renamed, optional feature-activated, target-specific, build, development and transitive forbidden edges each fail with `LAYER_FORBIDDEN_EDGE` or `LAYER_FORBIDDEN_TRANSITIVE_EDGE`; an unassigned member and a stale policy entry fail; a manifest that cannot be read is an error, never a pass. Command results are in the milestone record.

## Consequences

Easier: a new dependency in the wrong direction fails CI; the core can be tested without I/O; application use cases can be tested with in-memory ports. Harder: types shared across layers must live in the inner layer; a composition root that still contains adapters is only as disciplined as its own modules until it is split. New constraint: every new workspace member needs a layer, and every exception needs a written reason.

## Validation

The gate is falsified if any forbidden edge kind in the fixture list passes, or if the valid fixture fails. The architecture is falsified if core semantics are found in an adapter or composition root and cannot be moved inward without an effect.

## Related

[ADR-0016](ADR-0016-keep-a-pure-semantic-core.md), [ADR-0012](ADR-0012-add-a-server-crate-in-a-workspace.md), [system view](../architecture/system.md).
