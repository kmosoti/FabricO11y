# Resume after the WSL interruption

Recovery audit: 2026-09-26. The first section records what survived the interruption; the resume sequence below tracks subsequent work, including the first [Stage 6 measurement](experiments/benchmarks/local-log-stage6.md). See [current state](CURRENT.md) and the [learning path](LEARNING_PATH.md).

## Where work stopped

- **Stages 1–3 are implemented:** typed events, a repeatable `fake` generator, and a bounded FIFO buffer.
- **Stage 4 has a checked target model:** commit must precede ACK; the model includes expected counterexamples for early ACK.
- **Stage 5 is implemented:** the local log, framing and checksums, two syncs per append, recovery, and CLI write/replay/resume paths are present in [the source](../src/log.rs) and [tests](../tests/log.rs).
- **Stage 6 has not started.** The existing [batch probe](../examples/batch_probe.rs) measures the generator/buffer path without storage. It is not a durable-log baseline.
- **The Homa/SIRD research plan is saved.** The [proposal](experiments/ablation/receiver-driven-transport.md) contains the latest baseline, calibration, queue, goodput, and attribution revisions. The previous turn was interrupted during final independent review; final acceptance of those revisions was not recorded. No transport simulator, protocol implementation, or ablation result exists.

The agreed order remains: research plan now, local Stage 6 baseline next, transport ablations afterward. Optional agent telemetry work is a separate track and is not a prerequisite for that sequence.

## What was verified after restart

The workspace is on ext4 under WSL2. Available tools report Rust/Cargo 1.98.0 and Bun 1.4.0. The cached TLC jar and Java runtime are present.

| Command | Observed result |
| --- | --- |
| `cargo test --offline --locked` | Exit 0; all 24 integration tests passed |
| `bun tools/docs/check.mjs` | Exit 0; documentation checks passed |
| `bun tools/docs/check.test.mjs` | Exit 0; 40/40 acceptance probes passed |
| `python3 -B tools/docs/test_hooks.py` | Exit 0; 10 tests passed |
| `TLA_JAR=/home/kmosoti/.cache/fabric_o11y/tla/tla2tools-v1.7.1.jar JAVA_BIN=/home/kmosoti/.cache/fabric_o11y/tla/jdk-21.0.12.1+1-jre/bin/java bash formal/delivery/check.sh` | Exit 0; safe model passed and both expected early-ACK counterexamples were found |
| `git diff --check` | Exit 0 for tracked changes |

These checks establish that the surviving source builds and its checked contracts still hold. They do not prove the cause of the WSL crash, completeness against an unavailable pre-crash filesystem snapshot, physical power-loss durability, or the integrity of an arbitrary existing event-log file. No existing user log was opened or repaired during this audit.

## Resume sequence

1. **Scoped Git checkpoint created after review.** At the interruption audit, Git had only `3966405` (`Initial Commit`), and most source, tests, documentation, formal models, and tooling were untracked. The subsequent checkpoint captures the core project and the Stage 6/H1 evidence while excluding caches, build products, and the intentionally concurrent dashboard and `.superdesign` paths. The original audit itself did not stage or commit anything.
2. **Transport plan review closed on 2026-09-26.** A read-only adversarial reviewer approved the revised [proposal](experiments/ablation/receiver-driven-transport.md) after a probe showed that its 95% injection-interval completion floor rejects a misleading low-throughput result. This approves a study plan, not a transport or a measured result. Exact topology and tuning-grid values remain for the later runnable experiment registration, before measurements.
3. **Stage 6a registered.** The [local-log baseline](experiments/benchmarks/local-log-stage6.md) fixes the seed, event count and shape, buffer capacity, batch size, repeat count, ext4 host, release build, and timing boundaries. It keeps the two-sync append contract and requires exact separate-process replay. Changing sync behavior would change the durability comparison.
4. **Stage 6b probe implemented and checked.** The [example](../examples/local_log_probe.rs) measures successful append latency, pipeline throughput, file bytes, open/recovery time, and verified replay with per-event console output outside timing. It uses a fresh file for each trial. A feature-gated allocator counts requests in a separate build; `VmHWM` provides whole-process peak memory. Smoke cases, wrong-input probes, and an injected append failure passed independent review. The [runner](../tools/bench/run_local_log_stage6.sh) fixes the trial sequence used for the first result.
5. **Stage 6c first baseline recorded.** The [experiment](experiments/benchmarks/local-log-stage6.md) preserves raw CSV, commands, source hashes, and environment. It reports p50/p99 append latency, throughput, bytes per event, memory, allocations, recovery observations, and variation between repeats. The next question is how append time divides among encoding, event write/sync, and marker write/sync; no storage or transport winner is established.
6. **First transport-model cell executed.** The [finite model](experiments/formal/transport-credit-ownership.md) checks credit and ownership safety under stated bounds and has injected early-ACK and overgrant counterexamples. The [H1 packet-slot experiment](experiments/ablation/receiver-credit-h1-run-01.md) fixes stable synthetic identities, a byte trace, caps, and M0/M1 scheduling, then runs ten paired seeds with raw results. It models immediate receiver commit and no loss, so retry under loss, priority/active grants, sender/core feedback, sink-limited service, and real-host validation remain in the [broader track](experiments/ablation/receiver-driven-transport.md).

The next local learning increment is **Stage 6's append-phase investigation**. The next transport increment is a separately registered mechanism cell with the H1 model's limits addressed. The recovery audit itself made no application change; the subsequent Stage 6 and H1 experiments are linked above.
