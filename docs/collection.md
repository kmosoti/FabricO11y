# SDK and Spindle local collection

Spindle is a Linux user service for local durable observation custody. Rust
applications use `fabric::sdk`; other harnesses keep one `fabric-adapter` process
open and send JSONL commands. New collection writes typed v2 observations; legacy
prototype APIs and `observation::read_legacy` preserve v1 without inventing units.
See the [SDK](contracts/sdk.md) and [collector](contracts/spindle.md) contracts.

## Install and configure

```sh
cargo build --release -p fabric --bins
install -Dm755 target/release/spindle ~/.local/bin/spindle
install -Dm755 target/release/fabric-adapter ~/.local/bin/fabric-adapter
install -Dm644 config/spindle.service ~/.config/systemd/user/spindle.service
install -Dm600 config/spindle.json ~/.config/fabrico11y/spindle.json
```

Edit every example path and UID before starting. Configuration uses absolute
paths without environment or tilde expansion. The service creates private runtime
and state parents. For direct `spindle serve CONFIG`, create those parents first
with mode 0700. Storage creates exactly one directory below an existing parent and
synchronizes both directories before accepting submissions.

```sh
systemctl --user daemon-reload
systemctl --user enable --now spindle.service
spindle status /run/user/$(id -u)/spindle/spindle.sock
spindle export /run/user/$(id -u)/spindle/spindle.sock > observations.jsonl
```

`status` remains readable at quota exhaustion. `read SOCKET [AFTER]` returns at
most one record and its increasing local ID; repeat with that ID for pagination.
`export` captures the durable high watermark, then paginates that finite prefix
to JSONL while collection may continue. `stop SOCKET` is a graceful local shutdown. Service
access uses the same UID, a private socket parent and a mode 0600 socket.

An optional file entry is `{"id":"application","path":"/absolute/app.log",
"format":"text","replay":false}`; `jsonl` accepts bounded JSON objects and
preserves their fields in `json_log`. Only regular files are accepted; symbolic
links and FIFOs are refused. New sources start at the current end. Set `replay`
explicitly to collect existing contents. Source IDs must be unique and stable.
Offsets refer to source bytes; incomplete lines wait. Oversized, malformed or
invalid records produce explicit gap records. Renamed files drain their open
descriptor before the replacement starts; truncation or changes behind the last
offset produce a gap. Restart with a different inode records the missing coverage
before reading the replacement. File identity uses device/inode and a persisted
32-byte tail fingerprint; undetectable inode reuse with an identical fingerprint
is a remaining filesystem limitation.

An optional journal entry is `{"unit":"application.service","system":false,
"replay":false,"include_self":false}`. Only journals accessible to the user are
read. A saved cursor is checked against the first returned entry; absence records
a gap before advancing. Empty initial attachment persists a time boundary.
Permission failures and missing journal capabilities appear in source status.
Spindle's identifier and unit are excluded unless explicitly enabled. A bounded
`journalctl` subprocess has a 250 ms deadline, a 256 KiB output limit and is reaped.
Persistent oversized native journal entries may pause the source with an explicit
status rather than skip an unknown cursor.

Optional `host` sampling reads bounded `/proc` snapshots and configured filesystem
capacity. CPU units are kernel ticks, disk sectors are converted using Linux's
512-byte accounting unit, and memory/network/capacity use bytes. Counters are raw,
not rates; first samples, boot changes and decreases set `reset`. The producer
generation includes the boot ID. Missed or partially collected intervals are
reported as gaps on recovery and never backfilled. Polling bounds source work;
health is maintained in memory and does not recursively collect itself.

## Admission and custody

`try_record` reserves one preallocated payload slot and performs bounded validation
and encoding. It returns explicit acceptance/refusal and never waits on disk or
transport. Caller-owned data and process/allocator overhead are outside the SDK
payload reservation. The one delivery worker retains the charged slot until a
matching durable receipt arrives. `flush(Instant)` snapshots the accepted sequence
frontier and waits for custody through that frontier. A timeout has an unknown
outcome: retry the same identity and bytes. Drop closes admission immediately;
pending memory data requires a successful flush before dropping the client.

Wire submissions are a big-endian u32 length followed by raw v2 JSON. Input is
limited to 64 KiB and 16 nesting levels before owned deserialization. Control
frames use `command` (`status`, `read`, `stop`). Responses are bounded to input
size plus 8 KiB; only one read record is returned per request. Every connection has
an absolute deadline. New SDK generations use the kernel UUID source.

SQLite commits the record, retry identity and source checkpoint in one
transaction. DELETE journaling, synchronous EXTRA, a bounded page count and an
explicit reserve account for rollback recovery. At least half the configured
quota is reserved for rollback and metadata overhead. Existing larger databases
are refused. No automatic eviction, forwarding or retention is performed.
Receipts imply a successful local filesystem synchronization, subject to the
filesystem and storage device honoring that operation; they do not imply remote
replication. Clear or migrate custody only with the service stopped. Preserve the
whole custody directory when making an offline backup.

Producer names `host`, `file:*` and `journal:*` are reserved for collector sources.
Identity is `(producer,generation,sequence)`. A duplicate succeeds only for the
exact same submitted bytes; changed serialization or payload is a conflict. A
new SDK process creates a new generation; applications requiring retry across
process death must persist the complete original envelope themselves. Registry
descriptors are retained and compared exactly under their registry ID.

## Harnesses and adapter

Run `cargo run -p fabric --example agent_harness -- SOCKET`, or
`FABRIC_ADAPTER=target/debug/fabric-adapter python examples/agent_harness.py SOCKET`.
The [generated tool examples](../examples/generated-tool/README.md) use the same
client or adapter instance as their harness.

The adapter accepts `record` with typed `data` and `timestamp_ns`, `flush` with
`timeout_ms` (capped at 30 seconds), and `status`. Every command returns one JSON
line. Inputs are bounded before decoding; oversized lines are rejected and close the adapter after its bounded final flush.
EOF triggers one additional flush with a two-second deadline and a structured
`eof_receipt` or error reply. Diagnostics use stderr. A successful memory admission
alone is never a durable receipt.

The existing admission kernel proofs remain scoped to the legacy bounded kernel;
they do not prove SQLite, transport, SDK worker or source adapter correctness.
