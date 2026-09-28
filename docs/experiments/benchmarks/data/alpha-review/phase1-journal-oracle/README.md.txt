# Independent alpha journal oracle

This standalone package tests only `fabric_o11y::alpha::journal::{Batch, Cursor, Journal}` through the supplied public API. It was authored from `docs/ALPHA.md` and the user-supplied API/contract before compilation or execution against the alpha implementation. No alpha implementation source or compiler source excerpts are inputs to the oracle.

## Assumptions fixed before execution

- `Journal::open` creates or reopens a journal in a fresh owned directory. Tests use one journal per directory, except the intentional second-open lock probe.
- `identity()` returns a durable 16-byte node ID and a nonzero stream generation; the node ID is nonzero and independently generated for separate directories. `next_sequence()` returns the sequence assigned to the next successful append. Its initial numeric value is unspecified.
- `append` replaces the caller's `node_id`, `generation`, and `sequence` with canonical durable values, preserving `version`, both exact OTLP byte strings, cursors, and collection gaps. `Batch::validate()` accepts a returned committed batch and rejects malformed protobuf wire bytes. Version `1` is the proposed envelope version.
- The valid metrics and logs fixtures are encoded `ExportMetricsServiceRequest` and `ExportLogsServiceRequest` messages from pinned `opentelemetry-proto=0.33.0` and `prost=0.14.4`. Their resource, timestamp, value, and body fields carry real content. A valid unknown protobuf field tests byte preservation across append; no semantic interpretation of that field is required.
- A failed validation, size, or full-capacity append leaves committed sequence and `used_bytes()` unchanged. A full journal does not advance a source cursor because no batch carrying that cursor is committed. The 8 KiB limit is an actual journal byte cap, and the 1 MiB batch limit applies to the encoded `Batch` after canonical identity fields are assigned.
- Tests assume successful filesystem sync calls and a stable, intact independent ancestor namespace. They establish process reopen behavior, not power-loss durability.
- The single-writer rule applies even to two `Journal` handles in one process. Lock release occurs when the first handle is dropped.
- Failed input validation and capacity rejection are not storage I/O errors and need not quarantine the writer.

## Checks and expected oracle

Six integration tests check persistent identity and sequence, independent directory identity, canonical assignment and content preservation, complete OTLP wire-byte preservation, malformed OTLP rejection, encoded-size rejection, byte-cap rejection, and advisory writer exclusion. After rejection, the observable next sequence and committed used bytes remain unchanged; after restart they remain so. Test data are created only under `data/` and removed on normal test completion. The oversize fixture is in memory, so retained data stay well below 5 MiB.

The oracle deliberately does not infer private filenames, record framing, replay behavior, or FOL2 compatibility from implementation. The public API given here cannot inspect persisted OTLP bytes after reopening; the exact-byte check is on the successful `append` return value. It also cannot directly observe data-sync/marker-sync ordering, known-I/O-error quarantine, or partial-tail recovery.

## Proposed external probes requiring an integrator seam

1. Freeze the on-disk format or expose a test-only abstract I/O fault seam. In a fresh owned directory, inject a short write or failed data sync before marker commit. Assert `append` errors, the current writer refuses further appends, and reopening reports recovery required without exposing the candidate as committed. Retain an independent source copy for recovery. A complete readable frame alone must not clear known-error authority.
2. Inject a marker-sync failure after bytes are written, including a failure while clearing any recovery-required sidecar. Assert that opening still requires explicit recovery and does not silently treat readable bytes as authority. Record exact syscall failure point and return values. This probe requires a fault seam capable of distinguishing the two syncs.
3. Freeze a format-independent replay/inspection API, then compare persisted payload bytes and cursors with the original OTLP fixture after close/reopen; corrupt a committed payload byte and require fail-closed open/replay. With a frozen format, also create an incomplete final append and require the uncommitted batch remain invisible.
4. For FOL2 compatibility, use a preexisting legacy fixture and its public `EventLog` interface to verify unchanged replay before and after an alpha journal is created in a separate directory. This requires an agreed fixture; no alpha test should reinterpret legacy bytes.

None of these proposed probes is counted as executed or passed. Server retry/dedup, physical power loss, and performance are outside this local journal oracle.

## Reproduction

From this directory, with the source package at `../..`:

```sh
CARGO_HOME=/home/kmosoti/projects/fabric_o11y/target/alpha-cargo \
CARGO_TARGET_DIR=/home/kmosoti/projects/fabric_o11y/target/alpha-oracle-build \
cargo test --offline --locked --test journal_contract
```

The test source SHA256 is frozen separately in `SHA256SUMS` before this command is first run. Compiler diagnostics are captured to a local file and summarized without displaying alpha implementation source excerpts.
