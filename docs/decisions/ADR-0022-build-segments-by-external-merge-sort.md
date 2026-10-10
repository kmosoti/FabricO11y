# ADR-0022: Build Segments by external merge sort

## Status

Accepted on 2026-10-01 in the bounded-sealer milestone; input/group ownership amended on 2026-10-10 below. The owner chose external merge after an exploratory comparison of seven candidates. The [sealer view](../architecture/sealer.md) describes the implementation, including spans and per-file workers introduced by ADR-0025, and sixteen-way multi-pass merging. The original single-thread, two-sorted-table description below belongs to the decision's historical context; its claimed universal memory bound has not been established. Registered acceptance thresholds remain unchanged.

The Segment layout, manifest version, table schemas, row order, commit protocol and readers remain compatible with [ADR-0020](ADR-0020-store-sealed-history-as-parquet-segments.md) and the [retained-history contract](../architecture/retained-history.md). The amendment states which physical group boundaries change; byte equality is attached to its measured reference, not every historical writer.

## Context

Implementation follow-up: the [speed investigation](../experiments/benchmarks/sealer-speed-run-01.md)
uses compact binary scratch records and rewrites only enough contiguous runs
to meet fan-in on a partial pass. These refine disposable scratch processing;
the decision, persistent formats and acceptance protocol below are unchanged.

The [readiness continuation](../experiments/formal/readiness-continuation-results.md)
records an opt-in aligned-input writer with reference physical row groups:
eight cells with three pairs passed the original builder gates, and eight
supplemental spill replays preserved inputs/manifests. Named merge, cleanup
and heap mutants were caught; finite syscall/kill cuts were independently
query-graded. The subsequent combined disk-PageStore campaign and
[companion-compatible soak](../experiments/benchmarks/soak-run-02.md) passed
their registered gates. The original failed soak and failed intermediate
candidates retain their outcomes.

## Input and encoded-page ownership amendment, 2026-10-10

The normal writer separates owned input chunks from physical Parquet groups.
Logs use encoder-aligned 1,024-row input chunks with an estimated 17 MiB target;
metrics and spans retain 8,192-row/8 MiB input targets. All sorted tables retain
8,192-row physical groups across input writes. Raw Batch/gap chunks keep explicit
physical flushing. One oversized row is preserved even above an input target;
these estimates are not absolute allocation bounds.

Every table stores completed encoded pages in private disk scratch and loads
each consumed key on demand. Page files must be removed before manifest
publication. Any read, write or cleanup error prevents publication and retains
journal custody. This supersedes the original 8 MiB physical-group closure for
sorted tables. It changes neither encoding nor schemas, readers, manifest
version, ordering, custody, publication or sync order. Physical bytes can differ
from the former bounded writer when its byte cap closed a group early; recorded
byte equality is to the retained whole-file reference on the measured inputs.

The reason is an executable counterexample: bounded input alone retained
136.45 MiB of encoded pages on high-entropy logs. Disk-backed page ownership
reduced that probe to 40.07 MiB with exact output. The repeated eight-cell,
three-pair campaign passed the original 80 MiB ceiling and 10% scaling rule;
the full R2 soak passed all ten original gates with exact simulator and companion
custody. The [encoded-page record](../experiments/formal/encoded-page-memory-run-01.md)
and [soak record](../experiments/benchmarks/soak-run-02.md) retain commands and
limits. This decision admits that tested path as the default, subject to an
unflagged build and regression verification. It does not assert an arbitrary-input
heap bound, deployment qualification, or completion of the milestone's merge/CI
requirements. Legacy build selectors accepting `1` remain compatible.

## Original design context

The sealer turns each sealed server journal file into a Segment. Today `segment::build` does it in memory, all at once:

- it decodes the whole file into Groups;
- it clones every entry for the raw Batches table;
- it extracts every log row and metric point, then sorts all of them;
- it builds each Parquet file in a buffer before writing it.

For a 64 MiB file the peak is about 356 MiB, about 5.5 times the file. In [soak run 01](../experiments/benchmarks/soak-run-01.md) the server's memory stepped from about 140 MiB to about 535 MiB at the first seal and stayed there, because the allocator kept what the build had used. The run failed its `no_rss_growth` gate.

A replacement has to satisfy five conditions:

1. No row is lost or duplicated.
2. Every Segment answers every query as today's does, which means the same rows in the same order, because queries skip row groups by their time range.
3. Peak memory is bounded in bytes, and does not grow with the file or depend on the shape of its data.
4. The build is repeatable: the same input gives the same files.
5. Every failure leaves the journal file in place and can be retried.

## Decision

The sealer builds a Segment by **external merge sort**. It reads the sealed file once, in a stream; sorts the log rows and metric points in byte-capped runs that it spills to scratch files; then merges the runs into the final tables.

1. **Stream the journal file.** Read one frame at a time (at most 4 MiB, the existing `MAX_GROUP_PAYLOAD`), decode its Group, and drop the payload. Entries move on without being cloned.
2. **Route each entry.** Its raw bytes go to the Batches table in journal order, in chunks of at most 4 MiB. Its collection gaps go to the gaps table in journal order. Its log rows and metric points go to the sorted tables' run builders.
3. **Cut runs.** For each of logs and metrics, collect rows until 32,768 rows or an estimated 16 MiB, sort them by the contract's order key (time, node identity, sequence, index), and write them as one run file inside the Segment's `.building-<label>` directory. A run is a private, length-prefixed record stream. It is scratch: it is not synced, because the journal file stays the source of truth until the Segment commits.
4. **Merge.** After the last frame, open every run with a 64 KiB buffer and merge them with a min-heap holding one row per run. Emit rows into row groups of at most 8,192 rows or an estimated 8 MiB, and write each through the table's Parquet writer.
5. **Hash while writing.** Each table writes through a hashing file writer, so the manifest's SHA-256, size and row count are known without buffering a whole file or reading it back. Each file is synced when it closes.
6. **Delete the runs, then commit.** The run files are removed before the manifest is written. The manifest, the directory sync, the rename to `seg-<label>` and the parent sync are exactly ADR-0020's commit protocol.
7. **Fail cleanly.** On any error, the builder removes `.building-<label>` with its runs and returns the error. The journal file is untouched, and the next pass retries. After a crash, startup removes any `.building-*` directory, runs included, and the sealer builds the Segment again.

The order key is a total order, so the merge's output does not depend on how rows were split into runs. The row groups differ from today's only where the byte cap closes a group before 8,192 rows.

**Constants, not configuration.** The run, row-group and buffer sizes are constants in the sealer. They add no configuration key, because the [roadmap](../ROADMAP.md#known-risks) treats a key without a gate consumer as a defect.

| Constant | Value | Bounds |
| --- | ---: | --- |
| Frame payload | 4 MiB | the decoded Group being read (existing limit) |
| Run size, per table | 32,768 rows or 16 MiB | rows held to sort |
| Raw Batches chunk | 4 MiB | entries held for the Batches table |
| Row group | 8,192 rows or 8 MiB | a Parquet writer's in-progress group, per table |
| Run reader buffer | 64 KiB | the merge, per run |
| File writer buffer | 256 KiB | each table's output |

Sizes in bytes are estimates computed from each row's field lengths, not exact allocations, so the measured peak, not the arithmetic, is the evidence for any ceiling.

**What stays in place.** The sealer remains a single thread that builds one Segment at a time, so the bound is per server and not per file. Retention runs after the builds as it does now. The `SegmentStore` port, the retention kernel, the query code and the Segment format are untouched. The code lives in `fabric-server`'s Segment module beside the code it replaces; it adds no crate and no dependency.

## Alternatives considered

Six alternatives were prototyped against the server's own row extraction and Parquet writer, and run on four workloads of one 64 MiB file each. The [study](../experiments/benchmarks/sealer-study-run-01.md) is exploratory: no protocol was registered before it ran. Peak heap figures are the worst of the four workloads.

| Alternative | Result | Why it was not taken |
| --- | --- | --- |
| A. Sort each row-group-sized chunk alone | 40 MiB; fastest | Row groups overlap when rows arrive out of order, and queries then read up to 5.7 times more rows, permanently |
| C. Reorder window with late rows apart | 60 MiB | Exact only while the disorder fits the window; up to 3.1 times more rows read when it does not |
| D. Two passes with range partitioning | 49 MiB, rising with the file | Exact, but it reads the journal file twice and its key index grows by one entry per row (12 MiB at 64 MiB, 48 MiB at 256 MiB) |
| H. Window for in-order rows, merge for late ones | 65 MiB | Exact only when nothing is late; otherwise two sorted sequences and up to 1.3 times more rows read |
| G. Seal every 8 MiB instead of 64 MiB | 49 MiB for eight files | Memory still grows with the file; eight times as many Segments for queries to open |
| Seal in a child process | not prototyped | The peak is unchanged; only its return to the operating system is guaranteed, at the price of a process boundary |
| A different allocator | not prototyped | Returns memory after a seal; the peak and the new dependency remain |
| Seal incrementally while the file is active | not prototyped | Needs a second build path for crash recovery, because a half-written Parquet file does not survive a crash |
| Pipeline the stages across threads | not prototyped | A seal takes about 0.2 % of the time a file takes to fill at 100 identities, so there is nothing to win, and threads would compete with the commit thread |

The cost of choosing E over A, C and H is disk and I/O, not speed: temporary spill, and more bytes moved.

## Evidence

From the exploratory study, on the prototype and not on product code:

- **Memory.** E peaked at 42.5 to 43.0 MiB on all four workloads, against 300 to 356 MiB today, and at 42.5 to 44.9 MiB for journal files from 16 to 256 MiB, against 92 to 1,389 MiB today.
- **Output.** E produced the same rows in every table, the same manifests apart from file hashes, and the same row order as today on every workload. Its logs and metrics files were byte-identical to today's on the three workloads where the byte cap never closed a row group early.
- **Repeatability.** E built identical manifests, which carry every file's SHA-256, on repeated runs.
- **Query cost.** Read amplification equalled today's on every workload, including data that arrives out of order.
- **Cost.** E spilled 72.9 MiB for a 64 MiB file, read 137 MiB and wrote 119 MiB, against 64 and 46 today. It was no slower than today's sealer: about 1.1 s against 1.4 s on tmpfs for the steady workload, and 1.07 to 1.21 s against 1.22 to 1.62 s on a real disk.
- **A defect in the first prototype.** Before byte caps, E and three other candidates peaked near 170 MiB on 16 KiB log bodies, because their limits counted rows. The limits now count estimated bytes, and the big-rows workload stays as a regression workload.

## Consequences

- **Memory.** The sum of the caps is about 80 MiB: two runs of 16 MiB, four row groups of 8 MiB, a 4 MiB Batches chunk and a 4 MiB frame, with the decoded Group briefly alongside its payload. Every measured peak was between 42 and 45 MiB, because the buffers are rarely all full at once. The merge adds a 64 KiB buffer and one row per run, and a 64 MiB file gives about nine runs, so the bound is flat in the file size only while the run count stays small.
- **Disk.** While a seal runs, the state directory holds the journal file, the spill runs and the growing Segment: about 3 times the journal file at the peak (64 + 73 + 48 MiB for a 64 MiB file in the study). Spill is deleted before the Segment commits. A full disk fails the seal and leaves the journal file in place; the sealer retries every second and logs each failure, as it does today.
- **I/O.** About 2.1 times the bytes read and 2.6 times the bytes written compared with today, per Segment, on the steady workload. Spill is not synced.
- **Time.** No slower than today in the study, and not a constraint: it does not affect an ACK directly. The sealer's contention with the commit thread for CPU and disk is not measured yet.
- **Verification.** The current `segment::build` is kept as a test-only oracle for a differential test, as earlier milestones kept frozen copies of replaced code. The [milestone](../milestones/bounded-sealer.md) registers the tests, the mutants and the measurement protocol before the implementation.
- **Not decided here.** Renaming the `.faj` frame-log extension, and any change to the frame log, are separate decisions with their own contract change and migration.

## Open questions

- Whether a table that fits in one run should be written directly, without a spill file. The prototype always spilled. The change cannot alter the output, and the same differential test would cover it.
- Whether to check free space before a seal, or only to fail cleanly when it runs out.
- Whether spill I/O measurably moves ACK p99 beside a live commit thread. The study did not run them together.

## Validation

This decision is falsified by any of these:

- a Segment built by the new sealer answering a query differently from one built today, on any registered workload;
- peak heap above the registered ceiling on any workload, or growing by more than 10 % between a 64 MiB and a 256 MiB file;
- a rerun of the registered soak failing a gate that run 01 passed, or still failing `no_rss_growth`;
- a run file or `.building-*` directory surviving a completed or failed seal.

## Related

- [ADR-0020](ADR-0020-store-sealed-history-as-parquet-segments.md) (the Segment format this decision keeps)
- [ADR-0015](ADR-0015-adopt-a-hexagonal-architecture.md) and [ADR-0016](ADR-0016-keep-a-pure-semantic-core.md) (the sorting is a mechanism, so it stays in the adapter; the order it follows is already a contract)
- [ADR-0018](ADR-0018-accept-work-on-executable-evidence.md) (executable evidence)
- [Sealer view](../architecture/sealer.md)
- [Sealer study, run 01](../experiments/benchmarks/sealer-study-run-01.md)
- [Coordinated memory/query/recovery investigation](../experiments/benchmarks/readiness-labs-run-01.md): reconstructed shape evidence and a physical-index-count checker counterexample; full acceptance remains incomplete.
