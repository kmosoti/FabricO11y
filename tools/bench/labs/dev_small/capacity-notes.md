# Capacity fixture preparation

Status: implementation prepared; checks and workloads are coordinator-owned.
No execution result follows from this note.

`native.trial` privately adapts `tools/bench/run_dev_small.py`; the original,
production runtime and independent query oracle remain unchanged. The coordinator
supplies node count, three aggregate rates and the historical condition. Default
schedule is 60/60/60 seconds, five-second settling and twenty-second drain.
Fractional per-node offers accumulate over 100 ms ticks without dropping demand.
Every native source body remains 900 bytes with the frozen repetitive/entropy mix.

`lab_history_seed CONFIG CONDITION LEDGER [TARGET_BYTES]` prepares H0, H256 or
nearrotation through `Store::commit`, recording exact source/body and encoded
Batch hashes before each accepted production ACK. The override is a preparation
fixture only and is reported explicitly; it cannot silently replace H256.
H256 contains exactly 256 MiB of encoded Batches; the final Batch adds a bounded
`fixture.padding` attribute while retaining 900-byte bodies. Native Batch validation
runs before commit. A dedicated enrolled `oldhistory` credential binds the frozen
7171… identity, generation one and contiguous sequences. H0 enrolls the same
unused credential. Historical observations begin at 1759680000000000000 ns;
receive timestamps are actual production-clock readings and their range is
reported. Batch prefix hashes match across conditions; physical Segment hashes
can differ because their receive timestamps differ.

H256 closes its final active file with the existing FrameLog rotation operation,
uses the production bounded Segment builder, then reopens Store to checkpoint and
reclaim published journals. The harness requires an empty active journal. Q1
nearrotation leaves its verified prefix active; encoded input is 64 MiB minus
128 KiB, and the harness independently requires actual framed active bytes to be
within the last 256 KiB below the unchanged 64 MiB threshold. Live Spindles enroll
normally: native credentials, identities, Spools and sequences are not rewritten.
The additional historical identity limits fleet-attribution interpretation.

A post-publication production replay checks every seeded Batch's identity, SHA
and length against the pre-commit ledger. Final source/body exactness includes
old and live records; ACK exactness joins both the seeded ACK ledger and native
stdout ACK ledger to durable recovery. Seed source timestamps and native stdout
ACK measurements are unavailable, so seed rows are excluded from live latency
populations. Eight-field data-clock rows retain recovered body hashes for an
independent audit, including historical rows with null source/ACK timestamps.

The unchanged query oracle receives the complete old+live durable replay before
owned replay cleanup. The shared observer additionally queries oldest/newest
historical sentinels. Resource ceilings remain 4 GiB scratch and 900 seconds per
cell inside the coordinator's 20 GiB envelope. Declared server journal/retention
ceilings are 1 GiB each, retention is 24 hours and sealing has one worker.
Preparation CPU/wall time is separate from sampled timed application work.

Coordinator validation commands: build the helper and server_dump examples;
run `native.controls()`; prepare an H256 fixture with TARGET_BYTES=1048576;
verify its replay and empty active file; then admit the frozen cells. The
controls reject missing, changed and extra source records and altered seed Batch
hashes. None of these commands was run by the Capacity PI.
