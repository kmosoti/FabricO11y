# Journal reclaim: local run 01

Status: local mechanism and server regression tests passed. The full fast profile failed its existing Clippy gate; its other 19 checks passed. This is a correctness/progress investigation under the [registered protocol](journal-reclaim-local-protocol.md), not a performance or target-host qualification result.

## Revisions and environment

Baseline: `5f0c52f16f8afe01cf29a118513cf8a43b4cb333`. Protocol registration: [`9df2e17`](https://github.com/kmosoti/FabricO11y/commit/9df2e17764c8d02fcb9397dfecc3f19c541244e9) (published equivalent of local registration `0e2d3f0`, committed before source changes). Candidate implementation: [`9c4713c`](https://github.com/kmosoti/FabricO11y/commit/9c4713c735314cac962ce850500efa09013ccd88). Candidate source SHA-256 for `crates/fabric-server/src/sealer.rs`: `432d676986206d81cc1e8f70ffd04a76cb395d09c1c9253debf794ff4e73ec96`.

Debian 13, x86_64, glibc 2.41, kernel 6.18.44; five visible CPUs, cgroup quota four CPU equivalents and 16 GiB memory. Workspace uses overlayfs. Rust 1.98.0, two build jobs, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`; shared build cache outside both worktrees. Bun 1.4.0 for documentation checks. Native processes only. No PC access, Docker, Wasm, service installation or release.

## Mechanism and controls

Six tests cover the seven registered cases (some tests cover several cases): prepublished prefix, blocked later group with one/two workers, build failure and panic, successful prefix within a failed group, reclaim error and retry, and a real journal/Segment checkpoint/reopen fixture. The latter uses five small valid Batches, a 1 KiB rotation threshold and a 16 MiB journal ceiling. A directory occupying `streams.json.tmp` forces checkpoint creation to fail before deletion; removing it permits retry. Exact retained bytes, duplicate ACK and conflicting retry are checked after reopening.

The first fixture attempt was rejected by Batch validation because one collection-gap string exceeded the existing 256-byte cap (5 tests passed, 1 failed). The fixture was corrected to eight 224-byte strings. No product validation limit changed.

The deferred-reclaim mutant returned an empty prefix while the later group was blocked, where `[1]` was required (exit 101). The cross-hole mutant reclaimed `[1, 2, 3, 4]` where only `[1]` was permitted (exit 101). Both controls were removed, and the source hash above was verified before regression tests. The controls and scheduler tests share their authorship; they are not an independent oracle or a proof. Existing history tests supply independent Python query-oracle coverage.

## Checks

Commands ran with the toolchain environment above; candidate test and lint invocations used an external `timeout 900`. Baseline server tests completed before source changes (the initial baseline invocation did not have that external timeout; it completed normally).

| Scope | Command | Exit and evidence |
| --- | --- | --- |
| Baseline server | `cargo test -p fabric-server --lib --tests --locked` | 0; 21 tests, [output](data/journal-reclaim-local-run-01/baseline-tests.txt) |
| Focused candidate | `cargo test --locked -p fabric-server --lib sealer::tests` | 0; 6 tests, [output](data/journal-reclaim-local-run-01/targeted.txt) |
| Candidate server | `cargo test --locked -p fabric-server --lib --tests` | 0; 27 tests, [output](data/journal-reclaim-local-run-01/candidate-tests.txt) |
| Deferred control | Same focused command with `sealer::tests::completed_group_reclaims_while_later_group_is_blocked` | 101, expected rejection; [diff](data/journal-reclaim-local-run-01/deferred.patch), [output](data/journal-reclaim-local-run-01/deferred.txt) |
| Cross-hole control | Same focused command with `sealer::tests::failure_or_panic_never_reclaims_across_a_hole` | 101, expected rejection; [diff](data/journal-reclaim-local-run-01/cross-hole.patch), [output](data/journal-reclaim-local-run-01/cross-hole.txt) |
| Baseline workspace lint | `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | 101; existing `chunks_exact_to_as_chunks` at `fabric-observation/src/crc32.rs:81`, [output](data/journal-reclaim-local-run-01/baseline-clippy.txt) |
| Candidate server lint | `cargo clippy -p fabric-server --all-targets --all-features --locked --no-deps -- -D warnings` | 101; same lint at unchanged `fabric-server/src/text_filter.rs:153`, [output](data/journal-reclaim-local-run-01/server-clippy.txt) |
| Baseline server lint | Same server lint command | 101 at the same line, [output](data/journal-reclaim-local-run-01/baseline-server-clippy.txt) |

`timeout 900 cargo xtask checks --profile fast` exited 1: 19 checks passed and Clippy failed at the baseline CRC line above. [Runner output](data/journal-reclaim-local-run-01/fast.txt) and [workspace-test receipt](data/journal-reclaim-local-run-01/receipts/test.json) retain the results; all 20 receipts are in that directory. The passing checks include workspace tests, independent Python oracles, property tests, corpus replay, simulation, architectural boundaries, documentation and counterexample registration. The clone was unshallowed so historical counterexample fix commits were available.

Checks ran on the candidate working tree, whose only runtime-code change is the sealer source hash above. Receipts correctly mark it dirty and name the planning commit present when each check ran (`0e2d3f0` or its equal-tree published equivalent `9df2e17`); these are not claims about pristine planning commits. Documentation was updated during the run and checked again after final edits. Stored text output has trailing whitespace and excess final blank lines removed; control diffs use zero context. Receipt JSON is unchanged. No lint was suppressed and no verification policy changed. The shared build cache reached about 2.7 GiB, the toolchain/cache 804 MiB; workspace free disk remained above 26 GiB at that checkpoint, well above the 4 GiB floor. This is build storage, not measured server RSS.

### Native PC smoke procedure

Use Rust 1.98.0 and the branch source in an ordinary user checkout. Cargo builds the native test processes; OpenSSL and Python 3 must be on `PATH` for the integration fixtures.

```sh
cargo test --locked -p fabric-server --lib sealer::tests
cargo test --locked -p fabric-server --lib --tests
```

These are correctness smoke checks, not a benchmark harness. Record commit SHA, toolchain version, command exits and OS/filesystem before any performance comparison.

## What remains for the PC

The mechanism returns an eligible prefix earlier in the deterministic schedule. No elapsed-time speedup, disk-pressure reduction, ACK p99, RSS bound or query-freshness gain was measured. Extra checkpoint scheduling may contend with ingest. Keep the candidate experimental until the [mixed-load comparison](../../milestones/journal-reclaim-progress.md#draft-comparison-protocol) is registered and executed.

Use the frozen baseline and candidate source with one toolchain on the target PC. First record OS/architecture, filesystem, CPU/RAM limits and free disk; run the focused/server tests from unprivileged owned scratch directories. Build on the PC if its glibc is older than this environment's. Hardware work remaining: paired mixed-load trials measuring offer-to-ACK latency including refused/retried offers, duplicate byte-time and capacity-blocked duration, sustained throughput and long-run memory. Real-filesystem crash/power-loss claims need separately scoped fault experiments. Systemd acceptance, if requested later, belongs on a disposable VM; do not run the container-specific installation acceptance script on the everyday PC. No installation is needed for the native benchmark.
