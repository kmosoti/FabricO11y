# Fedora performance refinement protocol

Registered at base `a6d4905` before new performance comparisons. This follows the
[refinement audit](../../research/performance-refinement-audit.md) and extends the
[completed cloud screen](responsibility-isolation-r2-run-01.md). Native processes,
sequential resource-intensive trials; exploratory evidence, not deployment
qualification. No shipping defaults, durable ACK, exact Batch custody, reset rule,
or publication-before-reclaim semantics change. Original failures stay immutable.

## Correctness prerequisite

Reproduce the three retained diagnostics before modifying rates. Add HTTP/OTLP
integration regressions graded by the unchanged Python oracle: colliding display
keys, integers above 2^53, full i64 delta, mixed integer/double points, 2,048
independent series, scan/walk, journal/Segment and restart. Preserve existing
presentation ordering; break display-key ties by structural map, and retain the
existing timestamp/identity/sequence/index ordering inside each series. Mixed
comparison uses exact represented values; mixed subtraction retains floating-point
arithmetic, consistent with the independent oracle. Non-finite rates reset.
Run the fast profile and documentation check, retaining failures and receipts.

## Host and resource boundaries

Capture CPU model/topology/affinity, RAM/swap, storage/mounts, all ancestor cgroup
limits/events, process limits, toolchain and source revision. Compare with the
cloud's four CPU equivalents / shared 16 GiB, rather than transferring its numbers.
Owner-directed storage amendment, 2026-10-04: use
`$FABRIC_SCRATCH_ROOT/performance-refinement` on the mounted data drive through
the [resource launcher](../../../tools/resource_group.py), replacing repository
`target/performance-refinement` for future trial scratch. Keep observations and
failure evidence before owned-file cleanup, and record the changed filesystem
when interpreting measurements. Earlier results retain their original storage.
`/tmp` on this Fedora host is tmpfs and is unsuitable for storage costs. Do not
flush global caches, stop unrelated services or delete unrelated files. Do not
run loads on digitalocean-01 while its webserver is serving. Source, producer,
server and observer share this PC; thermal/frequency and background contention
remain confounders. Guard each heavy process at 8 GiB address space and each trial
at 8 GiB predicted scratch, with at least 12 GiB available RAM and 20 GiB free
disk before launch. Stop on failed exactness, process failure, insufficient
headroom or OOM; archive before removing only owned scratch.

## Attribution and calibration

H1: materialization, encoding/compression, source loading or output construction
concentrates CPU/allocation demand. H0: no concentrated cost on these coordinates.
Use feature-gated benchmark observation, disabled in production builds. Capture
nested inclusive wall/thread CPU spans, allocator live/requested snapshots and
owned process RSS/IO. Nested spans overlap: never add or subtract independently
measured percentiles. Track live memory at input, decode/projection, sort, Arrow,
writer finish, sync/publication, query source acquisition/execution/result output,
serialization and release. Library allocations not using the Rust allocator and
file cache are distinct, incompletely owned populations. Compression combined
with Parquet encoding/write is labeled inclusive unless directly isolated.

Native collection measures actual Spindle collection and OTLP assembly, alongside
file reading, host sampling and durable Spool commit. Cover two configured source
paths and invalid UTF-8/missing-source gap cases. Query attribution covers first
and warm scan/walk, broad/selective, tail/Segment, independent exact grading, and
answer serialization/release. Rate cardinality and resets are correctness screens
before timing. Full mixed-signal/consumer coverage is not inferred from logs.

Preflight each distinct new case once before measurements. Calibrate plain versus
phase observation and counted allocator at 4,096 shuffled 1,024-byte log bodies:
five fresh-process pairs, alternating AB/BA; report timer floors and all costs.
A >=10% median change in four of five same-direction pairs is material observer
perturbation. Do not subtract observer costs. Plain timings and counted memory
are separate populations. Mark >20% CV unstable; retain all trials, no favorable
retries. No p99 with fewer than 1,000 comparable observations.

## Granularity, workers and interactions

H1-G: finer journal files reduce per-builder live memory at a CPU/metadata/IO
cost. H0-G: memory is not reduced or useful completion does not compensate costs.
First hold one worker, shuffled seed 42, 262,144 x 1,024-byte raw log bodies
constant, and vary rotation at 1, 16, 64 MiB encoded bytes. Ensure at least four
representative default-sized journal files; capture actual file/group sizes and
active tail. The fixture journal allowance is 1 GiB to admit the complete input
before the timed finite pass; this is not a product cap selection. Hold 64 MiB rotation next and vary 1/2/3/4 workers. Then test all
selected granularity x worker interactions: 16 and 64 MiB x 1/2/3/4 workers.
Deduplicate cells already executed. Five fresh-process repetitions per cell,
counterbalanced order; counted repetitions separate from plain. Exact recovered
Batch bytes and independent source bodies are mandatory. Record wall, total CPU
per exact record/raw and encoded byte, incremental/live heap, RSS, IO, file counts
and sizes, publication/reclaim. These are finite drain passes, not arrival-rate
capacity tests. Extra workers never mean reserved CPUs.

Keep 64 MiB, workers and caps as hypotheses. Describe the Pareto set. For the
composite select the lowest worker count within 10% of the fastest median whose
CPU is <=1.5x the one-worker same-file baseline and whole-process RSS <3 GiB;
if no cell meets constraints retain one worker. Choose 16 MiB only if it reduces
incremental heap >=25% without >25% extra CPU or >20% extra wall at that count;
otherwise use 64 MiB. Selection is a trial coordinate, not a shipping default.

## One improvement at a time

The rate correction is correctness work, not a performance win. Attribute first;
register a separate candidate protocol before any algorithmic optimization.
Each candidate must preserve exact answers/custody and compare against the frozen
baseline separately, with a >=10% primary median improvement in four of five
pairs and no >10% CPU or peak-memory regression unless prospectively allowed.
Combine only candidates that survive isolated comparisons; no unimplemented
bounded builder, admission budget or pipeline is claimed as tested here.

## Sequential native composites

After prerequisites, run development, small and moderate in that order with real
Spindles, native TLS and original source-write clock. Finite 120-second offers:
30 s normal, 30 s 3x burst, 60 s recovery. Rates 10 / 1,000 / 10,000 logs/s;
1 / 4 / 20 real Spindles, deterministic seed 42, 256-byte unique text bodies,
100-ms producer ticks. One selected worker/file coordinate from the above screen;
development may use 64 KiB rotation to cross first publication, explicitly labeled.
Also test moderate 200 nodes at equal aggregate rate as a separate cardinality
cell if resource preflight permits; a resource failure is retained, not omitted.

Offer queries sequentially at 1 s cadence, alternating broad paginated logs and
selective marker queries through HTTP, for programmatic consumer timing. Keep
request/response, original file-write, independent OTLP collection, server receive,
ACK and first successful query clocks. Visibility polling gives upper bounds with
interval censoring. Do not relabel engine timings as HTTP or consumer latencies.
Capture original source time outside the body so telemetry fields are unchanged.

At 60 s stop server for 5 s, retain source writing/Spool custody, then restart;
allow up to 120 s drain after offer. Stop/restart nodes and server after drain,
query again and verify exact recovery against original source IDs/hashes and the
unchanged query oracle. Record pending bytes/oldest source age and drain curve.
Archive exact records before retention. Run a second retention phase with a
finite byte budget (at least two Segments, drop oldest), verify retained answers,
completeness/window/page expiry and durable custody checkpoint after restart.
Retention intentionally expires evidence; distinguish pre-retention exactness
from expected post-retention coverage. No destructively injected faults.

CPU per useful record/byte includes producer/server/node separately; report
observer separately. Measure RSS peaks and retained after-drain RSS, cgroup
anonymous/file/kernel/event snapshots, IO and archive size. Populations separate
normal/burst/recovery/tier/clock; p50/p99 only when comparable counts suffice.
Gates: exact pre-retention source recovery, no hidden gaps, clean exits, bounded
120 s drain, nonnegative clocks, no OOM; descriptive latency/resource evidence
without invented deployment SLO. Failure stops dependent comparisons. Document
completed and unrun cells explicitly, and preserve every raw failure.

Prospective fixture refinement before implementation/preflight: the 262,144-row
fixture provides four closed 64 MiB files, so a three/four-worker comparison is
not silently limited to two available tasks. Earlier protocol text used 131,072
rows; no measurements ran under that size.

Compression attribution refinement, before preflight: obtain decompressed encoded
pages from the native-built Parquet tables and replay the unchanged Parquet Zstd
level-3 codec on those exact page payloads, checking exact decompression. This
standalone primitive cut isolates codec work; it is not an exclusive phase of the
original integrated writer. Preserve inclusive writer-write/finish/flush/sync
spans alongside it, and do not subtract their independent percentiles.
