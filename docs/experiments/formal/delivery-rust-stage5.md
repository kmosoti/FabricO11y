# Stage 5 local delivery implementation checks

## Claim and scope

Under a filesystem that honors successful file and directory sync, with durable ancestor path names and no unresolved storage I/O error, a local `EventLog::append` success leaves one recoverable event frame with a valid commit marker. An unmarked final event with a plausible partial header or marker is removed on reopen; detected damage in checked header or marker fields or a complete payload is reported as an error. This is a receiver-side implementation claim, not a proof of network delivery or physical power-loss safety.

## Model-to-code mapping

| Stage 4 model | Stage 5 operation |
| --- | --- |
| `Receive` | [`EventBuffer::try_push`](../../../src/buffer.rs) holds an owned event in memory. |
| `Commit` | [`EventLog::append`](../../../src/log.rs) writes and syncs the event frame, then writes and syncs its commit marker. |
| `Acknowledge` | The [`write` command](../../../src/main.rs) prints `committed event N` after append succeeds; no network ACK is sent. |
| `Forget` | The local event value is dropped after successful append. The same seed/count can reconstruct the synthetic upstream source on manual restart. |
| `ReceiverCrash` | A separate process reopens and replays the file; tests also inject incomplete tails. |

The model assumes a stable upstream copy and unique event identity. The Rust CLI supplies only a deterministic generator prefix, not a general independent sender. It compares entire recovered events rather than trusting reused `EventId` values.

## Setup and method

The checked workspace used Rust `1.98.0` on Linux `6.18.33.2-microsoft-standard-WSL2` x86_64. `Cargo.lock` pins `fake 5.1.0` and `crc32fast 1.5.2`. The repo is on ext4; `/tmp` is tmpfs. The independent integration tests use `std::env::temp_dir()`, so the targeted command below sets `TMPDIR` to the repo's `target` directory to exercise the ext4 path.

Run from the repository root:

```sh
env TMPDIR=/home/kmosoti/projects/fabric_o11y/target cargo test --offline --locked --test log --test cli_log
cargo test --offline --locked
```

The tests append and reopen all current domain variants, check floating-point bit preservation, inspect the frame and commit-marker layout, inject short headers and bodies, remove complete unmarked events and partial markers, flip bits in complete frames and markers, reject inconsistent partial length, header checksum, marker offset, and marker checksum fields, reject an oversized event, check that a second writer cannot acquire the lock, and run the CLI in separate write/replay processes. The CLI tests verify exact-prefix resume, no duplicate append on identical rerun, rejection of `-0.0` as a prefix for generated `+0.0`, and refusal of a different seed or smaller count. These are distinct representative traces, not exhaustive crash-state exploration.

## Results

The targeted ext4 command exited `0`: 12 log tests and 2 CLI tests passed. The full test command exited `0`: 24 integration tests passed across the existing buffer, generator, and CLI checks plus the new log checks. The corruption tests deliberately inject foreign short data, an undersized or oversized partial length, bad partial header checksum, bad partial marker offset/checksum, bad magic, an increased complete-header length that would otherwise look like a short tail, a changed payload, and a bad complete marker. Each causes `EventLog::open` to fail while leaving file bytes unchanged. Genuine short final frames, complete unmarked events, and short markers are truncated to the exact prior committed prefix.

A manual run used a fresh path. `cargo run --offline --locked -- write target/fabric-stage5-demo.SAyIfU/events.log 42 3` exited `0` and printed commits `1`, `2`, and `3`; `cargo run --offline --locked -- replay target/fabric-stage5-demo.SAyIfU/events.log` exited `0` and printed three events. Repeating the write exited `0` with `already committed 3 event(s)` and no new commit lines.

An adversarial review found that opening a dangling symlink could create a target file while syncing only the symlink's parent directory. The implementation was changed to resolve the path after opening and sync the target file's parent. Its Linux `LD_PRELOAD` fsync probe then observed a sync of the target `data/` directory before file sync and the `committed event 1` line. This supports the filename boundary when the pathname remains stable.

The same review constructed a previously committed frame whose length and complement were both changed consistently from 116 to 117. The earlier reader mistook it for a short final frame and truncated the log. The header now stores CRC32 over marker and length instead of the complement, and checks that CRC before testing whether the payload is short. The revised regression test increases only the recorded length and requires open to fail without truncation. This does not protect against coherent adversarial edits that also recompute the CRC.

The review also showed that a stored Gauge `-0.0` was accepted as a prefix for the generator's `+0.0` because derived `Event` equality uses floating-point numeric equality. `write` now compares canonical encoded bytes. A new CLI test stores `-0.0` for seed `98`, then requires the `+0.0` rerun to fail without changing the log.

A cross-model review found that the first reader silently truncated a detectably foreign file shorter than 16 bytes, and that a complete frame left after a failed event sync could be mistaken for an acknowledged record on reopen. The short-tail parser now checks available magic bytes, complete lengths, available header-checksum bytes, and available marker bytes; it does not decode partial payloads. The log now writes a checked `FOC2` marker only after event-data sync succeeds, then syncs that marker before returning success. New tests require a complete unmarked frame and a matching partial marker to disappear on recovery, while short headers or markers inconsistent with checked fields fail without truncation. This checks the observable recovery states, not kernel behavior under an actual fsync failure.

An independent probe then supplied four inconsistent short tails with enough bytes to judge: an oversized `FOL2` length, a wrong `FOC2` offset byte, a wrong `FOL2` header-checksum byte, and a wrong `FOC2` marker-checksum byte. The prior reader truncated each. After the stricter checks, all four `replay` commands exited `1` and preserved the input bytes. Three undersized lengths (`0`, `1`, and `56`) likewise now exit `1` without truncation; `57` is the smallest encodable event payload. The reviewer also tried all 297 byte prefixes of a valid two-record file; every prefix recovered exactly zero, one, or two complete records as appropriate. This is a finite prefix probe of one file, not a proof for every event shape.

A final review pointed out a deliberate parser limit: with a valid 16-byte header promising a 57-byte payload, a 56-byte partial payload is truncated without decoding its present fields. We confirmed this with a payload containing an invalid tag at payload offset `52`. The test file was 220 bytes before `replay`; the command exited `0`, printed the one previously committed event, and left a 148-byte file. This behavior is documented as partial-payload recovery, not corruption detection; it cannot be used to prove every truncated tail came from this writer.

For a mutation check, an isolated scratch copy disabled complete-marker offset and checksum rejection. `cargo test --manifest-path /tmp/fabric-stage5-defect.cm6w83/Cargo.toml --offline --locked --test log foreign_short_tail_and_bad_complete_marker_are_not_silently_removed` exited `101` at the bad-marker assertion. That was the test's name before the later short-tail cases were added. The repository source was not changed by the mutation.

## Interpretation and limits

The tests support the stated process-restart and parser behavior. `File::sync_all` attempts to persist data and metadata; these tests cannot verify device cache behavior, sudden WSL or machine power loss, or guarantees of every filesystem. After an ambiguous **marker** sync error, a valid marker may remain visible even when its durability is unknown. Reopen cannot identify the earlier failed sync from file bytes alone. The CLI's automatic prefix resume is therefore scoped to runs without a storage I/O error; after one, rebuild the log from an independent trusted source on healthy storage. A full-sized torn payload or marker may look like corruption; recovery fails closed instead of guessing that it is unacknowledged. A writeback error might affect earlier data too, beyond this per-event model. The file lock is advisory and assumes other writers honor it; concurrent pathname rename is out of scope. The resolved target's parent is synced, but newly created ancestor directories and symlink entries require their own durability before use. CRC32 is not authentication, and coherent corruption can imitate a short tail. The format has a 16 MiB record cap, two file syncs per event, and a linear reopen scan. No throughput, latency, or recovery-time measurements were made. [Stage 6](../../LEARNING_PATH.md#stage-6--measure-and-challenge-the-baseline) will register a workload and metrics before comparing storage strategies.

## Decision impact

The local result supports the receiver commit point in [ADR-0005](../../decisions/ADR-0005-ack-after-durable-commit.md) under the stated assumptions and documents the first format in [ADR-0006](../../decisions/ADR-0006-use-framed-local-log.md). It does not establish the Stage 4 invariant for an arbitrary network sender. See the [storage architecture](../../architecture/storage.md) and [TLA+ investigation](delivery-ownership.md).
