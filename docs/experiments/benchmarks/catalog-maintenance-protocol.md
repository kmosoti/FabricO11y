# CQ2: synchronous catalog preparation versus acquisition

Status: draft; register this revision before building or measuring.
This screen uses [the real storage-owned read boundary](../../architecture/retained-history.md),
after the separately retained publication/reclaim discovery corrections. It extends
[CQ1](catalog-boundary-protocol.md); it does not repeat that extraction comparison.

H1: explicit preparation after each durable append lowers subsequent acquisition
cost enough to reduce total schedule CPU in a read-heavy schedule. H0: repeated
directory discovery and Manifest cloning make preparation neutral or worse when
refresh and query costs are both charged. A useful counterexample is rate zero:
refresh cannot save query work there. No proactive mechanism is adopted by this run.

The sole treatment is calling `History::refresh_catalog(newest)` synchronously
after ACK, versus allowing `History::run` to discover new frames lazily. Both use
Walk, owned Manifest descriptors, the same initial priming query, unchanged row
validation, and borrowed-log experiment disabled. Public defaults, ACK ordering,
durability, journal rotation, retention and wire formats are unchanged. This is
a probe scheduler, with no background task or production ingestion integration.

The fixture is seed 42, 65,536 chronological logs of 1,024-byte bodies, 128 rows
per Batch. After durable seeding and exact replay, actual recovered entries form
64 nonoverlapping typed Segments of 1,024 rows each, then the Store reopens for
exactly 32 appends of 128 rows. Seed journals may remain physically present;
ordinary coverage masking and Store recovery handle that overlap. There is no
concurrent builder in the measured schedule. The small control uses one Segment. Node ID is `[7;16]`, label
`fixture`, generation 1, sequence/group 1..544; observed timestamps start at
1,600,000,000,000,000,000. Bodies use the existing xorshift generator: even rows
pad `R`, odd rows printable ASCII. Attributes are empty. Native Store groups one
submitted Batch at a time, and the probe checks every ACK and committed frontier.
Journal rotation is 512 KiB; fixture size and append count bound refresh work.

Rate is 0, 1 or 4 queries per append. Calls alternate broad and substring
`bench-0007 `, both within the full fixture window and limit 10,000. The per-query
snapshot is the actual committed prefix after that append. Two quiet broad and
selective controls follow every schedule, including rate zero. Initial priming,
fixture construction, final recovery, output files and continuation grading are
outside the schedule. The primary wall/process-CPU span includes all 32 submits
through durable ACK, refresh when enabled, first-page queries, serialization and
bookkeeping. Nonoverlapping append/refresh/query/serialization spans attribute
cost. Counted allocations are diagnostic per span; nested resets do not provide
a total-schedule allocation measurement. No warmed-page timing is a cold-disk claim.

Each rate gets three fresh plain pairs in order lazy/eager, eager/lazy, lazy/eager.
Separate preflight jobs run small seed-128 lazy/eager controls and an eager
control omitting refresh 32. Admit full jobs only after those controls complete;
a separate diagnostic slice contains the full counted pair. Skipping preparation must still
produce exact results by lazy acquisition. Counted wall time is not a speed claim.
Report each plain pair's eager/lazy total CPU and wall ratios, plus sums by phase,
refresh/query counts and counted allocated bytes/peak increments. H1 is supported
only for cells where all three total-CPU ratios are below 1 with exact controls;
otherwise report H0-compatible/noisy or workload-specific evidence. Latency moving
out of `run` without a total improvement cannot establish H1.

The [native probe](../../../crates/fabric-server/examples/catalog_maintenance_probe.rs)
retains every measured first page, then closes the writer and recovers actual
entries. The [driver](../../../tools/bench/labs/catalog/maintenance.py) drains one
complete chain at a time in separate native invocations, grades every chain with
the unchanged Python query oracle against its recovered Batch prefix, and checks
the measured first page byte-for-byte. No digest substitutes for an exact oracle.
Changed-body, missing-row and duplicated-row controls must fail the same checker.
Archive controls independently reject changed framing, missing chunks and duplicated
chunks through the exact byte/hash/length reconstruction checker.
Actual record bytes/hashes, exact wrappers, ordered page maps, oracle verdicts,
commands, explicit BENCH environment, source/oracle/binary hashes and native
timing outputs are retained. Completed trial summaries and per-answer grading
maps are written incrementally, including before the next drain is launched. Only identical row-byte chunks
are deduplicated; every reconstructed JSON is compared with the original bytes.

A separate [freeze launcher](../../../tools/bench/labs/catalog/maintenance_freeze.py)
builds both variants once with `FABRIC_BORROWED_LOG_EXPERIMENT=0`, checks unchanged
source hashes across capture, and archives `plain.gz`/`counted.gz` with exact binary
readback hashes. The counted build adds `responsibility-alloc-probe`; each build
has a 600-second subprocess ceiling within the shared freeze deadline. Root
registers this protocol before the contained freeze command:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-maintenance-freeze-01 --lab query --stage query --seconds 1500 -- python3 -B tools/bench/labs/catalog/maintenance_freeze.py --out docs/experiments/benchmarks/data/catalog-maintenance-freeze-01 --seconds 1200
```

For each rate `R=0,1,4` and slice `S=preflight,diagnostic,pair1,pair2,pair3`, the
concrete driver command is:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-maintenance-R-S --lab query --stage query --seconds 1500 -- python3 -B tools/bench/labs/catalog/maintenance.py --freeze docs/experiments/benchmarks/data/catalog-maintenance-freeze-01 --objects docs/experiments/benchmarks/data/catalog-maintenance-objects-01 --account CLOSED_QUERY_DATASETS --cap-mib 1024 --reserve-mib 96 --rate R --slice S --seconds 1200 --out docs/experiments/benchmarks/data/catalog-maintenance-R-S
```

Root replaces `R`, `S` and `CLOSED_QUERY_DATASETS` with the recorded literal rate,
slice and complete existing query dataset directory list. That list includes all
CQ1 slices, metadata precursor, frozen baseline archives, shared pools, compaction
receipts and completed CQ2 slices. The driver always adds its freeze, current slice
and explicit object pool itself. It verifies archived and decoded executable
hashes, decompresses only into launcher-owned scratch, and never duplicates binary
archives in slice evidence. Declared directory links preserve per-slice object
paths; accounting explicitly scans the real shared target once and deduplicates
inode identities. Exact content collision/readback checks apply to all row chunks.

The separately reviewed [evidence allocation](catalog-evidence-allocation-protocol.md)
permits 1 GiB persistent query evidence and 2 GiB aggregate catalog evidence;
original overshoot and compaction failure remain recorded. Each slice reserves
96 MiB before admission, checks current allocation before another trial and after
verified large-JSON compression, and stops with preserved artifacts on overrun.
Report logical path-size sum, unique-inode lengths and allocated blocks separately.
Root also audits aggregate catalog storage including capacity and coordinator
archives. Existing 8 GiB owned scratch, 16 GiB free reserve and memory/swap limits
remain unchanged. First-page retention is capped at 1 GiB; a chain at 256 MiB.
Native children and grading share the explicit 1,200-second deadline; timeout
kills the process group and preserves completed incremental maps/results. Success
cleans owned scratch. No workload or validator ran during preparation.

Rate four has 130 drain subprocesses per trial, including quiet controls. Every
startup creates a fresh History and opens real Segment descriptors; broad full
chains have seven pages versus one at preflight. One Segment at preflight cannot
predict 64-Segment metadata discovery cost. Use actual small native/grading duration
and the first full counted job to admit the remaining frozen matrix. A rate-four
plain pair entails 260 fully graded drains. If the shared budget cannot fit those,
stop expansion and report an incomplete screen; never delete chains or repetitions.

This fixture measures repeated append/read discovery across 64 fixed Segments
and a growing tail; it does not isolate clone-versus-Arc metadata or reader
concurrency, leases or cancellation. CR3 owns those comparisons; Q3 owns
frontier selection attribution. Concurrent reader/writer interleaving is covered
by separately registered exact gap/paused-cut controls, not inferred from this
single scheduler. A synchronous positive result would still require a separate
bounded scheduling design before any server adoption.
