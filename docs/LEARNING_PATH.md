# Learning path

## Rust mechanisms in the running system

| Mechanism | Contract and trade-off | Source or evidence |
| --- | --- | --- |
| Checked UI epochs, normalized string capacity and integer bucket boundaries | Late results cannot enter a new account/query; retained allocation follows payload bounds; sorted timestamps allow O(P) boundary divisions rather than O(N), preserving extrema and gaps. Browser work remains separate from server custody. | [UI model](../crates/fabric-ui/src/model.rs), [counterexamples](../crates/fabric-ui/tests/model.rs), [derivation](architecture/console-algorithms.md) |
| Captured authority and publication-time validation | Rust ownership keeps a request's captured authority valid as data; it cannot make that authority current after logout, expiry or policy change. Recheck before releasing a read, and serialize mutation authorization with durable intent/outcome publication. | [Access state](../crates/fabric-server/src/access.rs), [barrier regressions](../crates/fabric-server/src/console_tests.rs), [identity boundary](architecture/identity-access.md) |
| Checked arithmetic before effects | Sequence exhaustion must refuse before journal writes or cursor movement. | [Spool](../src/spindle/spool.rs), [invariant audit](experiments/formal/invariant-consolidation.md) |
| Explicit poisoned state | A failed directory sync after rename makes publication uncertain; an in-memory rollback cannot restore authorization. | [Control](../crates/fabric-server/src/control.rs) |
| `Drop` and process ownership | Graceful stop, kill/reap and parent-death signaling cover different lifecycle failures; queried diagnostics still need an independent delivery check. | [Companion](../crates/fabric-server/src/companion.rs), [ADR-0026](decisions/ADR-0026-launch-a-dedicated-spindle-with-each-server.md), [native smoke](../tools/self_observation_smoke.py) |
| `File` versus `PathBuf` ownership | A path can name a different journal after rotation. Bounded identity reads preserve snapshot bytes without pinning every file. | [Tail](../crates/fabric-server/src/tail.rs), [regressions](../crates/fabric-server/src/tail_identity_tests.rs) |
| `Vec::clear` and payload lifetime | Fewer live allocations need not lower RSS; earlier predicates can regress other query populations. | [Native ownership](experiments/benchmarks/native-frontier-findings.md), [workload sweep](experiments/benchmarks/cross-system-sweep-findings.md) |
| Borrowed records and higher-ranked callbacks | Storage owns Arrow buffers; a callback consumes views without retaining them. Exact digest checks still apply to excluded records. | [Range evidence](experiments/benchmarks/catalog-range-evidence-findings.md) |
| Owned permits, worker joins and `Cow` | Cancellation does not end blocking work. Permits follow that work; snapshot metadata is borrowed only when fully covered. | [Transition investigation](experiments/benchmarks/catalog-transition-ownership-findings.md), [source dissections](research/cross-system-source-synthesis.md) |

This project is deliberately split into short stages. Finish one stage, run it, and be able to explain its contract before moving on. The longer-term architecture is in [`architecture.md`](architecture.md); this page is the route through it.

Read [current project state](CURRENT.md) for what actually exists and [system architecture](architecture/system.md) for its boundaries. Stage 4 is a checked target model, Stage 5 has a local log and replay path, Stage 6 has its first measured baseline, and Stage 7 remains planned work.

## Stage 1 — Describe an observation

**Status: complete for this first version.** The Rust library defines distinct ID and time types plus an `Event`. Stage 2 uses them to make repeated examples.

Questions to answer while reading:

- Why is `EventId` a different type from `SourceId` if both contain an integer?
- Why do event time and observed time have separate types?
- What meaning does an event keep regardless of how it is stored?

Read [the event model](../src/lib.rs) and [the event concept](concepts/event.md). Try changing a field in the generator and running `cargo test`. Avoid optimizing this data model before we have a workload to measure.

## Stage 2 — Make repeatable input

**Status: implemented as a first synthetic workload.** [The generator](../src/generator.rs) accepts a `u64` seed and `u32` event count. It yields owned Gauge events one at a time. The [`fake` crate](https://docs.rs/fake/5.1.0/fake/) supplies seeded range values through a named ChaCha8 RNG; a small adapter assigns Fabric's IDs, times, and event shape. Given the same seed, count, and locked dependency versions, it produces the same sequence; the first three events for seed `42` are pinned in a test. This gives us repeatable input for later stages, though its fixed shape does not yet represent real traffic.

Run `cargo run -- 42 3` twice, then try `cargo run -- 43 3`. The first two runs should match; the changed seed should change tenant IDs or gauge values. Run `cargo test` to check the sequence contract. Read [the generator concept](concepts/generator.md) and trace `EventGenerator::next`.

Learn: iterators, ownership, seeded library data generation, workload shape, and how to define an experiment. Read [ADR-0004](decisions/ADR-0004-use-fake-for-synthetic-values.md) to see why the adapter remains small.

## Stage 3 — Buffer and batch

**Status: implemented as a single-threaded baseline.** The [buffer](../src/buffer.rs) uses `VecDeque<Event>` with a positive logical capacity. On a full push it returns the owned event to the caller. `take_batch` removes up to a positive limit in FIFO order; the final batch may be short. The [demo](../src/main.rs) uses capacity `2` and batch size `2`: `cargo run -- 42 3` shows event 3 returned, a two-event batch drained, and event 3 retried.

Trace [the data and control flow](architecture/ingestion.md), then run `cargo test`. Explain why the buffer cannot silently lose the rejected event and why the demo can retry after draining. [ADR-0002](decisions/ADR-0002-reject-full-buffer.md) compares reject, block, and drop under the current one-thread assumption. Run the [current batch-size probe](experiments/benchmarks/generator-library-stage3.md) to see the measured limits of this baseline; the [original probe](experiments/benchmarks/batch-size-stage3.md) remains as historical evidence.

Learn: collection choices, bounded memory, API contracts, state transitions, and backpressure. Measure several batch sizes against the same workload.

## Stage 4 — Model the ownership guarantee

**Status: target protocol modeled and checked; receiver-side local commit implemented in Stage 5.** The [delivery architecture](architecture/delivery.md) describes the handoff. The [TLA+ model](../formal/delivery/README.md) tracks an upstream copy, a volatile receiver copy, a durable receiver copy, and an ACK received upstream. Its safety rule is “an acknowledged event has a durable owner.” The [TLC investigation](experiments/formal/delivery-ownership.md) checks two events and records early-ACK counterexamples.

Run `bash formal/delivery/check.sh` with `TLA_JAR` set as in the model README. It should report one safe model and two expected counterexamples. Follow `Receive → Commit → Acknowledge → Forget` in the [spec](../formal/delivery/DeliveryOwnership.tla), then ask what happens if the receiver crashes after each step. Compare the model's volatile set with [EventBuffer](../src/buffer.rs): `try_push` moves an owned Rust value into memory, but it does not make a durable commit. A failing trace would have an ACK or forgotten upstream copy without a durable receiver copy.

Learn: states, actions, invariants, safety versus liveness, and counterexamples. TLA+ explores behavior over time; Z3 can later answer a different question, such as whether tenant quotas fit a memory budget. The Stage 4 model uses TLC through a Java runtime; it adds no Rust dependency.

## Stage 5 — Preserve ownership on disk

**Status: local receiver boundary implemented and checked.** [EventLog](../src/log.rs) frames each event with a length and CRC32, syncs its bytes, then writes and syncs a commit marker. The optional `write` command prints `committed event N` only after both syncs succeed. `replay` opens the file, validates committed frame-marker pairs, removes an unmarked final event, and prints recovered events. Repeating `write` with the same seed and a count at least as large verifies and skips the existing generator prefix.

From the repository root, run:

```sh
cargo run --offline -- write target/learning-events.log 42 3
cargo run --offline -- replay target/learning-events.log
cargo run --offline -- write target/learning-events.log 42 3
```

The first command should report three committed events. The separate replay process should print three events. The final command should report three already committed events and append none. Inspect the [storage architecture](architecture/storage.md), then trace `EventLog::append`, `recover`, and `run_write`. Run `cargo test --offline --locked --test log --test cli_log` to exercise incomplete-tail and corruption cases. If the path already contains another workload, use a fresh path.

The sender may release its value only after successful append under the storage assumptions in the [storage view](architecture/storage.md). An append error can leave a partial event, a complete unmarked event, or a visible marker after an ambiguous marker sync. Recovery discards unmarked tails with plausible partial headers or markers; it does not decode partial payloads. Visible bytes after a failed storage sync do not prove durability. After a storage I/O error, rebuild from an independent trusted source on healthy storage instead of automatically resuming the path. The CLI reconstructs only its deterministic source on an ordinary manual restart. There is no general sender, network ACK, or power-loss test. A record missing after a successful append and process restart, or damage to checked fields silently skipped by recovery, would falsify the intended local behavior under its assumptions.

Learn: file I/O, framing, checksums, partial writes, crash recovery, and the difference between “written” and “durable.”

## Stage 6 — Measure and challenge the baseline

**Status: first baseline measured; no alternative selected.** The [local-log baseline](experiments/benchmarks/local-log-stage6.md) fixes one Gauge workload, defines each timing boundary, and preserves five release trials with exact separate-process replay. Run the [small probe](../examples/local_log_probe.rs) using the commands in that record, then explain why a successful `EventLog::append` includes both syncs and why a full-buffer rejection still leaves the caller owning its event. Compare the pipeline interval with the append samples: nearly all measured time was inside append, which includes encoding and both syncs. The [S0 follow-up](experiments/benchmarks/append-attribution-s0.md) separates those phases: syncs dominate the instrumented run, while the paired build differs by 11.017%, above its perturbation threshold. Explain why that flag limits precise cost attribution before choosing an optimization.

Learn: benchmarking, ablation, experimental controls, and how optimizations move costs instead of making them disappear.

The parallel [storage/query research agenda](experiments/ablation/observability-storage-research.md) began with an [S1 probe](../tools/storage-probe/README.md): replay the existing log, move rows into a private immutable snapshot, and compare full scans with optional block summaries. Explain why a missing Bloom filter means “scan,” why duplicate event IDs do not collapse row positions, and why reducing rows inspected in RAM does not establish reduced disk I/O. Read the [S1 result](experiments/ablation/storage-query-s1-run-01.md); its work reductions do not select a disk layout. This is Stage 6 research tooling; Stage 7's application query/API remains unimplemented.

The completed coverage increment within Stage 6 is [E1R](experiments/ablation/coverage-e1-run-01.md), with its [registered protocol](experiments/ablation/coverage-e1-protocol.md).
Trace `SealedSnapshot::new`, `query`, and `verify` in the [coverage module](../tools/storage-probe/src/coverage.rs).
Explain why owned private rows prevent post-validation mutation, why `Result<CoverageStatus, CoverageError>`
separates incomplete evidence from invalid metadata, and why the metadata-only verifier cannot detect
an honestly sealed bad summary. Run the independent coverage tests before adding resumable answers.
The [experimental decision](decisions/ADR-0007-experiment-with-coverage-receipts.md) states the trust boundary;
this work does not advance the application to Stage 7.

The completed local research increment is the [local prototype](architecture/research-prototype.md). Run the [separate-process demo](../tools/storage-probe/README.md) in a fresh directory, then trace `EventBuffer::try_push`, `disk::publish`, `Accumulator::residual`, and `disk::load_checkpoint`. The Rust idea is that a rejected owned `Event` remains available to retry, while a borrowed append can report success only after the log's sync boundary. The system contract adds an independently retained publication root and checkpoint digest: a snapshot or checkpoint cannot authenticate itself. Try removing a candidate block and observe an incomplete answer; restore it and resume against the same root. Contrast that with an authenticated metadata exclusion, which needs no raw read but says nothing about raw retention. The [S2 result](experiments/ablation/durable-snapshot-s2-run-01.md) checks restart and fail-closed behavior. Read the [completed E3 corpus](experiments/ablation/resume-e3-run-01.md) and [cost trials](experiments/benchmarks/research-costs-run-01.md), including receipt overhead on cheap predicates.

The separate [hybrid layout probe](../tools/layout-probe/README.md) introduces Parquet projection and optional postings for a registered S3/S4 comparison. Its complete raw Event column duplicates predicate fields, and whole-file authentication still reads all bytes before projection. Use its exact query tests to reason about positional equality and fallback; use the [measured costs](experiments/benchmarks/research-costs-run-01.md) to explain why compression can pass the synthetic gate while a validating sidecar fails its total-CPU gate. These results do not select an application layout. The offline Logs adapter is similarly a strict local profile, with no network receiver claim.

## Stage 7 — Add the application edges

The [product contract](PRODUCT-CONTRACT.md) and [qualification](QUALIFICATION.md) begin with contracts and a controlled source before a Spindle or server is promoted. In [the source generator](../tools/qualification/workload.py), a Python iterator yields one offer at a time; the caller checks a byte cap before each write. In [the runner](../tools/qualification/runner.py), `run` returns an explicit result with `passed` and `stop_reason`, so a process that exits `0` after exceeding a budget still fails the gate. The independent [rate oracle](../tools/qualification/rate_oracle.py) checks the registered per-second offered count and backlog arithmetic. Run `python3 -B tools/qualification/test_runner.py` and `python3 -B tools/qualification/test_workload.py`, then inspect the intentional overrate, fast-exit disk and nonfinite-duration defects. This teaches the distinction between a generated schedule and actual timely admission. The harness has a [fresh FOL2 baseline](experiments/benchmarks/alpha-phase0-baseline.md); the Spindle has [native measurements](experiments/benchmarks/alpha-phase1-native-run-02.md) from an earlier revision.

For the Spindle's [Spool](../src/spindle/spool.rs), `append` borrows the caller's batch, returns a committed copy only after two successful file syncs, and quarantines the writer on a write or sync error. A corrupted length once made recovery erase two acknowledged batches; a checksum on the header now rejects the [frozen counterexample](experiments/formal/alpha-journal-length-repair.md). Two sidecars separate an interrupted append from a known failure: a process killed mid-append leaves `append-in-progress`, and reopen keeps only frames with a valid commit marker; a reported sync error writes `recovery-required`, which blocks reopen. Run `python3 -B tools/qualification/kill_probe.py` to SIGKILL `collect` hundreds of times and check exact replay. This teaches why a commit order (data sync before marker) lets a crash be resolved while readable bytes still cannot settle a reported error ([ADR-0011](decisions/ADR-0011-separate-interrupted-append-from-known-failure.md)).

Trace one [Spindle](architecture/spindle.md) cycle: build an owned OTLP batch from bounded host and log reads; move its source cursor only after the batch commits. Run `cargo test --offline --locked --test spindle --test log_source` to observe source errors, gap bounds, same-inode replacement and spool exhaustion. The small prefix witness detects changed consumed bytes but cannot prove an unchanged file when the witness matches.

The Btrfs cursor fix illustrates `Result<Option<T>>`: `Ok(Some(identity))` is a
stable namespace, `Ok(None)` is a successfully identified different filesystem
type, and `Err` is an unknown identity. Collapsing the last two would turn an
unavailable lookup into permission to reread or skip data. The
[adapter regressions](../crates/fabric-adapter-linux/src/log_source.rs) and
[quiet migration tests](../src/spindle/skip_progress_tests.rs) check the distinct
outcomes and commit-before-cursor rule.

Once the core pipeline has understandable behavior, add ingestion protocols, a query path, an API, and a UI in small slices. Keep external formats and storage engines at the edges of the domain model. Before selecting a network ingestion mechanism, work through the [Homa/SIRD receiver-driven transport study](experiments/ablation/receiver-driven-transport.md). Its first [H1 packet-slot ablation](experiments/ablation/receiver-credit-h1-run-01.md) compares fixed sender windows with receiver credits; the [finite model](experiments/formal/transport-credit-ownership.md) checks that credit permission and durable ACK ownership remain distinct. Try the tiny simulator tests, then read the raw result: receiver credits lower modeled switch peaks while increasing sender waiting. The priority, active-grant, sender/core feedback, sink-limited, and real-host cells remain to be built. There is no application network transport to benchmark yet.

From the repository root, run `cargo test --offline --locked --manifest-path tools/transport-sim/Cargo.toml one_message_serialization_and_propagation`. In that small test, M0 sends its first packet at tick `0`, finishes the three-packet message at tick `7`, and receives the modeled durable ACK at tick `11`. M1 waits for an announcement and credit, first sends at tick `8`, finishes at tick `15`, and gets its ACK at tick `19`. Trace the `Event` enum and `MessageState` in [the simulator](../tools/transport-sim/src/lib.rs): `Option<u64>` records which milestones have actually occurred, and `Result` stops a run that violates a cap or invariant. The simulation owns packet copies in its queues; the sender's retained message remains its retry responsibility until the durable ACK event.

## Stage 8 — Deliver, control and retain

**Delivery.** A node owns each batch until the server says it is durable. In [the node](../src/spindle/runtime.rs), `deliver` reads the oldest unacknowledged batch as its exact stored bytes, sends it, and calls `record_ack` only for an `ack`. On the server, one commit thread in [the store](../crates/fabric-server/src/store.rs) owns the journal: handlers pass a `Submission` holding the bytes and a reply channel, and the thread answers only after the group's data and marker syncs. Run `cargo test --offline --locked -p fabric-server --test delivery`, then `python3 -B tools/qualification/delivery_faults.py --scenario server-kill --seed 1` to watch the [delivery oracle](../tools/qualification/DELIVERY_ORACLE.md) grade real processes killed mid-flight. The idea: an ACK transfers responsibility, so it may follow durability, never precede it.

**Control.** Configuration flows the other way. The server stores a desired revision per node; the node polls, validates with the same `Config::validate` as a local file, stores the view by synced rename, and only then activates it. Read [central control](architecture/control-plane.md) and run `cargo test --offline --locked -p fabric-server --test control`. The idea: validation and durability come before activation, so a restart never runs something it could not have accepted.

**Retention.** Sealed journal files become immutable Parquet [segments](../crates/fabric-server/src/segment.rs); a directory rename is the commit point, and the commit thread writes a stream checkpoint before it deletes the journal file. Queries bind pages to a range of group numbers rather than to files, because records move from the journal into segments but never change group. Run `cargo test --offline --locked -p fabric-server --test history`; its answers are graded by an independent [query oracle](../tools/qualification/QUERY_ORACLE.md) written before the query code. The idea: an exact scan is the reference, and every optimization must agree with it. The [sealer scheduler](../crates/fabric-server/src/sealer.rs) distinguishes publication from reclamation: builds may finish out of order, while deletion advances only through the oldest published prefix. Scoped thread handles retain worker ownership. Joining in label order permits reclaim after each completed prefix without a completion queue; even after reclaim fails, every started worker is joined. Run `cargo test --locked -p fabric-server --lib sealer::tests`; its blocked-worker controls cover both sibling and later-group progress, and its checkpoint fixture verifies exact replay after restart. The trade-off still to measure is checkpoint work competing with ACKs.

**Packaging.** [packaging/](../packaging/) turns the binaries into two sandboxed systemd services under one slice, running as the static `fabricolly` user. Run `packaging/build-deb.sh target/package-out` twice and compare the SHA-256 values to see a reproducible build.

## Stage 9 — Separate decisions from effects

**Status: architecture foundation milestone.** The delivery rule used to be decided inside the server's commit loop, next to `SystemTime::now()` and the journal append. It now lives in [`fabric_core::delivery`](../crates/fabric-core/src/delivery.rs) as a pure function: `decide_delivery(committed, incoming, binding)` returns a `DeliveryDecision` value, and [`commit_group`](../crates/fabric-app/src/delivery.rs) performs effects only through the `DurableJournal` and `Clock` traits in [fabric-ports](../crates/fabric-ports/src/lib.rs). The Rust ideas: a `#![no_std]` crate cannot name `std::fs` or `SystemTime`, so purity is structural rather than a promise; `NonZeroU64` makes sequence 0 unrepresentable; `checked_add` turns the `u64::MAX` overflow into an explicit "no successor". The system contract: an ACK is returned only when the journal reports a durable commit. The trade-off: the caller must compute the digest and read the clock, and `BTreeMap` replaces `HashMap`.

Run `cargo test -p fabric-core -p fabric-app`, then read [the differential test](../crates/fabric-app/tests/delivery.rs): a frozen copy of the old loop is compared with the new kernel on millions of small groups. Run `cargo xtask check-layers` and `cargo xtask check-core-purity`, then add `ureq` to `crates/fabric-core/Cargo.toml` and run `target/debug/xtask check-core-purity --declared-only` to watch the gate reject it (undo the edit afterwards). Finally run `cargo xtask mutants --only M-DEL-ACK` and explain why a test that cannot fail proves nothing.

## Stage 10 — Try to falsify the kernels

**Status: verification-tooling milestone.** Example tests check the inputs someone thought of. Four tools search for the inputs nobody thought of:

- **Property tests** state a contract for all inputs a generator can produce, and shrink any failure to a minimal case: [fabric-properties](../crates/fabric-properties/tests/kernels.rs).
- **Kani** checks a contract for every value of the input types, within a bound, and also looks for panics and overflow: [proofs.rs](../crates/fabric-core/src/proofs.rs).
- **Fuzzing** mutates bytes, keeps the ones that reach new code, and hunts for panics in decoders: [fuzz/](../fuzz/).
- **turmoil** runs the real server on a simulated network, where partitions and lost answers happen on a seeded schedule: [fabric-sim](../crates/fabric-sim/tests/delivery.rs).

The Rust ideas:

- a crate in a separate layer can depend on the product while nothing depends on it;
- `#[cfg(kani)]` compiles proof code only for the model checker;
- a `u128` sum of `u64` sizes cannot overflow, where a saturating `u64` sum silently loses information.

The system contract: a rate row is either a reset or carries a finite rate.

The trade-off: every tool has a bound. A property is only as wide as its generator, Kani only as deep as its unwinding bound, and a fuzz run only as long as its time box.

Try it:

1. Run `cargo test -p fabric-properties` and `bash formal/kani/check.sh`.
2. Read [CX-RETENTION-SATURATED-TOTAL](formal/counterexamples.json). Explain why proptest, whose generated sizes stay below 50 bytes, could not find it, and why Kani did.
3. Run `cargo xtask mutants --only M-SIM-DUPLICATE`. Explain which lost answer makes the simulation fail.

## Stage 11 — Sort more data than fits in memory

**Status: normal writer adopted and verified.** The [continuation](experiments/formal/readiness-continuation-results.md) records builder/recovery checks, 17 final fast checks and three documentation checks; the [native R2 trial](experiments/benchmarks/soak-run-02.md) passed all ten service gates. The whole-file builder remains an independent reference. Sort in pieces, then merge, trading extra disk work for lower live heap.

1. Read the file as a stream, a frame at a time, and cut the rows into **runs** of a fixed size.
2. Sort each run in memory and write it to a scratch file.
3. Merge at most sixteen runs at once, keeping their front rows in a min-heap. Use multiple passes when there are more runs. Pop the smallest, write it, and refill from that run.

The [speed investigation](experiments/benchmarks/sealer-speed-run-01.md) shows
why representation and scheduling matter even after choosing the algorithm:
private binary rows avoid JSON parsing, and eighteen runs need only three
merged into one to reach sixteen. Keeping all untouched runs on disk saves
work without raising the live-row limit. Try the boundary cases with
`python3 tools/resource_group.py -- cargo test -p fabric-server segment::bounded`.

Major payload buffers are capped per sorted table, with at most sixteen merge heads. A decoded frame, writer metadata and filters have additional costs; the measured screen is not a universal heap proof. The [sealer view](architecture/sealer.md) has the diagram, the steps and a ten-row worked example.

The normal writer teaches a second ownership
boundary: an Arrow input chunk can be released while the Parquet writer
still owns encoded state for a physical row group. Logs align input to
1,024-row encoder batches with a 17 MiB estimated byte target; metrics and spans
retain their 8,192-row/8 MiB estimated input bounds. All sorted tables retain
8,192-row physical groups. A single oversized row is preserved despite exceeding
the input target, so the estimates are not absolute heap bounds. Filters must
accumulate over the physical group's rows, not reset at each input chunk. The
combined campaign measured about 37.3 MiB incremental heap at steady256 and
39.5 MiB at bigrows64. These finite measurements do not establish an
arbitrary-input or whole-server bound.

The [encoded-page probe](experiments/formal/encoded-page-memory-run-01.md) shows
why the next owner matters: high-entropy logs raised heap to 143,075,502 bytes
despite bounded input chunks. The normal writer now moves completed Parquet
blobs from every table into private disk scratch,
retaining locators and loading one consumed key at a time. The same probe then
used 42,011,899 bytes and preserved exact files. Read
[page_store.rs](../crates/fabric-server/src/segment/bounded/page_store.rs): a key
can be taken once, and failed deletion prevents publication. This trades resident
payload for scratch I/O; the finite result does not bound arbitrary blobs or
metadata. Legacy `FABRIC_ROW_GROUP_CHUNKS_EXPERIMENT=1` and
`FABRIC_PAGE_STORE_EXPERIMENT=1` remain accepted but are no longer required.
Unflagged bounded/recovery checks passed, and loaded-ELF comparison connects
the normal server path to the accepted frozen trial; full-file hashes are not
identical. The continuation records the exact receipts and remaining final checks.

The Rust ideas:

- an iterator yields one item at a time, so a stream never needs the whole collection;
- `BinaryHeap<Reverse<T>>` is a min-heap, because the standard heap is a max-heap;
- a type that wraps a writer and hashes each write (an adapter over `Write`) computes a checksum without a second pass;
- a guard value with a `Drop` implementation can remove a scratch directory on every exit path.

The system contract: a Segment answers every query as it did before, because its rows are in the same order and its row groups cover disjoint times.

The trade-off: the merge costs disk. Spill is about 1.1 times the file, and the peak is about three times the file while a seal runs. Cheaper algorithms exist, and the [study](experiments/benchmarks/sealer-study-run-01.md) measured why each was rejected: sorting each chunk alone makes queries read up to 5.7 times more rows.

Try it:

1. Run the [worked example](architecture/sealer.md#a-worked-example) by hand with a run size of three. How many runs are there? How many rows does the merge hold at once?
2. Read the [study's heap table](experiments/benchmarks/sealer-study-run-01.md#results). Why did a limit of 16,384 rows fail on 16 KiB rows, and what does the design count instead?
3. Run `python3 tools/resource_group.py -- cargo test -p fabric-server segment::bounded` and inspect stable ties, the 64-seed byte-limit checks and fan-in boundaries. The named merge-order, spill-left and retain-runs mutants were caught in the continuation; the retain-runs defect failed the registered steady128 heap ceiling.
4. Run `python3 tools/resource_group.py -- cargo test -p fabric-server --test completion_storage named_sealer_kill_cuts_recover_exact_custody_and_oracle_query_chains -- --exact`. Why does comparing a recovered manifest alone give weaker evidence than grading query chains against the predeclared source ledger before and after restart?
5. Read [spool_enospc.rs](../tests/spool_enospc.rs): a real scoped ENOSPC leaves prior unACKed bytes intact but marks the writer quarantined. Explain why readable bytes do not authorize normal reopening after a reported I/O failure, and how the no-hit control distinguishes successful injection from a test that never reached its intended syscall.

## Stage 12 — One record for every signal, built from the bytes up

**Status: accepted for the server's in-memory block tail ([ADR-0023](decisions/ADR-0023-define-an-observation-record-with-a-canonical-encoding.md), [ADR-0024](decisions/ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md) part 3); not on the wire or on disk.** Fabric keeps two copies of every line: the node's exact OTLP bytes (custody) and a body column (query). [Storage layout run 01](experiments/benchmarks/storage-layout-run-01.md) measured that the pair is most of a Segment. The two cannot be one object because OTLP's protobuf is not canonical: the same observation has many byte strings, so the server can only vouch for the bytes it received, never for the records.

The Rust idea: a **canonical encoding** is a pair of functions with `decode(encode(b)) == b` for every valid value *and* `encode(decode(x)) == x` for every accepted byte string. The second half is the hard one, and it cannot be bolted on at the top: a decoder that tolerated one overlong varint anywhere would give the same records two byte strings. So [fabric-observation](../crates/fabric-observation/src/lib.rs) is built as a tower ([architecture page](architecture/observation.md)), each level stating what it refuses before the next is allowed to use it:

1. **bits**: shifts and masks on a machine word, the seven-bit group a varint byte carries, little-endian packing. The standard library's conversions are used only as the oracle the proofs compare these against.
2. **bytes** and **crc32**: a reader that cannot run past its slice and names the offset of every failure; the IEEE check built from its polynomial, bit by bit, with a compile-time table proved equal to the definition.
3. **varint**: integers in exactly one (shortest) form; counts that cannot reserve more than the bytes left.
4. **zigzag**: signed to unsigned, a bijection proved for every value.
5. **delta**: differences that invert under wrap, so a regular series costs one byte per element.
6. **dictionary**: repeated values once, in a table whose order is a function of the data.
7. **cells**: numbers and attributes with one form each; no NaN, keys sorted.
8. **record**: the Observation and the rules that give it an encoding.
9. **block**: columns, two dictionaries, a CRC; `encode` and `decode`.

The crate has no dependencies and no standard library beyond `core` and `alloc`, like the semantic core.

The contract: the crate is a pure codec in adapter support; it decides nothing about delivery or retention and performs no effect. The trade-off: strictness. A reader that accepts only canonical bytes refuses input a lenient one would take, by design; and the type is only useful once the node emits it, which is a wire-format decision this stage does not take.

Try it:

1. Read [bits.rs](../crates/fabric-observation/src/bits.rs), then [varint.rs](../crates/fabric-observation/src/varint.rs) and its test `overlong_forms_are_rejected`. Then read the property `varint_accepts_only_the_shortest_form` in [observation_levels.rs](../crates/fabric-properties/tests/observation_levels.rs). Why is the second the stronger statement, and what would it take to prove it for every input rather than test it?
2. Run `cargo test -p fabric-properties --test observation`. The mutation property found [CX-FOB1-DUPLICATE-DICTIONARY](formal/counterexamples.json) on its first full run. Which level's contract was incomplete, and why did the block-level round-trip test not see it?
3. Read [observation encoding run 01](experiments/benchmarks/observation-encoding-run-01.md). Why does the encoding save a fifth on real text and nothing on the synthetic workload, and what does that say about which workload to measure storage on?

## Stage 13 — Read only what the answer needs

**Status: part 1 of [ADR-0024](decisions/ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md) implemented as `query_plan=walk`; default `scan`.** A `limit 50` query over a 64 MiB real-text tail decoded all 149,585 entries to return 50 rows. The [optimality bounds](research/optimality-bounds.md) state what any algorithm must read for each shape, and the walk reads that plus one source.

The Rust idea: **borrow the decision, own the effect**. The rule that makes stopping sound lives in the core as an executable definition (`fabric_core::query::spec::threshold_walk`, with its theorem as a property); the server's [query.rs](../crates/fabric-server/src/query.rs) and [tail.rs](../crates/fabric-server/src/tail.rs) apply it to files, holding a mutex only while the index is extended and a frame cache only for one query.

The [scoped evidence cache](../crates/fabric-server/src/read_catalog/evidence.rs)
illustrates a different ownership boundary: derived facts can outlive one query,
but authorization cannot. Its mutex serializes bounded construction; each caller
selects current source and signal grants from the immutable summary. A partial
snapshot uses the exact reader. The
[integration controls](../crates/fabric-server/tests/scoped_evidence.rs) demonstrate
why a cached full-Segment maximum would be incorrect for an older page.

The contract: the walk returns the scan's answer, page for page. The trade-off: a few MiB of index per process and a first query that builds it, against reading every source on every query.

Try it:

1. Read `Smallest::threshold` and the stop line `if best.threshold().is_some_and(|t| min > t.0)` in query.rs. Why is it `>` and not `>=`? The test `walk_and_scan_plans_answer_identically` needed a third Spindle writing at the same instants before it could tell the two apart.
2. Run `cargo test -p fabric-server --test history walk`.
3. Read [query walk run 01](experiments/benchmarks/query-walk-run-01.md). Which shapes did the walk not speed up, and which part of ADR-0024 addresses each?
4. Read [text_filter.rs](../crates/fabric-server/src/text_filter.rs) and the test `text_filters_skip_only_groups_without_the_needle_and_fall_back_when_corrupt`. A Bloom filter has no false negatives only while its bytes are the sealer's: what does the reader do to keep that true, and which failure does the last part of the test show it cannot catch?
5. Read `Pending::close` and `visit_block` in [tail.rs](../crates/fabric-server/src/tail.rs), then [block tail run 01](experiments/benchmarks/block-tail-run-01.md). Why are the blocks kept in memory and not written beside the journal, and what does the walk do with a block the codec refuses?

## Working rule

For each new component, answer these in plain language before coding:

1. What does it receive and produce?
2. What promise does it make?
3. Who owns the data before and after it runs?
4. What can fail, and how will that failure be visible?
5. What measurement or model could prove the design wrong?

**Streaming output experiment.** The [table writer](../crates/fabric-server/src/segment.rs) on the streaming-output branch uses a buffered sink and hashes only bytes accepted by `Write`. It flushes before file sync. [Run 01](experiments/benchmarks/streaming-output-local-run-01.md) shows why removing a whole-file output buffer can preserve every byte and still miss a peak-heap target: decoded rows and Arrow arrays remain. Lower RSS and lower live heap are different observations.

The [Fedora refinement protocol](experiments/benchmarks/performance-refinement-fedora-protocol.md) connects typed counter inputs to fidelity: an `i64` retains information a premature `f64` conversion loses. Structural maps define identity; cached display keys define presentation order. The independent rate oracle and HTTP/Segment/restart regression check these two responsibilities separately.

The [development/small observation](experiments/benchmarks/dev-small-observation-run-01.md)
connects custody transitions to performance units: source logs offered, successful
Spool batches, and server ACKs are different counters. Larger batches can carry
three times the log rate without increasing batch rate. Keep epoch nanoseconds
as integers when assigning phases; its independent accounting check found a
floating-point boundary error even though the native workload preserved all data.

The [matched native lab screen](experiments/benchmarks/dev-small-labs-run-02.md)
connects Rust ownership to allocation lifetime: bounded builder buffers do not
bound query-owned decoded rows, concurrent copies or allocator-retained RSS.
Compare query-on/off with and without history before attributing process memory
to ingestion. Explain why a 1 Hz publication sample brackets a transition without
proving that a particular request overlapped its builder.

The [fixed-demand query comparison](experiments/benchmarks/query-plan-run-01.md)
separates consumer work from plan cost: offer the same requests at the same cadence
before comparing CPU. Its profiling counterexample also separates one exact page
from a complete pagination transcript. Explain why a valid continuation token
must not be interpreted as lost rows, and why receive timestamps can differ when
identical Batch bytes reach storage through different fixture construction paths.

The [catalog controls](experiments/benchmarks/catalog-cold-lifetime-findings.md)
connect `Arc` and `Weak` to two separate lifetimes: a held Sources view owns
metadata allocations, while retention may delete their described files. Pointer
identity distinguishes a shared owner from a same-content allocation; aborting
and joining a paused task releases its owners. Explain why neither surviving
metadata nor a cached locator makes an expired page valid. The preserved
[discovery counterexample](experiments/benchmarks/catalog-discovery-race-findings.md)
also shows why fallible source acquisition must report movement as `Interrupted`
instead of converting a publication/reclaim gap into a successful empty answer.

The [descriptor reuse screen](experiments/benchmarks/coupled-query-findings.md)
extends this exercise: explain snapshot retirement through `Weak`, invalidation
when published labels change, and fallback when admission limits are exceeded.
Distinguish a structural cap and estimated charged bytes from measured allocator
usage or process RSS; ownership of metadata still does not own a file lease.

The [coupled completion record](experiments/benchmarks/coupled-completion-run-01.md)
connects three further boundaries to executable controls. First, read
`check_raw_available` in [segment.rs](../crates/fabric-server/src/segment.rs) and
compare the original query oracle with the separately scoped
[availability companion](../tools/qualification/projection_availability_oracle.py).
Explain why missing raw custody makes an answer incomplete while surviving
projection rows and producer-derived metadata remain, and why a size/schema/footer
check does not authenticate raw page contents. The companion's all-raw-lost case
requires a nonempty matching producer population; empty-window semantics are not
claimed by that fixture.

Next, trace `Runs::push` in [bounded.rs](../crates/fabric-server/src/segment/bounded.rs):
its strict compile selector permits 8/16/32 MiB, default 16, and counts estimated
owned bytes separately for each signal. Explain why an oversized single row is
an explicit exception. The [native pruning protocol](experiments/benchmarks/coupled-pruning-native-protocol.md)
checks real group bounds and exact query chains; 42 finite chains and matching
Manifest/table files do not measure actual query IO or prove all workload shapes.

Finally, trace `deliver_with_one_prepared` in
[runtime.rs](../src/spindle/runtime.rs). The caller owns mutable Spool/cursor/config/
meter state; the scoped worker owns immutable request bytes and joins before ACK
processing. Explain why retrying N with durable N+1 forbids preparing N+2, and why
private-worker controls differ from a real TLS performance pilot. Six controls
passed in controls02; subsequent service screens did not nominate overlap and
the production CLI remains serial. The [overlap protocol](experiments/benchmarks/coupled-overlap-protocol.md)
rejects trace listeners rather than extending that experiment's ownership scope.

The [delivery work investigation](experiments/benchmarks/catalog-delivery-read-findings.md)
then connects a move to a measurable ownership property: `append_owned` preserves
the addresses of signal buffers, cursor paths and gap strings through durable
append, while the borrowed compatibility method clones them. Explain why moving
a `Vec` transfers its allocation, why source cursor/history updates must still
wait for commit, and why fewer requested allocations do not imply the same
percentage reduction in RSS or wall time. Compare this with the rejected ACK
cache: less work is not sufficient when evidence is mixed and error detection
changes. The observer ablation also shows why measurement overhead must be
separated from product cost.

The [algorithm round](experiments/benchmarks/catalog-algorithm-round-findings.md)
extends ownership into data-structure choice. Explain why a heap needs rank keys
but payloads can live in dense reusable slots, and why slot numbers cannot replace
admission order when keys tie. Trace `GroupPlan` from one inline binding through
a compact list to indexed bindings; show how promotion preserves the first
forward mapping and every reverse mapping. Its initially slower eight-source
case illustrates why asymptotic improvement alone is insufficient for small
deployments. Finally, compare `Option<History>` with cloning unchanged state:
absence of a replacement means retain the current owner, while a new owner still
cannot become authoritative until the Spool commit succeeds.

The [progress and projection round](experiments/benchmarks/catalog-log-progress-findings.md)
connects that ownership rule to liveness. A bounded reader can advance its local
skip cursor while the collector repeatedly refuses an empty Batch. Trace how a
real late metric sample lets that progress use the existing durable commit path,
and why a full Spool must still preserve the old cursor/history. Then compare
borrowing a decoded log tree with consuming it in
[rows.rs](../crates/fabric-server/src/rows.rs): moving Strings avoids redundant
copies while map construction must preserve duplicate and non-string behavior.
Use the counterexample and exact pointer/output controls; distinguish extraction
timing from end-to-end service performance.

The [native lifecycle round](experiments/benchmarks/catalog-native-lifecycle-findings.md)
then crosses the component boundaries. In
[server composition](../crates/fabric-server/src/lib.rs), explain why a fallible
TLS load must precede spawning workers that own an Intake: dropping a join handle
does not join its thread, and another owner can keep the journal locked. Read the
failed startup trace before the corrected retry. In
[the native integration test](../crates/fabric-server/tests/native_lifecycle.rs),
compare producer frames with recovered bytes before asking the Python oracle to
grade queries. Explain why agreement between a query and incomplete recovered
input alone would not establish end-to-end custody.
