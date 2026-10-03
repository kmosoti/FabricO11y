# Current project state

## Active work

Milestone **bounded sealer** ([record](milestones/bounded-sealer.md)): design accepted, implementation not started. The sealer will build each Segment by external merge sort, so its memory stops growing with the journal file ([ADR-0022](decisions/ADR-0022-build-segments-by-external-merge-sort.md), [sealer view](architecture/sealer.md)). Its acceptance protocol is registered; the comparison that chose the algorithm is an exploratory [study](experiments/benchmarks/sealer-study-run-01.md). Merged: the [architecture foundation](milestones/architecture-foundation.md), [verification foundation](milestones/verification-foundation.md), [semantic kernels](milestones/semantic-kernels.md), [history qualification](milestones/history-qualification.md), [delivery and recovery qualification](milestones/delivery-recovery.md), [Linux installation qualification](milestones/linux-installation.md) and [verification tooling](milestones/verification-tooling.md). The remaining target-host runs are in the [qualification runbook](qualification-runbook.md).

Research in step with it ([design consolidation](research/design-consolidation.md) collapses it into decisions with their gates; [experiment durability](research/experiment-durability.md) says what of the research loop is reproducible from a clean clone): the [storage direction](research/storage-direction.md) (one record spine for logs, metrics and traces; measured on real text in [storage layout run 01](experiments/benchmarks/storage-layout-run-01.md)), the proposed [Observation record and FOB1 codec](decisions/ADR-0023-define-an-observation-record-with-a-canonical-encoding.md) ([fabric-observation](../crates/fabric-observation/src/lib.rs), tested and fuzzed, wired to nothing), the [executable query specification](formal/query-semantics.md) in the core with its theorems as properties and bounded proofs, and the query prototypes of ledger [L-04](research/ledger.md#l-04-the-threshold-algorithm-over-source-bounds) and [L-05](research/ledger.md#l-05-budget-bounded-answers-with-sound-partial-completeness), with the [optimality bounds](research/optimality-bounds.md) measuring each against its floor: the prototypes within 2× of every measured bound on the fixtures, the tail answered from canonical blocks at 0.26 to 0.28 of stock on its slowest shape (0.08 with a trigram filter per block), and a node-presence set shown to have nothing to skip below fleet scale; a trigram filter per row group (ledger [L-25](research/ledger.md#l-25-a-text-filter-per-row-group)) takes a no-match text search on Segments from about 120 ms to under 10 and awaits a product-contract decision. All of it is research: no prototype is wired into the product.

## Implemented

- **Spindle** (`fabric-node`): bounded Linux host metrics and selected log files as OTLP inside version-one Batches, a durable `FAB1` Spool with ACK cursor and whole-file reclaim, visible collection gaps, SIGTERM-safe cycles, remote configuration with last-valid fallback ([Spindle view](architecture/spindle.md)). It meters its own output as `fabric.spindle.*` metrics and can cap its delivery rate (`max_output_bytes_per_s`); a pass that leaves a log backlog is followed by another once delivery has caught up, about 3.6 MB/s of real log text per node on this machine, set by one Batch in flight and the server's 50 ms commit window ([spindle run 01](experiments/benchmarks/spindle-run-01.md)).
- **Delivery**: TLS, bearer credentials bound to one Spindle, one Batch in flight per Strand, ACK only after the server's grouped two-sync commit ([delivery view](architecture/delivery.md)). The decision is a pure kernel in `fabric-core`, orchestrated by `fabric-app` over the `DurableJournal` and `Clock` ports.
- **Traces** ([ADR-0025](decisions/ADR-0025-carry-traces-as-a-third-signal.md)): local applications export spans to the Spindle's loopback OTLP/HTTP endpoint, answered after the Spool commit; Batches carry them in field 9; the server stores them in `spans.parquet` with a trace-ID filter and answers `{"kind":"spans"}` by window, trace ID, name and node in both query plans, graded by the independent oracle.
- **Throughput on this machine** (4 CPUs): the server seals journal files on `seal_workers` threads and sustained 48, 75 and 81 MB/s of real-text ingest with 1, 2 and 3 workers, bound by sealing CPU, relying on back-pressure when the journal filled ([ingest run 01](experiments/benchmarks/ingest-run-01.md)); how each component would scale with more resources, to 500 hosts at 500 MB/s, is in the [scaling design](research/scaling-design.md).
- **Fabric Server**: journal with replayed Strand and binding state, stream checkpoint before reclaim, Zstd Parquet Segments sealed off the commit path, retention by age and bytes, log/metric/rate queries with completeness, freshness, gaps and snapshot-bound pages ([retained history](architecture/retained-history.md)), read by the scan plan or, with `query_plan=walk`, by the key-ordered walk over a tail index and cached Segment bounds (ADR-0024 part 1; identical answers; 21× faster on a random query set over a real-text tail in [query walk run 01](experiments/benchmarks/query-walk-run-01.md); default `scan` until the history protocol and soak are re-run); Segments carry a seal-time trigram filter per logs row group that the walk uses to skip groups without a needle (ADR-0024 part 2, contract amended; a no-match text search over 64 real-text Segments 95 to 131 ms to under 5 ms in [text filter run 01](experiments/benchmarks/text-filter-run-01.md)); and the walk reads the unsealed tail as FOB1 blocks derived from the journal in memory (ADR-0024 part 3, ADR-0023 accepted for this use; the slowest tail shape 614 to 28 ms in [block tail run 01](experiments/benchmarks/block-tail-run-01.md)), central control ([control](architecture/control-plane.md)).
- **Packaging**: systemd units, slice, sysusers file and a reproducible `.deb` ([deployment](architecture/deployment.md)). The package targets Debian-family distributions (glibc 2.34+, systemd 249+): dependencies from `dpkg-shlibdeps`, a glibc-ceiling check, the build host's architecture.
- **Checks**: layer and purity gates with fixture negative controls; independent Python delivery, query and rate oracles; semantic-mutant registry; TLA+ delivery model with trace validation; fault harness; property tests, Kani proofs of the core, fuzzing, turmoil network simulation and a cargo-deny dependency policy; check registry with receipts ([verification strategy](formal/verification-strategy.md)).
- **Legacy and research**: the FOL2 [demonstration](architecture/fol2-demo.md) remains supported; research packages under `tools/` stay outside the product.

## Architecture currently affected

Crates: `fabric-core` (core), `fabric-ports` (ports), `fabric-app` (app), `fabric-frame` (adapter support), `fabric-adapter-linux` (adapter), `fabric-server` and the root package (composition roots that still contain their adapters). See the [system view](architecture/system.md).

## Current assumptions

- ACKs rely on successful sync calls being honored by the filesystem; physical power loss is untested.
- WSL2 on ext4 was the environment of the earlier measurements; the history measurements ran in a four-CPU Ubuntu 24.04 Firecracker VM on ext4. Neither is the target profile.
- Batch identity for duplicate detection is the SHA-256 of the exact bytes; collisions are assumed infeasible.
- The layer gate sees crates, not modules: the adapters inside the two composition roots are protected only by tests, oracles and mutants; their decisions are kernels in the core.

## Unresolved questions

- When, if ever, to rename the `fabric-node` executable ([ADR-0017](decisions/ADR-0017-name-the-spindle-and-the-strand.md)).

## Known risks

- Qualification is outstanding (below); earlier passing measurements belong to earlier revisions.
- One host runs server, simulator and harness in fleet tiers; CPU contention distorts p99.
- The server's sealer holds about ten times a journal file in memory while it builds a Segment, and the allocator keeps it: server RSS plateaus near 536 MiB after the first seal at 100 identities ([soak run 01](experiments/benchmarks/soak-run-01.md)). [ADR-0022](decisions/ADR-0022-build-segments-by-external-merge-sort.md) accepts a bounded replacement; it is not implemented.
- An ineffective I/O controller on WSL must be reported, never counted as enforcement.

## Outstanding qualification

From the [capability ledger](QUALIFICATION.md#capability-ledger): history query latency, freshness and journal-versus-Segment comparison **measured and passing** under [revision 2](experiments/benchmarks/history-protocol-r2.md) on a four-CPU host ([history run 01](experiments/benchmarks/history-run-01.md)), not qualified on the target profile; outage and drain **measured and passing** ([outage run 01](experiments/benchmarks/outage-run-01.md)); burst, rejection and concurrent management **measured and passing** under [stress revision 2](experiments/benchmarks/stress-protocol-r2.md) ([stress run 01](experiments/benchmarks/stress-run-01.md)); soak **failed** its RSS-growth gate ([soak run 01](experiments/benchmarks/soak-run-01.md)); running installation **inconclusive** ([installation acceptance run 01](experiments/formal/installation-acceptance-run-01.md): every check but `MemoryHigh` enforcement passed in a Debian 13 container on a legacy cgroup hierarchy); release **not performed**, no tag. No capability is qualified on the target profile.

## Next validation steps

- [x] History measurement under revision 2 on a four-CPU host ([milestone record](milestones/history-qualification.md)).
- [ ] History revision 1 on the 12-CPU target host.
- [x] Outage, stress (revision 2) and soak run on a four-CPU host ([milestone record](milestones/delivery-recovery.md)).
- [ ] Bound the sealer's working set, then rerun the registered soak unchanged ([soak run 01](experiments/benchmarks/soak-run-01.md); [bounded-sealer milestone](milestones/bounded-sealer.md)).
- [x] Running-installation acceptance in a container ([milestone record](milestones/linux-installation.md)): inconclusive.
- [ ] Running-installation acceptance on a host with the unified cgroup hierarchy (decides `MemoryHigh` enforcement).
