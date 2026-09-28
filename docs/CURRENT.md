# Current project state

## Active work

Milestone **semantic kernels** ([record](milestones/semantic-kernels.md)): control, query, retention and collection decisions as `no_std` kernels, retention as a use case over a port, and the Spindle's Linux reads in `fabric-adapter-linux`. Merged: the [architecture foundation](milestones/architecture-foundation.md) and the [verification foundation](milestones/verification-foundation.md). Later milestones are in the [roadmap](ROADMAP.md).

## Implemented

- **Spindle** (`fabric-node`): bounded Linux host metrics and selected log files as OTLP inside version-one Batches, a durable `FAB1` Spool with ACK cursor and whole-file reclaim, visible collection gaps, SIGTERM-safe cycles, remote configuration with last-valid fallback ([Spindle view](architecture/spindle.md)).
- **Delivery**: TLS, bearer credentials bound to one Spindle, one Batch in flight per Strand, ACK only after the server's grouped two-sync commit ([delivery view](architecture/delivery.md)). The decision is a pure kernel in `fabric-core`, orchestrated by `fabric-app` over the `DurableJournal` and `Clock` ports.
- **Fabric Server**: journal with replayed Strand and binding state, stream checkpoint before reclaim, Zstd Parquet Segments sealed off the commit path, retention by age and bytes, log/metric/rate queries with completeness, freshness, gaps and snapshot-bound pages ([retained history](architecture/retained-history.md)), central control ([control](architecture/control-plane.md)).
- **Packaging**: systemd units, slice, sysusers file and a reproducible `.deb` ([deployment](architecture/deployment.md)).
- **Checks**: layer and purity gates with fixture negative controls; independent Python delivery, query and rate oracles; semantic-mutant registry; TLA+ delivery model; fault harness; check registry with receipts ([verification strategy](formal/verification-strategy.md)).
- **Legacy and research**: the FOL2 [demonstration](architecture/fol2-demo.md) remains supported; research packages under `tools/` stay outside the product.

## Architecture currently affected

Crates: `fabric-core` (core), `fabric-ports` (ports), `fabric-app` (app), `fabric-frame` (adapter support), `fabric-adapter-linux` (adapter), `fabric-server` and the root package (composition roots that still contain their adapters). See the [system view](architecture/system.md).

## Current assumptions

- ACKs rely on successful sync calls being honored by the filesystem; physical power loss is untested.
- WSL2 on ext4 is the only exercised environment.
- Batch identity for duplicate detection is the SHA-256 of the exact bytes; collisions are assumed infeasible.
- The layer gate sees crates, not modules: the adapters inside the two composition roots are protected only by tests, oracles and mutants; their decisions are kernels in the core.

## Unresolved questions

- How to map fault-harness transcripts onto the TLA+ delivery actions.
- When, if ever, to rename the `fabric-node` executable ([ADR-0017](decisions/ADR-0017-name-the-spindle-and-the-strand.md)).

## Known risks

- Qualification is outstanding (below); earlier passing measurements belong to earlier revisions.
- One host runs server, simulator and harness in fleet tiers; CPU contention distorts p99.
- An ineffective I/O controller on WSL must be reported, never counted as enforcement.

## Outstanding qualification

From the [capability ledger](QUALIFICATION.md#capability-ledger): history query latency, freshness and journal-versus-Segment comparison **not run**; outage and drain **interrupted** (first trial stopped mid-run, no result); stress/burst **not run**; soak **not run** and unregistered; running installation **not run** (needs root on a disposable host); release **not performed**, no tag. This milestone reran bounded fault runs and oracle suites only; it is not qualification.

## Next validation steps

- [ ] History qualification under the registered protocol.
