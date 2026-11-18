# SDK and Spindle local collection

Spindle is a Linux user service for local durable observation custody. Rust
applications use `fabric::sdk`; other harnesses keep one `fabric-adapter` process
open and send JSONL commands. New collection writes typed v2 observations; legacy
prototype APIs and `observation::read_legacy` preserve v1 without inventing units.
See the [SDK](contracts/sdk.md) and [collector](contracts/spindle.md) contracts.

## Install and configure

Build the production executables and runnable harness, then install for the current
Linux user. Installation resolves `HOME`, `XDG_RUNTIME_DIR`, `XDG_CONFIG_HOME`
and `XDG_STATE_HOME` (the latter two default under the home directory), creates
private collection directories and writes an SDK-only configuration. Existing
files must match exactly; conflicts report the path and require explicit backup
or relocation. Custody is never replaced. Repeating an identical installation is
safe. Installation does not activate the service.

```sh
CARGO_BUILD_JOBS=2 cargo build --release --workspace --bins --examples
cargo xtask collection smoke
cargo xtask collection install
systemctl --user daemon-reload
systemctl --user enable --now spindle.service
~/.local/bin/spindle status "$XDG_RUNTIME_DIR/spindle/spindle.sock"
cargo xtask collection smoke --socket "$XDG_RUNTIME_DIR/spindle/spindle.sock"
```

The isolated smoke starts the actual Spindle executable, runs Rust and Python
harnesses through the SDK and persistent adapter, checks exact exported records,
restarts the daemon against the same custody and verifies recovery and continued
submission. Its processes and storage are temporary. `--socket` checks an existing
idle SDK-only service by adding four records; it never stops that service or deletes
its records. For installed-service restart acceptance, export before restarting,
run `systemctl --user restart spindle.service`, compare the export after restart,
then run the socket smoke again. A successful smoke is JSON with `passed: true`.
Use `--bin-dir /absolute/path` to select another built binary directory; it must
also contain `examples/agent_harness`. Both modes have bounded child deadlines.

## Coupled stack installation (`Spindle` + `fabric-server` + `fabric-daemon`)

For the full production telemetry and canonical custody pipeline, install all three user services inside `fabrico11y.slice` using `install-stack`:

```sh
cargo build --locked --release --workspace --bins --examples --target-dir /data/cargo-target -j 4
/data/cargo-target/release/xtask collection smoke --bin-dir /data/cargo-target/release
/data/cargo-target/release/xtask collection install-stack \
  --bin-dir /data/cargo-target/release \
  --data-dir /data/fabrico11y \
  --tmp-dir /data/tmp \
  --listen 127.0.0.1:17440
systemctl --user daemon-reload
systemctl --user enable --now fabric-server.service spindle.service fabric-daemon.service
/data/cargo-target/release/xtask collection smoke \
  --bin-dir /data/cargo-target/release \
  --socket "$XDG_RUNTIME_DIR/spindle/spindle.sock"
fabric custody --json
fabric doctor --json
```

`install-stack` validates non-`/tmp` paths (including XDG layout paths) and a loopback non-zero `--listen` address, installs the six release executables (`fabric`, `spindle`, `fabric-server`, `fabric-adapter`, `fabric-query`, `fabric-mcp`), provisions distinct `0600` credentials (`spindle-node.token`, `fabric-reader.token`), binds Spindle's persistent `origin` and `fabric-server`'s canonical `authority` inside `server.json` and `spindle.json` (`reclaim_confirmed: false`), and writes rootless-safe systemd user units (`fabrico11y.slice`, `fabric-server.service`, `spindle.service`, `fabric-daemon.service`). `fabric-server.service` and `spindle.service` use `Type=notify` (`READY=1` after socket bind); `fabric-server` and `fabric-daemon` handle `SIGINT`/`SIGTERM` gracefully while `spindle.service` stops gracefully via `ExecStop=spindle stop`, so ordered startup and shutdown work cleanly across all three units. Repeating an identical `install-stack` run is idempotent (both offline and while services hold active custody locks); any conflicting pre-existing file fails preflight before mutating custody state and must be explicitly backed up before retry. Note that `fabrico11y.slice` has no `[Install]` section—enable only the `.service` units, which activate `fabrico11y.slice` automatically via `Slice=fabrico11y.slice`.

Configuration uses absolute paths without environment or tilde expansion. Optional
collectors require explicit configuration. For direct `spindle serve CONFIG`,
create private parent directories first. Storage creates exactly one directory
below an existing parent and synchronizes both directories before submissions.

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

Wire submissions are a big-endian u32 length followed by original v2 or v3 JSON. Input is
limited to 64 KiB and 16 nesting levels before owned deserialization. Control
frames use `command` (`status`, `read`, `stop`, `lease`, `fabric_progress`). Responses are bounded to input
size plus 8 KiB; only one read record is returned per request. Every connection has
an absolute deadline. V2 generations use the kernel UUID source; v3 generations
are authenticated leases issued by Spindle.

SQLite commits the record, retry identity and source checkpoint in one
transaction. DELETE journaling, synchronous EXTRA, a bounded page count and an
explicit reserve account for rollback recovery. At least half the configured
quota is reserved for rollback and metadata overhead. Existing larger databases
are refused. Payload reclamation requires an explicitly configured canonical
authority and a matching durable contiguous receipt. Canonical history is retained.
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

## Check receipt projection

Save the receipt JSON returned by Spindle and record the Spindle origin associated
with the submission. The receipt file can contain the receipt itself or the full
`result: receipt` reply. Use `--receipt -` for standard input.

```sh
fabric projection-ready --origin "$SPINDLE_ORIGIN" --receipt receipt.json --wait 30s --json
```

Exit 0 means the exact observation was read from committed local Parquet. Exit 3
means custody was verified and the projection frontier was still behind. Exit 4
means evidence was unavailable, including a wrong origin, mismatched identity,
legacy projection, unreadable storage, or raw data removed by retention. Exit 2
indicates invalid arguments or receipt JSON. The result contains the observed
frontier when readable. Allow up to five minutes with `--wait`, or omit it for a
single bounded check. The check does not start collection or flush data.

Upgrade local Spindle, the collector, and readers together. Existing files remain
queryable. Earlier projected observations have no receipt provenance, and older
readers cannot read files with the new internal provenance field. Keep an existing
collector data directory bound to its existing Spindle store across the upgrade.

## Harnesses and adapter

Run `cargo run -p fabric --example agent_harness -- SOCKET [GLOBAL_RUN_UUID]`, or
`python3 fabric/examples/agent_harness.py SOCKET PATH_TO_ADAPTER [GLOBAL_RUN_UUID]`.
Both examples use v3; passing the same global UUID joins their observations. The
Python example also waits for Fabric custody and requires configured forwarding.
The older `examples/agent_harness.py` continues to demonstrate the v2 adapter.
The [generated tool examples](../examples/generated-tool/README.md) use the same
client or adapter instance as their harness.

The adapter accepts `record` with typed `data` and `timestamp_ns`, `flush` with
`timeout_ms` (capped at 30 seconds), and `status`. Every command returns one JSON
line. Inputs are bounded before decoding; oversized lines are rejected and close the adapter after its bounded final flush.
EOF triggers one additional flush with a two-second deadline and a structured
`eof_receipt` or error reply. Diagnostics use stderr. A successful memory admission
alone is never a durable receipt.

Executable delivery arithmetic is checked by Verus and linked to the SDK;
[collection verification](collection-verification.md) distinguishes those guarantees
from the unverified SQLite, transport, SDK worker and source adapter boundaries.


For shared runs, construct `RunContext::new()` once or deserialize the context
passed by the parent process, then use `RunClient::new(config, context)` in each
Rust process. Pass the same JSON context to child adapters:

```sh
fabric-adapter SOCKET PRODUCER --run-context '{"run_id":"f404b128-2c19-412f-bcd0-823f9c1f1b31"}'
```

Agent-run and tool-execution payloads use that same UUID in `data.run_id`.
`flush` in context mode confirms local custody; `flush_to_fabric` waits for
canonical custody. `context` returns the shared identity; `renew` replaces the
lease only after all accepted records have local custody. Leases normally last
24 hours. Pending stored records remain forwardable after expiry, while new
records and retries are refused. The service permits at most 128 retained
generations and fails lease decisions closed if wall time falls behind its
persisted clock floor.

Configure canonical forwarding explicitly in the Spindle JSON configuration:

```json
"forwarding": {
  "endpoint": "127.0.0.1:48200",
  "credential_file": "/absolute/private/node-credential",
  "authority": "fabric-primary",
  "reclaim_confirmed": false
}
```

The endpoint must be a loopback TCP address, usually a local tunnel to the
receiver. The credential file must be a private owned regular file. The canonical
receiver must authorize the persistent `status.origin`; its authority binding
cannot change after configuration. Back up the whole custody directory, including
its origin, private lease key and allocation state.

Forwarding starts in shadow mode: `reclaim_confirmed` defaults to `false`, so
matching canonical receipts advance `status.canonical_through` while every local
payload remains retained. Reads and exports continue to use those local bytes.
Before enabling reclamation, verify backfill through a captured prefix and
compare canonical record IDs and original bytes with local data, including
restart, retry and readback behavior. Check that the canonical authority, origin
and contiguous prefix match the configured destination. Then set
`reclaim_confirmed` to `true` and restart Spindle. Previously confirmed records
become eligible immediately, even if there are no new observations to forward.

Forwarding uses one background worker and bounded batches. Reclamation requires
both explicit opt-in and matching durable confirmation. The separate
`status.reclaimed_through` reports actual local payload deletion;
`status.forwarding_error` exposes handoff or reclamation failures.
Reads, export and duplicate retries retrieve reclaimed bytes from Fabric through
bounded remote workers. An unavailable receiver produces an error, never false
EOF. Export preserves original JSON bytes and its captured logical frontier.
Without forwarding configuration, the local retention and refusal behavior
continues unchanged.


After any payload has been reclaimed, forwarding cannot be disabled or switched
to another authority. Roll back further deletion by setting `reclaim_confirmed`
to `false`, keeping canonical access configured for already reclaimed records.
Shadow mode can exhaust the local quota while verification is pending; its
confirmed prefix is preserved and does not imply payload reclamation.
