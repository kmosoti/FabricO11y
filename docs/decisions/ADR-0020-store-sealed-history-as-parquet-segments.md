# ADR-0020: Store sealed history as immutable Zstd Parquet Segments

## Status

Accepted on 2026-09-28 in the history-qualification milestone, as plan item 4.6 asked. The format was implemented before this record was written; the record captures it, with the measurement that tested it. It changes no bytes. Amended on 2026-10-02 by [ADR-0024](ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md), part 2: a Segment may hold an optional fifth file, `text_filter.bin` (one trigram bloom per logs row group), listed in the version-1 manifest's `files` like the others; the sentence "There is no token index" below no longer holds. Version-1 readers that do not know the file ignore it, and a reader that uses it falls back to the exact scan when it is missing or its digest differs.

## Context

The server's journal is the durable record of every committed batch. A query that decodes journal frames must parse every Batch in its range. Retention also has to delete old data in whole units without touching the journal's Strand state. The [retained-history contract](../architecture/retained-history.md) was written, and graded by the independent query oracle, before any storage format was chosen.

## Decision

Each sealed server journal file (64 MiB by default) becomes one immutable **Segment**. A background sealer builds the Segment off the commit path.

**Layout.** A Segment is the directory `segments/seg-<label>`, where the label is 20 digits. It holds four Parquet files, compressed with Zstd level 3, with at most 8,192 rows per row group:

- `batches.parquet` holds every record exactly: group, label, receive time, SHA-256 and the exact batch bytes.
- `logs.parquet` and `metrics.parquet` are projections sorted by the query order: time, then node identity, sequence and index.
- `gaps.parquet` holds the collection gaps.

`manifest.json` is written last. It is version 1 and records:

- the journal label and group range;
- the record count;
- the receive-time bounds;
- the newest time for each node;
- each file's size, row count and SHA-256.

**Commit point.** The Segment is built in `.building-<label>`, with every file synced and the manifest synced last. It is then renamed, and the parent directory synced; the rename is the commit point. Only after that does the commit thread write the stream checkpoint and delete the journal file. At startup, `.building-*` and `.deleting-*` directories are removed.

**Retention.** Retention deletes whole Segments, oldest first, by renaming each one to `.deleting-<label>` first. The kernel `fabric_core::retention` decides which Segments to delete.

**Reads.** Queries prune with row-group statistics on the time column. The Segment's file size, schema and row count are checked against its manifest on each read, and a failure makes the answer incomplete. Queries then scan exactly. The unsealed journal tail is read by decoding frames. There is no token index.

## Alternatives considered

- **Journal only.** This is simpler, with no second representation. It was measured: every query kind was about 3 to 4 times slower at p99, and it used about 25% more live bytes ([history run 01](../experiments/benchmarks/history-run-01.md)). Journal-only reads remain valid for the tail and for the equivalence test.
- **An embedded database** (for example SQLite). Rejected in [ADR-0012](ADR-0012-add-a-server-crate-in-a-workspace.md) for the server's dependency and durability surface.
- **A token or postings index for text search.** The protocol registered one only if the 1,000,000-record query gate failed. The gate passed without it: text-search p99 was 465 ms or less with Segments.
- **Per-query whole-file SHA-256.** Rejected for its cost. `segment::verify` checks whole-file hashes on demand. The known gap is silent bit rot inside a readable Parquet page, listed in the [verification matrix](../formal/verification-matrix.md) under HIST-3.

## Evidence

- **Equivalence.** `journal_and_segment_representations_answer_identically` and the oracle-graded history tests show that Segments and journal answer the same. The semantic mutant M-HIST-SEGMENT is caught.
- **Corruption.** `a_corrupt_segment_makes_the_answer_incomplete` shows that a corrupt Segment makes the answer incomplete; M-HIST-CORRUPT is caught.
- **Latency and space.** [History run 01](../experiments/benchmarks/history-run-01.md) measured this under [protocol revision 2](../experiments/benchmarks/history-protocol-r2.md). Over about 1,033,000 records, every query kind had p99 of 481 ms or less. The data used 242 MiB at rest, against 304 MiB journal-only.

## Consequences

The server depends on `arrow` and `parquet`, which raises build time and memory, as recorded in the roadmap's known risks. Two representations must agree, so HIST-1 and HIST-2 are permanent checks.

Changing the manifest version, schema, file names or commit order is a persisted-format change. It needs explicit scope and a new ADR. Readers of version 1 must keep reading existing Segments or refuse them visibly.

## Validation

This decision is falsified by any of the following:

- a query over Segments disagreeing with the same query over the journal;
- a corrupt or missing Segment yielding `complete: true`;
- a crash leaving a Segment visible before its manifest is durable;
- the registered query gate failing on the target profile.

## Related

- [Retained history](../architecture/retained-history.md)
- [ADR-0012](ADR-0012-add-a-server-crate-in-a-workspace.md)
- [ADR-0013](ADR-0013-deliver-batches-in-order-with-bounded-dedup.md)
- [History protocol](../experiments/benchmarks/alpha-phase4-history-protocol.md)
