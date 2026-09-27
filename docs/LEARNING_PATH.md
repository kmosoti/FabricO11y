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

The approved [Linux alpha phase ledger](ALPHA.md) begins with contracts and a controlled source before a node or server is promoted. In [the source generator](../tools/alpha/workload.py), a Python iterator yields one offer at a time; the caller checks a byte cap before each write. In [the runner](../tools/alpha/runner.py), `run` returns an explicit result with `passed` and `stop_reason`, so a process that exits `0` after exceeding a budget still fails the gate. The independent [rate oracle](../tools/alpha/rate_oracle.py) checks the registered per-second offered count and backlog arithmetic. Run `python3 -B tools/alpha/test_runner.py` and `python3 -B tools/alpha/test_workload.py`, then inspect the intentional overrate, fast-exit disk and nonfinite-duration defects. This teaches the distinction between a generated schedule and actual timely admission. Phase 0 has a [fresh FOL2 baseline](experiments/benchmarks/alpha-phase0-baseline.md); the local node has [pre-repair native measurements](experiments/benchmarks/alpha-phase1-native-run-01.md), with final review pending. Network and fleet qualification remain unimplemented.

For the local [alpha journal](../src/alpha/journal.rs), `append` borrows the caller's batch, returns a committed copy only after two successful file syncs, and quarantines the writer on a write or sync error. A corrupted length once made recovery erase two acknowledged batches; a checksum on the header now rejects the [frozen counterexample](experiments/formal/alpha-journal-length-repair.md). A recovery-required sidecar blocks reopen after known sync failure. A live writer's marker makes inspection retryable, while an abandoned marker requires source-retained recovery. This teaches why readable bytes alone cannot settle an ambiguous durability result.

Trace one [native node](architecture/node.md) cycle: build an owned OTLP batch from bounded host and log reads; move its source cursor only after the batch commits. Run `cargo test --offline --locked --test alpha_node --test alpha_log_source` to observe source errors, gap bounds, same-inode replacement and spool exhaustion. The small prefix witness detects changed consumed bytes but cannot prove an unchanged file when the witness matches. The phase-1 review checkpoint leaves the final cross-family check and post-repair qualification to the parent.

Once the core pipeline has understandable behavior, add ingestion protocols, a query path, an API, and a UI in small slices. Keep external formats and storage engines at the edges of the domain model. Before selecting a network ingestion mechanism, work through the [Homa/SIRD receiver-driven transport study](experiments/ablation/receiver-driven-transport.md). Its first [H1 packet-slot ablation](experiments/ablation/receiver-credit-h1-run-01.md) compares fixed sender windows with receiver credits; the [finite model](experiments/formal/transport-credit-ownership.md) checks that credit permission and durable ACK ownership remain distinct. Try the tiny simulator tests, then read the raw result: receiver credits lower modeled switch peaks while increasing sender waiting. The priority, active-grant, sender/core feedback, sink-limited, and real-host cells remain to be built. There is no application network transport to benchmark yet.

From the repository root, run `cargo test --offline --locked --manifest-path tools/transport-sim/Cargo.toml one_message_serialization_and_propagation`. In that small test, M0 sends its first packet at tick `0`, finishes the three-packet message at tick `7`, and receives the modeled durable ACK at tick `11`. M1 waits for an announcement and credit, first sends at tick `8`, finishes at tick `15`, and gets its ACK at tick `19`. Trace the `Event` enum and `MessageState` in [the simulator](../tools/transport-sim/src/lib.rs): `Option<u64>` records which milestones have actually occurred, and `Result` stops a run that violates a cap or invariant. The simulation owns packet copies in its queues; the sender's retained message remains its retry responsibility until the durable ACK event.

## Working rule

For each new component, answer these in plain language before coding:

1. What does it receive and produce?
2. What promise does it make?
3. Who owns the data before and after it runs?
4. What can fail, and how will that failure be visible?
5. What measurement or model could prove the design wrong?
