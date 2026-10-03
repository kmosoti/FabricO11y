# Learning path

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

Once the core pipeline has understandable behavior, add ingestion protocols, a query path, an API, and a UI in small slices. Keep external formats and storage engines at the edges of the domain model. Before selecting a network ingestion mechanism, work through the [Homa/SIRD receiver-driven transport study](experiments/ablation/receiver-driven-transport.md). Its first [H1 packet-slot ablation](experiments/ablation/receiver-credit-h1-run-01.md) compares fixed sender windows with receiver credits; the [finite model](experiments/formal/transport-credit-ownership.md) checks that credit permission and durable ACK ownership remain distinct. Try the tiny simulator tests, then read the raw result: receiver credits lower modeled switch peaks while increasing sender waiting. The priority, active-grant, sender/core feedback, sink-limited, and real-host cells remain to be built. There is no application network transport to benchmark yet.

From the repository root, run `cargo test --offline --locked --manifest-path tools/transport-sim/Cargo.toml one_message_serialization_and_propagation`. In that small test, M0 sends its first packet at tick `0`, finishes the three-packet message at tick `7`, and receives the modeled durable ACK at tick `11`. M1 waits for an announcement and credit, first sends at tick `8`, finishes at tick `15`, and gets its ACK at tick `19`. Trace the `Event` enum and `MessageState` in [the simulator](../tools/transport-sim/src/lib.rs): `Option<u64>` records which milestones have actually occurred, and `Result` stops a run that violates a cap or invariant. The simulation owns packet copies in its queues; the sender's retained message remains its retry responsibility until the durable ACK event.

## Stage 8 — Deliver, control and retain

**Delivery.** A node owns each batch until the server says it is durable. In [the node](../src/spindle/runtime.rs), `deliver` reads the oldest unacknowledged batch as its exact stored bytes, sends it, and calls `record_ack` only for an `ack`. On the server, one commit thread in [the store](../crates/fabric-server/src/store.rs) owns the journal: handlers pass a `Submission` holding the bytes and a reply channel, and the thread answers only after the group's data and marker syncs. Run `cargo test --offline --locked -p fabric-server --test delivery`, then `python3 -B tools/qualification/delivery_faults.py --scenario server-kill --seed 1` to watch the [delivery oracle](../tools/qualification/DELIVERY_ORACLE.md) grade real processes killed mid-flight. The idea: an ACK transfers responsibility, so it may follow durability, never precede it.

**Control.** Configuration flows the other way. The server stores a desired revision per node; the node polls, validates with the same `Config::validate` as a local file, stores the view by synced rename, and only then activates it. Read [central control](architecture/control-plane.md) and run `cargo test --offline --locked -p fabric-server --test control`. The idea: validation and durability come before activation, so a restart never runs something it could not have accepted.

**Retention.** Sealed journal files become immutable Parquet [segments](../crates/fabric-server/src/segment.rs); a directory rename is the commit point, and the commit thread writes a stream checkpoint before it deletes the journal file. Queries bind pages to a range of group numbers rather than to files, because records move from the journal into segments but never change group. Run `cargo test --offline --locked -p fabric-server --test history`; its answers are graded by an independent [query oracle](../tools/qualification/QUERY_ORACLE.md) written before the query code. The idea: an exact scan is the reference, and every optimization must agree with it. The [sealer scheduler](../crates/fabric-server/src/sealer.rs) distinguishes publication from reclamation: builds may finish out of order, while deletion advances only through the oldest published prefix. Run `cargo test --locked -p fabric-server --lib sealer::tests`; its blocked-worker test observes reclaim before releasing the next group, and its checkpoint fixture verifies exact replay after restart. The trade-off still to measure is checkpoint work competing with ACKs.

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

**Status: bounded-sealer milestone, design only.** The sealer turns a 64 MiB journal file into a Segment, and today it needs about 356 MiB to do it. The fix is an algorithm older than databases: sort in pieces, then merge.

1. Read the file as a stream, a frame at a time, and cut the rows into **runs** of a fixed size.
2. Sort each run in memory and write it to a scratch file.
3. Open every run and keep only its front row in memory, in a min-heap. Pop the smallest, write it, and refill from the run it came from.

Memory is one run plus one row per run, whatever the file's size. The [sealer view](architecture/sealer.md) has the diagram, the steps and a ten-row worked example.

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
3. After the milestone merges, run `cargo test -p fabric-server` and find the test that fails when a run is kept in memory.

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
