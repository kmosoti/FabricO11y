# Cross-system execution readiness

Prospective registration, 2026-10-08. The owner restored execution permission;
the data drive is writable and the resource launcher has exited successfully.
This finite preparation slice starts the [cross-system queue](../../research/cross-system-lab-queue.json).
It changes no product, oracle, durability or performance acceptance criterion.

## Questions and decisions

G0 asks whether the current source can execute under the existing containment
and evidence allowances. O1 begins with the six existing `sealer::tests` controls:
published-prefix reclamation, blocked later groups, holes after failure/panic,
successful prefixes in failed groups, reclaim retry, and native checkpoint
failure preserving exact bytes. All six must pass for this baseline; this does
not test within-group early reclamation or complete O1.

Retrieve Turso revision `ff97ec42cdef94651a6985eb47e8f9d07e4608cd` from
`https://codeload.github.com/tursodatabase/turso/tar.gz/ff97ec42cdef94651a6985eb47e8f9d07e4608cd`.
Record the archive digest, complete member digest inventory, selected source
snapshots and observations. No upstream build, dependency adoption or performance
comparison is included. Failure to retrieve remains a failed retrieval, not a
source-performance result.

## Frozen execution

Run `python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/catalog/cross_system_readiness.py`.
The driver runs `cargo test --offline --locked -p fabric-server --lib sealer::tests`
with one test thread, then retrieves and examines the pinned source. Build/test
timeout is 150 seconds; network retrieval timeout is 60 seconds. One 240-second
preparation allocation includes admission, source comparison and cleanup. The
existing coordinator lock serializes work; the old 14400-second frontier and
86400-second total allocations remain. Charge the two preflight launcher
receipts (units `fabric-work-806ce58a0f7244f4a193d1aceab1a162` and
`fabric-work-d9bb5a6272ee4dfe9496c00f4ce398af`) to this receipt as well.

Memory high/max remain 16/20 GiB, swap zero, outer deadline 1800 seconds.
All temporary files and build cache use the mounted data drive. Keep at least
16 GiB free. Download at most 64 MiB compressed; inspect members in place,
without extracting an upstream tree, with a 512 MiB total decoded limit.
Only regular source members are read; links and special files are not extracted.
Temporary archive removal follows recorded digest/inventory and selected-source
readback. An incomplete download is retained as a digest, length and exact curl
diagnostic, not described as a verified checkout.

Reserve 512 KiB retained evidence using `coupled_admit.observe` before/after.
Reuse the immutable `catalog-range-evidence-checks-01/source.tar.gz` only after
comparing current Rust/Cargo/oracle bytes, with a changed-byte rejection control.
Save the new driver, protocol and source hashes separately. Test output is
bounded to 64 KiB; output beyond the bound fails the slice rather than being
silently accepted. Archive/member manifest and selected source snapshots use
gzip, with byte readback before temporary cleanup. Exceeding the retained bound
fails admission/cleanup and preserves the offending owned state for diagnosis.
No historical receipt or failed outcome is changed.

## Observations and limits

Record exact commands/exits, source and protocol hashes, elapsed wall time,
cgroup CPU, peak memory, memory events, swap and I/O, storage paths and cleanup.
Test/build memory is not server RSS. No throughput, ACK latency, reservation
coverage or independent mixed-signal query result is measured in this slice.
M1 and Q1 require their own registered workload adapters; this receipt cannot
complete those experiments, C5, deployment qualification or the overall goal.
