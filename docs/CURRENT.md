# Current project state

## Active work

Target-host qualification (`milestone/target-qualification`): the [qualification runbook](qualification-runbook.md) runs 2 to 6 ran on the 12-CPU target host from binaries frozen at `e68d6ce`, and a [comparison with established systems](experiments/benchmarks/baseline-comparison-protocol.md) is registered. Merged: the [architecture foundation](milestones/architecture-foundation.md), [verification foundation](milestones/verification-foundation.md), [semantic kernels](milestones/semantic-kernels.md), [history qualification](milestones/history-qualification.md), [delivery and recovery qualification](milestones/delivery-recovery.md) and [Linux installation qualification](milestones/linux-installation.md). Later milestones are in the [roadmap](ROADMAP.md).

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
- The target-profile runs ran under WSL2 with systemd on a 12-CPU laptop, Debian 13 on ext4 with cgroup v2 ([host](experiments/benchmarks/data/target-qualification/alpha-q1-host.txt)); the earlier history, outage, stress and soak runs ran in a four-CPU Ubuntu 24.04 Firecracker VM on ext4.
- Batch identity for duplicate detection is the SHA-256 of the exact bytes; collisions are assumed infeasible.
- The layer gate sees crates, not modules: the adapters inside the two composition roots are protected only by tests, oracles and mutants; their decisions are kernels in the core.

## Unresolved questions

- When, if ever, to rename the `fabric-node` executable ([ADR-0017](decisions/ADR-0017-name-the-spindle-and-the-strand.md)).

## Known risks

- The soak and the running installation are not qualified (below); every qualification belongs to `e68d6ce`.
- One host runs server, simulator and harness in fleet tiers; CPU contention distorts p99.
- Server memory at 1,000 identities is now about 420 MiB where Segments seal, against about 48 MiB before sealing existed ([fleet run 02](experiments/benchmarks/fleet-run-02.md)); it is the same sealer working set.
- The server's sealer holds about ten times a journal file in memory while it builds a Segment, and the allocator keeps it: server RSS plateaus near 536 MiB after the first seal at 100 identities, and near 538 MiB on the target host ([soak run 01](experiments/benchmarks/soak-run-01.md), [soak run 02](experiments/benchmarks/soak-run-02.md)).
- An ineffective I/O controller on WSL must be reported, never counted as enforcement.

## Outstanding qualification

From the [capability ledger](QUALIFICATION.md#capability-ledger), on the target profile for `e68d6ce`: history query latency, freshness and the journal-versus-Segment comparison **qualified** ([history run 02](experiments/benchmarks/history-run-02.md)); outage and drain **qualified** ([outage run 02](experiments/benchmarks/outage-run-02.md)); burst, rejection and concurrent management **qualified** ([stress run 02](experiments/benchmarks/stress-run-02.md)); fleet delivery and control at 10, 100 and 1,000 identities **qualified** ([fleet run 02](experiments/benchmarks/fleet-run-02.md)); soak **failed** its RSS-growth gate again ([soak run 02](experiments/benchmarks/soak-run-02.md)). Running installation stays **inconclusive** ([installation acceptance run 01](experiments/formal/installation-acceptance-run-01.md)); the target-host rerun needs Docker and root and has not run. Release **not performed**, no tag.

## Next validation steps

- [x] History measurement under revision 2 on a four-CPU host ([milestone record](milestones/history-qualification.md)).
- [x] History revision 1 on the 12-CPU target host ([history run 02](experiments/benchmarks/history-run-02.md)).
- [x] Outage, stress (revision 2) and soak run on a four-CPU host ([milestone record](milestones/delivery-recovery.md)).
- [x] Outage, stress (revision 1) and soak on the target host ([outage run 02](experiments/benchmarks/outage-run-02.md), [stress run 02](experiments/benchmarks/stress-run-02.md), [soak run 02](experiments/benchmarks/soak-run-02.md)).
- [ ] Bound the sealer's working set, then rerun the registered soak unchanged ([soak run 01](experiments/benchmarks/soak-run-01.md)).
- [x] Running-installation acceptance in a container ([milestone record](milestones/linux-installation.md)): inconclusive.
- [ ] Running-installation acceptance on a host with the unified cgroup hierarchy (decides `MemoryHigh` enforcement).
