# S2: immutable snapshots and persistent progress, run 01

Status: selected candidate checked, reviewed and integrated; the [full E3
retry gate](resume-e3-run-01.md) and [local lifecycle](local-prototype-run-01.md) passed. This is research tooling, with no change to the application's FOL2 log.
The [registered protocol](durable-snapshot-s2-protocol.md) and
[API](../../../tools/storage-probe/DISK_API.md) define the contract.

## Hypothesis and setup

A snapshot can publish immutable blocks and authenticated metadata, reopen against
an independently retained anchor, and persist incomplete query progress without
silently converting missing data into a complete answer. Publication writes and
syncs fresh staging files, syncs the staging directory, renames it, then syncs the
parent before returning an anchor. All publishers must respect its exclusive
publication lock. The caller supplies a fresh name and already durable parent.

Two independent Rust candidates implemented the same frozen oracle. The S2 JSON
codec preserves Event arrival order, duplicate IDs and attribute keys, all integer
boundaries, Unicode and exact floating-point bits. Metadata and checkpoints use
Serde, while the application Event types remain unchanged. This adds a separate
experimental format; it does not select JSON over another physical layout.

## Checks and counterexamples

Both candidates passed `cargo test --release --offline --locked --manifest-path
tools/storage-probe/Cargo.toml --test disk_contract --test disk_restart` (exit 0):
11 codec/publication/query tests and six checkpoint/process tests, including two
subprocess helpers. They cover interrupted-publication fixtures, simultaneous
publishers, real checkpoint restart, late/out-of-order events, hot/cold copies,
metadata rebuild and refusal to replace existing outputs. Six additional review
tests passed on the selected candidate (exit 0).

Review found two defects in candidate A. A partial successful read followed by EIO
discarded the partial byte count. A false availability bit produced the correct
incomplete page but omitted the ordinal from the current page's read metrics.
Both were repaired. The read-error checker intercepts real Linux reads: 64 bytes
are returned before EIO, including a metadata case that then reads its fallback.
The selected source reports those bytes. `python3 -B tools/bench/check_disk_io.py
target/disk-io/final /tmp/fabric-disk-a` exited 0 with both injections observed.

Removing the byte-count increment made that checker exit 101. The original
candidate B also exited 101 on this discriminating probe. Three earlier mutations
that trusted a changed anchor, declared missing raw data complete, or skipped the
checkpoint digest check each failed their intended test with exit 101; the clean
control exited 0. These were correctness checks, not timing comparisons.

The selected `disk.rs` SHA-256 is
`933b9be7c2c3cdc47b8a20fad12d262d5fe1aade3fa327c5be51b029efaf9c35`.
GPT and Claude independently ran adversarial probes and approved this source.
Claude's final run repeated its nine probes, the frozen tests and the real EIO
checker. Commands, exits, source hashes, original findings, repair reviews and
mutation evidence are preserved in the [run artifacts](data/durable-snapshot-s2-run-01/).

## Interpretation and limits

Queries authenticate metadata before pruning, then authenticate candidate raw
blocks before emitting rows. A missing, unreadable or corrupt candidate stays
unavailable. A valid cold copy can satisfy a failed hot read. Successfully excluded
blocks require no raw read; that establishes query coverage, not retention.
Read counters describe attempted opens and bytes returned to userspace, including
partial reads. They are not device-I/O measurements. Current-page unavailability
is separate from the accumulator's remaining work.

If all index metadata is lost, rebuilding tries the available raw copies and must
reproduce the old anchor. A shape-valid corrupt hot copy can prevent that match
even when a valid cold copy exists. This fails closed; remove an independently
identified bad copy or restore the trusted source before rebuilding. The prototype
does not search exponentially many combinations of alternate copies.

The filesystem assumptions require honored syncs, durable ancestors, stable paths,
cooperating publishers and no unresolved storage-I/O error. Process restart,
partial fixtures and injected read errors do not prove physical power-loss safety.
The independent publication and checkpoint hashes must remain trusted outside
their untrusted files. This cell supplies the persistence library; the
[completion contract](end-to-end-prototype.md) still requires the runnable lifecycle,
collection adapter and measured costs.
