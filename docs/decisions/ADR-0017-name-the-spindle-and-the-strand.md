# ADR-0017: Name the Spindle and the Strand, and nothing else thematically

## Status

Accepted on 2026-09-28 under the owner's architecture-foundation authorization.

## Context

The code and documents used "node" for at least four things: the host-resident collection process, a simulated identity in the fleet simulator, an entry in server control state, and generic network nodes. The ordered lineage that deduplication and gap detection work on was called a "stream", which collides with log streams, TCP streams and Rust iterators. Thematic names are tempting in a project called Fabric, but most of them would hide meaning rather than add precision.

## Decision

Two themed terms are canonical:

- **Spindle**: the host-resident FabricO11y collection and runtime role. A Spindle observes one host, reads configured sources, builds Batches, keeps local collection state, retains unacknowledged Batches in its Spool, applies validated collection configuration and delivers Batches to Fabric Server. It is not a synonym for every process, worker, collector adapter or remote machine.
- **Strand**: one ordered telemetry lineage produced by one Spindle generation, `StrandId = (SpindleId, generation)`. A Strand holds monotonically sequenced Batches and is the scope of sequence ordering, duplicate-retry identity, gap detection, deduplication and acknowledged progress. A new generation starts a new Strand. A Strand is not a connection, session, thread, log file, journal file, Parquet Segment or query result.

Everything else keeps a functional name: Batch, Spool, Delivery, Server, Journal, Segment, Retention, Query, Control, Port, Adapter, Core, Application. Names such as Shuttle, Bolt, Lens, Selvedge, Loom or Warp are rejected; Loom and Warp belong to another project. Crate names stay boring (`fabric-core`, `fabric-ports`, `fabric-app`, `fabric-frame`, `fabric-server`); there is no `fabric-strand` or `fabric-spindle-core`.

Compatibility boundary: persisted and wire names do not change. The envelope field `node_id` (field 2), the checkpoint's stream tuples, HTTP routes (`/v1/admin/nodes`), JSON keys, configuration files and command output keep their spelling. `SpindleId` and `StrandId` are validated core types that adapters convert to and from those representations.

Executable name: `fabric-node` stays for now. Renaming it to `fabric-spindle` touches the systemd unit `fabrico11y-node.service` and its `ExecStart`, the package build script, maintainer scripts, four qualification scripts, the registered protocols' commands and every operator instruction, and would change what the static packaging evidence describes. None of that improves correctness, so the rename waits for a release that states a compatibility plan. The architectural role is the Spindle; the binary that runs it is `fabric-node`.

## Alternatives considered

- Keep "node" and "stream". Ambiguous in exactly the places where correctness depends on scope.
- A full themed vocabulary. Every added metaphor is one more mapping a reader must learn, with no gain in precision.
- Rename the executable now. Pure churn against installation and qualification artifacts.

## Evidence

The type renames are compile-checked: `Node` became `Spindle` in `src/spindle/runtime.rs`, `Journal` became `Spool`, and `fabric-core` gained `SpindleId`, `StrandId` and `next_sequence`. Workspace tests pass unchanged; no byte format changed.

## Consequences

Easier: the delivery rule, the glossary and the verification matrix can say "Strand" and mean one thing. Harder: readers meet `node_id` on the wire and `SpindleId` in the core; the glossary records the mapping. New constraint: a further themed term needs its own ADR.

## Validation

Falsified if "Strand" is used for anything other than `(SpindleId, generation)` lineage, or "Spindle" for anything other than the host runtime role, in current documents or code.

## Related

[Glossary](../glossary.md), [ADR-0013](ADR-0013-deliver-batches-in-order-with-bounded-dedup.md), [ADR-0015](ADR-0015-adopt-a-hexagonal-architecture.md).
