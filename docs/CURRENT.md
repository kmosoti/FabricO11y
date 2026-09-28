# Current project state

## Active work

Milestone **history qualification** ([record](milestones/history-qualification.md)): the registered history protocol under [revision 2](experiments/benchmarks/history-protocol-r2.md) on a four-CPU host, and [ADR-0020](decisions/ADR-0020-store-sealed-history-as-parquet-segments.md) for the Segment format. Merged: the [architecture foundation](milestones/architecture-foundation.md), the [verification foundation](milestones/verification-foundation.md) and the [semantic kernels](milestones/semantic-kernels.md). Later milestones are in the [roadmap](ROADMAP.md).

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
- WSL2 on ext4 was the environment of the earlier measurements; the history measurements ran in a four-CPU Ubuntu 24.04 Firecracker VM on ext4. Neither is the target profile.
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

From the [capability ledger](QUALIFICATION.md#capability-ledger): history query latency, freshness and journal-versus-Segment comparison **measured and passing** under [revision 2](experiments/benchmarks/history-protocol-r2.md) on a four-CPU host ([history run 01](experiments/benchmarks/history-run-01.md)), not qualified on the target profile; outage and drain **interrupted** (first trial stopped mid-run, no result); stress/burst **not run**; soak **not run** and unregistered; running installation **not run** (needs root on a disposable host); release **not performed**, no tag. No capability is qualified on the target profile.

## Next validation steps

- [x] History measurement under revision 2 on a four-CPU host ([milestone record](milestones/history-qualification.md)).
- [ ] History revision 1 on the 12-CPU target host.
