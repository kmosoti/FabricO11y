# Current project state

## Active work

The approved [controlled Linux alpha contract and phase ledger](ALPHA.md) is being
implemented on the `alpha/controlled-linux-collection` branch, in the order of the
[completion plan](ALPHA-PLAN.md). Phase 0 is approved. Phase 1's close-out steps are
done: the Unicode gap-cap repair, recoverable interrupted appends with known failures
still refused ([ADR-0011](decisions/ADR-0011-separate-interrupted-append-from-known-failure.md)),
failed cycles reported as gaps, fair log reads with visible backlog, explicit lock
release, and a [post-repair native run](experiments/benchmarks/alpha-phase1-native-run-02.md)
that passed all three trials. A fault-injection [close-out review](experiments/formal/alpha-phase1-closeout-review.md)
found six defects, all repaired with discriminating regressions; its re-review is
pending. That reviewer is same-model for this author, the Gemini CLI run was denied
shell commands, and GPT review is unavailable until 2026-10-03.

Phase 2 is in progress. A frozen [delivery oracle](../tools/alpha/DELIVERY_ORACLE.md)
predates the server. The node spool and a new [Fabric Server](../crates/fabric-server/src/lib.rs)
share one rotating [frame log](../src/alpha/frame.rs). The node sends its oldest
unacknowledged batch over TLS and keeps a durable ACK cursor; the server commits grouped
frames before answering under the [ADR-0013](decisions/ADR-0013-deliver-batches-in-order-with-bounded-dedup.md)
rule. End-to-end tests pass and fail on two injected server defects, and
[oracle-graded fault runs](experiments/formal/alpha-phase2-delivery-faults.md) pass under
server kills, node kills and an outage. Ten real node processes, latency and the
commit-mode comparison remain. Ten real nodes pass the [registered delivery run](experiments/benchmarks/alpha-phase2-delivery-run-01.md)
in both commit modes. Phase 3 has [central control](architecture/control-plane.md): enrollment,
desired and applied configuration, pause, resume and revoke, and a fleet simulator; the
[registered fleet trials](experiments/benchmarks/alpha-phase3-fleet-run-01.md) pass at 10, 100 and 1,000 identities. Phases 4
and 5 are unimplemented; the [planned deployment](architecture/deployment.md) is unchanged.
FOL2 and the research packages remain supported.

The [scoped local research completion contract](experiments/ablation/end-to-end-prototype.md)
is complete. The [local research lifecycle](architecture/research-prototype.md)
composes offline collection, FOL2 ingest, immutable JSON publication, partial query,
checkpoint resume, independent verification and same-root rebuild across separate
processes. The [E3 result](experiments/ablation/resume-e3-run-01.md) passed 768,000
availability cases with zero contract violations under its stated trust assumptions.

The [registered cost results](experiments/benchmarks/research-costs-run-01.md) measure
receipt/retry overhead, four physical layouts and conservative sidecar hints. Receipts
are costly for cheap predicates; compressed hybrid layouts pass the registered
synthetic-workload gates; source-side hints fail the total-CPU benefit gate. The
research lifecycle retains JSON as its baseline. The
[Claude hypothesis study](experiments/ablation/claude-hypotheses.md) preserves the idea
lineage, killed claims and narrow surviving contracts; it establishes no novelty proof.
[S0 attribution](experiments/benchmarks/append-attribution-s0.md) retains its 11.017%
perturbation flag. The [E2 model](experiments/ablation/seal-e2-run-01.md) and
[cost trials](experiments/benchmarks/group-seal-cost-run-01.md) have a mixed result.
Network ingestion, external platforms and hardware offload remain conditional work.

The Rust application implements Stage 5: typed events, repeatable synthetic input, a bounded local FIFO buffer, and an optional framed local log with replay. Stage 4's checked [delivery ownership model](../formal/delivery/README.md) remains the target contract; Stage 5 implements its local receiver commit boundary. Stage 6 now has one measured local baseline; the [append attribution](experiments/benchmarks/append-attribution-s0.md) locates most measured time in the two syncs, with a material instrumentation/host-load limitation.

The [WSL recovery audit and resume plan](RESUME.md) records the checks rerun after the interruption. Stage 5 checks pass. [Stage 6's local-log baseline](experiments/benchmarks/local-log-stage6.md) is measured with its [probe](../examples/local_log_probe.rs), one fixed workload, and preserved raw data. The [Homa/SIRD proposal](experiments/ablation/receiver-driven-transport.md) remains a study plan; its [H1 receiver-credit ablation](experiments/ablation/receiver-credit-h1-run-01.md), [M2 one-packet-prefix ablation](experiments/ablation/unscheduled-prefix-m2-run-01.md), and [finite credit/ownership check](experiments/formal/transport-credit-ownership.md) now have results. The simulator is research tooling, not an application transport. Concurrent dashboard work remains separate.

## Implemented

The [storage/query research agenda](experiments/ablation/observability-storage-research.md) corrects the external survey against current code and primary sources. Its [S1 experiment](experiments/ablation/storage-query-s1-run-01.md) measured exact full scans versus optional time/Bloom summaries over immutable replayed events in a separate [research package](../tools/storage-probe/README.md). All 15,360 timed query results matched the reference. Clustered narrow-time queries avoided 93.945% of rows and absent-token queries avoided 100%; shuffled-time queries avoided only 1.172–1.367%. These are in-memory work reductions. The later [S2 disk result](experiments/ablation/durable-snapshot-s2-run-01.md) checks JSON blocks, publication and persistent progress. A separate [layout probe](../tools/layout-probe/README.md) implements hybrid Arrow/Parquet projection and postings, with [measured cost comparisons](experiments/benchmarks/research-costs-run-01.md). This is separate from append-phase attribution and adds no application query service.

The [E1R coverage experiment](experiments/ablation/coverage-e1-run-01.md) adds a research-only
validated summary builder, independently retained snapshot anchor and metadata receipt verifier.
All 1,536 clean results matched the independent positional oracle; 1,536 availability cases and
8,832 metadata-fault cases met their expectations. Two source mutations failed the intended tests,
and cross-family review approved the unchanged implementation. The verifier distinguishes complete,
incomplete and invalid receipts but relies on a correct builder and exact scan executor. It does
not establish raw retention. The later research CLI adds snapshot-bound residual answers;
[overhead measurements](experiments/benchmarks/research-costs-run-01.md) are complete. There is no application query service or durability change.

- The application is a Rust 2024 Cargo workspace: the root `fabric_o11y` package with a library, demo binary and node/CLI binaries, plus `crates/fabric-server`. It depends on `fake` for synthetic workload values and `crc32fast` for FOL2 integrity checks. The local alpha node adds pinned `opentelemetry-proto`, `prost` and `libc` for generated OTLP types and bounded Linux reads. A separate dependency-free [transport simulation package](../tools/transport-sim/README.md) is research tooling.
- Four `u64` ID types, two `i64` time types, five `Scalar` variants, `Attribute`, `Payload::{Log, Gauge}`, and `Event` in [src/lib.rs](../src/lib.rs).
- A deterministic, streaming Gauge generator in [src/generator.rs](../src/generator.rs). `fake` samples ranges with a named seeded ChaCha8 RNG; the locked versions, seed, and `u32` count determine the event sequence. Each event owns its strings and attributes.
- A single-threaded [event buffer](../src/buffer.rs) with a positive logical capacity. Full pushes return the owned event; draining transfers FIFO batches to the caller.
- [src/main.rs](../src/main.rs) retains the default seed/count print demo. Optional `write <PATH> <SEED> <EVENTS>` drains batches into the log, verifies any recovered log is an exact generator prefix, and appends only missing events. `replay <PATH>` opens and prints stored events in a separate process.
- A single-writer [EventLog](../src/log.rs) encodes each event in a 16 MiB capped, versioned frame with separate header and payload CRC32 checks. `append(&Event)` syncs event data, writes a checked commit marker, then syncs again before returning success. `open` locks, syncs the resolved parent directory, validates frame-marker pairs, removes an unmarked final event, and syncs before exposing the recovered prefix. [Storage architecture](architecture/storage.md) records the assumptions and format.
- A [current system view](architecture/system.md), [domain decision](decisions/ADR-0001-keep-domain-independent.md), and [documentation workflow](CONTRIBUTING.md). The JavaScript documentation tools and Python hooks are development tools, outside the Rust application.
- A separate [TLA+ delivery model](../formal/delivery/README.md) and [ADR-0005](decisions/ADR-0005-ack-after-durable-commit.md) describe when a sender may forget an event. [ADR-0006](decisions/ADR-0006-use-framed-local-log.md) records the first storage choice. The model is not executable against the Rust code.

Optional [agent telemetry tooling](architecture/agent-telemetry.md) records typed Codex hook observations and replays JSON snapshots without feeding activity into model context. Its [formal check](experiments/formal/agent-telemetry-merge.md) covers evidence merging and bounded implementation traces, not delivery or filesystem behavior. Hook activation requires runtime trust; the dashboard and structured project-context interface remain planned.

The Rust application has a local append-only event log and process-restart recovery, but no separate upstream sender, network ACK protocol, application query engine, API, UI, or concurrent ingestion mechanism. The research CLI has local query/resume and a strict offline OTLP/JSON Logs adapter, not a network collector. The [packet-slot simulator](../tools/transport-sim/README.md) models H1 and one-packet M2 research cells without real network or disk I/O. The [blueprint](architecture.md) describes other future possibilities.

## Assumptions and unresolved questions

- The generator assigns IDs `1..=events`, one millisecond between synthetic event times, and a ten millisecond observation delay. These values are not sampled from a clock. Other callers of the public event types still supply their own IDs and timestamps; uniqueness and clock ordering are not enforced globally.
- `u64` IDs differ from the blueprint's mostly `u128` examples. No representation experiment or settled identity-generation policy explains that choice.
- Attribute key uniqueness, non-finite gauges, units, and schema validation remain unspecified. Public constructors currently accept such values.
- The generator has one fixed workload shape: one service, one source and resource, four possible tenants, and Gauge durations. Whether it represents real traffic, and any performance targets, remain undefined.
- The buffer bounds queued event count, not total bytes; event size and `VecDeque` allocation overhead remain unconstrained. Accepting an event is temporary in-memory ownership, not a durability acknowledgement. The default print demo remains volatile.
- The local log's success boundary assumes successful file and directory sync calls are honored, all ancestor and symlink names are already durable, the path is not concurrently renamed, and writers respect its advisory lock. Recovery removes unmarked final events with plausible partial headers or markers. It does not decode partial payloads before truncation and cannot distinguish every coherent corruption from a torn tail; CRC32 is not authentication. A full-sized torn payload or marker can fail closed as corruption. A storage writeback or sync error can cast doubt on earlier records. The file alone cannot reveal a prior failed sync, so automatic resume after a storage I/O error is unsafe; rebuild from an independent trusted source on healthy storage. Open scans the whole file; each append performs two file syncs. No performance result selects this format.
- The CLI can reconstruct one deterministic generator sequence on manual restart when no storage I/O error occurred. `EventId` is not globally unique; arbitrary producers still need an independent retained upstream copy and retry/deduplication policy. The delivery model checks safety without a fairness or eventual-progress claim. No physical power-loss test has established durability on any device.
- The transport research cannot yet choose Homa, SIRD, or a receiver-driven feature. The H1 and M2 models have stable synthetic `(producer, sequence)` identities, fixed packet-slot traces, explicit sender/switch byte caps, and sender-window or scheduled-credit controls. M2 adds exactly one uncredited first packet and reports its switch cost; other prefix sizes remain untested. Those research identities and modeled ACKs are not wired into the application. The broader study still needs loss/retry behavior, prefix and credit-budget sensitivity, priority, sender/shared-link feedback, a sink-limited model, and real-host validation. Network CREDIT/GRANT is flow control, not the durable ACK in the delivery model.

## Evidence and next validation

`cargo test --offline --locked` checks the generator, buffer, default CLI, log framing/recovery, and write/replay CLI: 24 integration tests passed in the current Stage 5 run. `env TMPDIR=/home/kmosoti/projects/fabric_o11y/target cargo test --offline --locked --test log --test cli_log` passed all 14 targeted tests with test files on ext4. These are process-restart and injected-corruption checks, not physical power-loss evidence. The [Stage 5 record](experiments/formal/delivery-rust-stage5.md) maps them to the target model.

The [original Stage 3 batch-size probe](experiments/benchmarks/batch-size-stage3.md) used the archived hand-written generator and selects no batch-size winner. The [generator library ablation](experiments/benchmarks/generator-library-stage3.md) measured both variants on the same local pipeline: the `fake` variant's medians were 4.6–11.8% higher across four batch sizes, below the preregistered 25% investigation threshold at size `256`. The trial ranges overlap, and there is no representative throughput target or Rust delivery proof yet. Documentation and hook checks are described in the [contributor guide](CONTRIBUTING.md); they validate repository tooling and syntax, not application semantics.

The [Stage 4 TLC check](experiments/formal/delivery-ownership.md) explored all 64 reachable states of a two-event target model. It found no safety violation under commit-before-ACK and produced counterexamples for ACK-on-buffer. This is a model result, not a Rust delivery proof. Stage 5 maps the local commit and process-restart path to that model. In the first [Stage 6 baseline](experiments/benchmarks/local-log-stage6.md), five 2,000-event release trials on WSL2/ext4 achieved 340.0–394.8 events/s, with 99.965–99.969% of pipeline time inside `EventLog::append`. Those numbers describe one synthetic Gauge workload, not a storage-strategy winner. The [S0 follow-up](experiments/benchmarks/append-attribution-s0.md) now separates encoding, write, and sync time under the same commit boundary, with a material perturbation flag. The [E2R model](experiments/ablation/seal-e2-run-01.md) is checked; its separately registered cost comparison remains research work.

The [first H1 packet-slot result](experiments/ablation/receiver-credit-h1-run-01.md) passes its preliminary simulation gates: p99 burst hot-spot peak falls from 420,000 B under per-sender windows to 13,500 B under a nine-packet global receiver-credit budget at equal completed bytes. Sender queued byte-time rises 55.6% and total queued byte-time rises 1.1%. Its [TLA+ credit/ownership model](experiments/formal/transport-credit-ownership.md) checked 130 finite states and found the expected early-ACK and overgrant counterexamples when those defects were injected. Neither result establishes real-network performance or a durable application ACK.

The [M2 one-packet-prefix result](experiments/ablation/unscheduled-prefix-m2-run-01.md) passes its registered network-limited gates: across ten paired seeds, mean tiny-message p99 completion falls from 22.5 to 14.5 ticks (35.57% paired reduction, bootstrap 95% interval 35.10–36.05%). On the 32-producer incast, p99 burst switch peak rises from 13,500 to 48,000 wire bytes but stays under the registered 65,536-byte guardrail. The ten-seed M1 incast output exactly reproduces all preserved H1 M1 rows. Raw results and hashes are preserved. The next transport question is prefix/budget sensitivity or a scoped sink-limited cell; this result alone cannot justify an application transport.
