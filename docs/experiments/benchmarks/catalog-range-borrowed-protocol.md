# Borrowed raw records: a copy-allocation prediction

Registered before execution, following the [range-evidence
screen](catalog-range-evidence-protocol.md). The screen nominated direct
timestamp reduction, but its production-function confirmation did not reproduce
the 10% whole-scan improvement in two cells: only A met that threshold; B/C/D
improved 5.9/7.8/9.2%. All exactness, corruption and allocation guards succeeded.
Keep that result and the original decision rule unchanged. Do not resample it
until it passes or call the original speed hypothesis confirmed.

This is a distinct allocation-mechanism investigation: compare owned metadata
decoding with borrowed metadata decoding, with the same one-row Arrow reader
used in production. The previous default-reader Rows timings are context, not
this baseline. The primary outcome is eliminated copies; no replacement speed
threshold is introduced. The combined candidate may be retained on the explicit
allocation and non-regression evidence below, without claiming the previous
speed threshold or application throughput.

## Falsifiable model

The reader verifies SHA before constructing an owned Entry, including for
records outside the snapshot. The baseline moves that Entry's owned label into
the freshness map; it must not add the old probe's incidental per-included-label
clone. The candidate borrows label and Batch slices from the current Arrow
record batch. It allocates a freshness key only when a supported observation
first introduces a label. Both paths run identical typed validation and reduce
identical timestamps.

For all raw records i and distinct included freshness keys k, predicted requested
allocation saved per scan is exactly:

`sum_i batch_bytes_i + sum_i label_utf8_bytes_i - sum_k label_utf8_bytes_k`.

For nonempty raw bytes/labels in the timed fixtures, predicted allocation calls
saved are `2*N - K`. Every fixed fixture Entry has supported observations, so
K equals its included node cardinality. Empty/gap-only/unsupported records in
semantic controls do not add freshness keys. Test these predictions; do not
rewrite them after seeing discrepancies. They describe requested allocation
churn, not RSS, physical IO, or an equal reduction in live heap.

## Fixed comparison and controls

Keep seed 42 and cells A-D from the preceding protocol, including signal mix,
snapshot bounds, exact arithmetic expectations and raw bytes. Use five
alternating pairs (owned/borrowed, borrowed/owned, repeated), 64 full scans per
arm. Both owned and borrowed readers use Arrow batch size one. Fixture creation,
controls and output serialization remain outside timing. Measure warm-cache
wall/process CPU in a plain build and requested bytes, allocation calls and
incremental peak in a separate counted build. Counted time is diagnostic.
Record logical bytes hashed as fixture-derived work, not device IO.

The existing malformed-envelope/identity/payload, error-order, empty/zero/gap,
unsupported-metric, excluded malformed-OTLP, middle/empty-range, missing/corrupt
projection and excluded-digest controls remain required. Each timed arm must
match known fixture arithmetic. Add a Unicode/unequal-payload control: copy
borrowed records out of a callback, advance and destroy the reader, and compare
the retained copies with original producer entries. A late excluded digest
failure may follow valid callbacks but must reject the final staged evidence,
including a scan with no included records. The callback lifetime must remain
tied to the Arrow owner through a higher-ranked Rust callback; no unsafe code
or borrowed values escaping that owner enter production.

Existing owned `scan_batches` behavior stays intact through an owned wrapper
over the common reader. Its ordinary default Arrow batch size is unchanged.
Expose explicit owned/borrowed one-record scan APIs with actual query/probe
consumers; keep the reader implementation private. The Entry-based timestamp
helper delegates to the same byte-based reducer, preserving existing callers.
The query stages boundary metadata until scanning succeeds and clones a label
only on first insertion. No format, custody, integrity, retention or oracle
semantics change. Independent known-value timestamp tests and unchanged
snapshot-transition full-chain oracle tests run after the comparison.

## Decision and resources

Accept this mechanism only when every control and exact answer succeeds, every
counted pair eliminates the predicted bytes and calls, requested allocation
falls in every cell, and incremental peak never increases. Median paired plain
wall and CPU ratios must each be at most 1.05 in every cell. Report all ratios;
do not translate this finite result into service QPS or a confirmed speedup.
The numerical checker must reject altered predicted deltas and unsafe
regressions as well as missing/duplicate/misassociated samples and wrong output.

One new job, `catalog-range-borrowed-01`, uses preparation stage, 240 seconds,
two MiB evidence reserve after coordinator snapshot and a two MiB driver cap.
The exact decoded fixture-file cap remains two MiB. Root dispatches
`resource_group.py`, `completion/run_job.py`, `catalog/coupled_admit.py`, then
`catalog/range_evidence.py --mode borrowed`. Preserve all fixture bytes and raw
samples, source and binary hashes, command exits and negative-control evidence;
exactly read back archives before cleaning owned data-drive scratch.

The already registered `catalog-range-evidence-checks-01` remains the final
320-second verification job, with fast and manual documentation checks. No
time or evidence allocation is reset. About 954 frontier seconds and 322
verification seconds remain before this new job. The same mounted-drive,
16 GiB memory high/20 GiB max, no-swap and cgroup requirements apply. Preserve
and report an unexpected rejection or timeout; do not relax a guard to accept it.
