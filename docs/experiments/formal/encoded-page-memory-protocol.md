# Encoded-page memory counterexample probe

Status: prospective registration, 2026-10-09. No result is claimed. This is a new
probe of the opt-in aligned candidate, not a revision of historical bounded-sealer
acceptance gates. It changes no product default or persisted format.

## Hypotheses and mechanism

H0: the aligned candidate's incremental counted build heap remains at most
80 MiB on one full physical log row group of distinct high-entropy bodies.
H1: completed compressed pages retained by Parquet's default in-memory page
store push that heap above 80 MiB. Dictionary fallback near the default 1 MiB
limit makes a full-group dictionary an alternative causal explanation to reject,
rather than assume. Approximately 96 MiB of compressed base64 body pages is a
prediction, not a measured result.

The current candidate retains the reference 8192-row physical groups and uses
1024-row log input chunks with a 17 MiB resident-row cap, selected by
`FABRIC_ROW_GROUP_CHUNKS_EXPERIMENT=1`. No page-store change is implemented for
this registration. Only after observing a counterexample may a separately frozen
candidate use `FABRIC_PAGE_STORE_EXPERIMENT=1` with Parquet 60's
`ArrowWriterOptions::with_page_store_factory` to spill
completed serialized blobs. Such a comparison must preserve encoding properties,
input chunk boundaries, physical groups, filters and output bytes. The future
candidate sets both `FABRIC_ROW_GROUP_CHUNKS_EXPERIMENT=1` and
`FABRIC_PAGE_STORE_EXPERIMENT=1`, leaving other experiment flags unset. It spills
all table columns, including raw chunks, and records the resulting I/O overhead.
With `responsibility-alloc-probe`, per-table stderr observations count exact
completed serialized blob bytes, their peak outstanding bytes and largest blob;
these supplement the counted heap comparison without changing page production.

## Fixed fixture and observation

[`completion_builder.rs`](../../../crates/fabric-server/examples/completion_builder.rs)
adds `entropy-gen ROOT 8192`. Seed is `0xA11FA001`; exactly 8192 log rows each
contain 16384 characters from the base64 alphabet (128 MiB logical body bytes).
Every body has a 16-character row-index prefix guaranteeing distinctness; the
suffix uses the existing xorshift generator, with no repeated `R` rows. One node,
generation 1, sends 32 rows per Batch and one Batch per Group: sequences 1 through
256. Row index `i` has observed time `1800000000000000000 + i*1000000000`;
received time is the group's last row time plus 5000000 ns. There are no metrics,
spans or gaps. Existing Batch, Group and FrameLog codecs are used unchanged.
Generation retains only the current group and is excluded from build metrics.

Before measurement, record the source revision/diff, binary SHA256, compiler and
features, all compile-time experiment flags and this protocol's hash. Freeze the
binary first. Use its unchanged `run NEW_STATE INPUT reference|bounded` command
for both builders, in fresh processes and fresh owned state directories, against
the identical sealed input hash. Record exact argv, environment, exit status and
stdout/stderr. One reference/bounded pair is a diagnostic counterexample screen;
it establishes no timing advantage. Any future page-store pair must use the same
input and a separately recorded binary hash.

All commands run through `python3 tools/resource_group.py -- COMMAND ...`.
The owner supplies exact frozen binary and fresh paths under
`FABRIC_SCRATCH_ROOT`; these command forms are prospective:

```text
completion-builder-frozen entropy-gen SCRATCH/entropy-fixture 8192
completion-builder-frozen run SCRATCH/entropy-reference SCRATCH/entropy-fixture/sealed-00000000000000000001.faj reference
completion-builder-frozen run SCRATCH/entropy-bounded SCRATCH/entropy-fixture/sealed-00000000000000000001.faj bounded
```

## Decision rule and containment

Require successful builds, exactly 8192 decoded logs, 256 raw batches, and identical
ordered logical ledgers. Record every manifest file hash/byte/row count and require
the existing applicable-byte rule for `logs.parquet` and `metrics.parquet`, plus
exact `text_filter.bin` bytes in this new probe. Require identical physical log group boundaries, complete
text-filter bytes, filter observations and all existing 10/60-second pruning
windows. Raw batch byte layout remains independently recorded; its ordered
logical ledger must agree under the existing chunk-flushed raw policy. Any
correctness mismatch rejects the candidate. Empty gap output must also agree.
Preserve actual mismatches and the fixture hash as a counterexample receipt.

Record incremental counted build heap (peak minus pre-build live bytes), allocated
bytes/calls, elapsed build time, process CPU stat snapshots and `/proc/self/io`
deltas from the existing `run` boundary. H0 is rejected if the bounded build's
counted incremental peak exceeds 80 MiB (83886080 bytes); the reference is an
exactness baseline and its heap is recorded without that ceiling. A result above
the ceiling supports the counterexample, but does not alone attribute bytes to
page storage. Source evidence plus a future isolated page-store comparison can
distinguish that mechanism. Passing this finite probe establishes no universal
heap bound for arbitrary rows, metadata, cardinality or segments.

Use data-drive build caches, scratch and external result receipts. The lab remains
20 GiB maximum with no swap; any server child remains at most 4 GB. Total storage
remains at most 100 GB. Observe available storage before admission; do not execute
alongside another admitted heavy workload. Record cleanup outcome, retain failure
evidence externally, and remove only owned fixtures/state after evidence is saved.
New spill backends would additionally require exact byte comparisons and injected
write/read/cleanup failures before any promotion decision.

The page-store candidate's native fault screen uses
[`page_store_faults.py`](../../../tools/bench/labs/completion/page_store_faults.py)
and the unchanged existing `io_fault.c` interposer. A fresh steady-16 fixture,
clean positive control and deliberately nonexistent-path negative control precede
seven cases on `/logs.pages/`: write ENOSPC, read EIO, short-write then EIO,
short-read then EIO, SIGKILL on write, SIGKILL on read and cleanup failure. The
cleanup case pauses at the first native page read, replaces that owned page
pathname with a directory while retaining the open readable inode, then resumes;
unlink must fail and prevent publication. Require an actual injection marker,
nonzero exit (SIGKILL cases exactly -9), unchanged sealed input and no ordinary
error scratch before retry. Capture evidence before recovery, then require exact
clean-control manifest, no scratch and successful retry. Freeze/hash helpers and
the interposer, retain child command/exit/stdout/stderr receipts, and remove owned
fixtures only after all cases meet their predefined outcomes. A missed injection
or unsuccessful containment/cleanup is a failed screen, not a passing fault.
