# ADR-0006: Use a framed local event log for the first durable receiver

## Status

Accepted for the Stage 5 local prototype. The format is not an interoperability standard or measured performance winner.

## Context

The [Stage 4 contract](ADR-0005-ack-after-durable-commit.md) needs a concrete receiver commit that can be replayed after termination. The current event model has two payload variants and five scalar variants. A bare stream of event bytes cannot identify record boundaries. A complete event frame can also remain visible after a failed sync, so visible bytes alone cannot establish that its append was acknowledged.

## Decision

Keep domain types independent of storage and encode the full `Event` in [src/log.rs](../../src/log.rs). Prefix each payload with a `FOL2` marker, its length, a CRC32 of those header fields, and a separate CRC32 of the payload. Cap one payload at 16 MiB. Write and sync the event frame first. Only after that sync succeeds, write a checked `FOC2` commit marker with the frame-end offset and sync again. Return success only after both syncs succeed. Sync the resolved target file's parent directory when opening. On open, validate complete frame-marker pairs, truncate an unmarked final event with a plausible partial header or marker, and fail on detected corruption in checked header or marker fields or a complete payload. A partial payload is not decoded. Hold an advisory exclusive writer lock.

For the current synthetic CLI, replay and compare the encoded bytes of all stored events with the requested generator prefix before appending. This avoids treating the generator's reused `EventId` values as globally unique identities and preserves distinctions such as `+0.0` versus `-0.0`. It is not a general deduplication scheme.

## Alternatives considered

- **Debug or newline text:** Fast to show, but the current `Debug` output is not a stable parseable record format. Newlines can occur in event strings, and it offers no explicit commit or torn-tail boundary.
- **One raw binary event after another:** Encoding could be compact, but without a frame length and integrity check recovery could misread the tail or skip corruption.
- **One framed event with one sync:** It handles partial tails, but a failed sync can leave a complete frame visible in the page cache without establishing that the frame is durable. If a later open treats those bytes as committed, the synthetic source could skip and forget the event. A marker written only after a successful event sync distinguishes this unacknowledged tail.
- **A general serialization or storage engine now:** This may reduce handwritten codec work, but it would introduce a format and dependency before the project has throughput, schema evolution, or interoperability requirements. No benchmark has compared it with this baseline.

The chosen format is deliberately small. CRC32 detects many accidental changes, not malicious tampering. A separate header checksum prevents an ordinary damaged length from being misread as an incomplete payload and silently truncating a previously committed record. Coherent changes to the length and its checksum, or a CRC collision, remain indistinguishable from a true short tail. A full-sized torn payload can look like corruption; recovery fails closed in that case.

The ordering argument is conditional: if append succeeds, the event sync succeeded before the marker was written, and the marker sync also succeeded. A recovered marker is accepted only after its event frame validates. If the first sync fails, no marker is written. If the marker sync fails, the event data had already synced, but a surviving marker is not proof of its durability: the failed sync may leave visible bytes without a reliable crash-surviving copy. The current log has no persistent record of that prior error, so an automatic reopen cannot safely turn it into a retry decision. This contract assumes no unresolved storage I/O error, successful syncs are honored, and earlier committed data have not been invalidated by storage failure. After a storage I/O error, rebuild from an independent trusted source on healthy storage before resuming. The resolved target's parent is synced, but preexisting ancestor and symlink names must already be durable.

## Evidence

The [implementation tests](../experiments/formal/delivery-rust-stage5.md) exercise all current event variants, reopen and replay, plausible incomplete-header and partial-payload removal, complete unmarked-event removal, partial-marker removal, rejection of inconsistent checked header/marker fields and complete corruption, an oversized record, the writer lock, and synthetic-source resume. They do not test physical power loss. The [Stage 6 baseline](../experiments/benchmarks/local-log-stage6.md) measures one synthetic local workload; it is not a format comparison or a general throughput guarantee. The [storage view](../architecture/storage.md) maps the append and recovery operations to the model.

## Consequences

The receiver has an explicit local commit point, and recovery can preserve validated pairs while removing an unacknowledged tail. Each append pays for two file syncs and a marker. Open scans the whole file. Format changes require version handling; compaction, indexing, authentication, and a general duplicate policy remain future work. `EventLog::open` needs read and write permission even for replay because it may repair a tail.

## Validation

Run `cargo test --offline --locked --test log --test cli_log`, then inspect the file through `cargo run --offline -- write <PATH> 42 3` and a separate `cargo run --offline -- replay <PATH>`. A recovered prefix differing from the original committed pairs, an ACK-like success before the marker sync, acceptance of an unmarked event, or silent acceptance of detected corruption would falsify the chosen boundary under its assumptions. The tests do not establish recovery safety after a storage sync error.

## Related

- [Delivery architecture](../architecture/delivery.md)
- [Storage architecture](../architecture/storage.md)
- [Event concept](../concepts/event.md)
