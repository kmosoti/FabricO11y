# S3/S4: hybrid Parquet projection and selective postings

Status: registered before implementation and timing. [The exact contract](../../../tools/layout-probe/API.md)
defines a separate research package. No production format migration is selected.
The official [Parquet Rust documentation](https://docs.rs/parquet/60.0.0/parquet/)
describes Arrow batch writing and projected reading; the proposed schema and
correctness boundary here are Fabric experiments, not guarantees from that library.

## Hypotheses and workload

A hybrid Parquet layout may reduce stored bytes and parsing work relative to S2 JSON
blocks, while selective postings may repay build cost for repeated token queries.
Both hypotheses can fail. Full Event equality, including raw float bits and duplicate
physical positions, is mandatory before timing. An independent scalar oracle and
malformed-input tests are frozen before implementation.

Use mixed and shuffled-Log S1 workloads, seeds 201/202/203, 2,048 events and 128 queries
per snapshot. Repeat with 8,192 events for seed 201, scaling the timestamp modulus
and fourfold repeating the predicate cycle. Include a separate untimed edge corpus
with every scalar and payload variant, NaNs, infinities, signed zero, extreme IDs and
times, Unicode tokens, empty values and duplicate attributes/Events. The performance
corpus remains unchanged by codec edge-case tests.

Compare these physical layouts: S2 JSON row blocks of 64 events; Parquet row group
64 uncompressed; row group 64 Zstd level 3; then row group 256 Zstd level 3. Change
compression before changing row-group size. Keep remaining writer defaults fixed.
Use the identical fresh-file write, file sync and parent sync boundary for all.
Create durable parent directories outside timing. Include metadata/index bytes in
reported totals and retained source bytes separately; no compression ratio may hide
an additional copy needed by the correctness contract.

## Procedure and measurements

One warmup followed by five measured trials per dataset/configuration, alternating
layout execution order. Time encoding plus durable publication, separate file read,
full materialization query and projected query (with full-file authentication in
both query timings). All 128 predicates are checked against independently computed
positions and single-row digests. Record P50/P99 and each predicate family, bytes
written, serialized metadata bytes, process CPU and peak RSS, plus output hashes.
CPU is whole-process unless a separately documented phase timer is added before
measurement. A fresh process is process-cold, not a cold OS page cache. Do not drop
system caches or claim physical device I/O from userspace byte counts.

For S4, build only the exact-token posting list for `rare`, keeping its full table
binding and external hash. Measure index build time/bytes and projected queries with
and without this index. Also run absent, corrupted and wrong-table indexes; all must
fall back to projection and retain exact answers. Rebuild from retained rows and
require identical deterministic bytes. Never filter ingestion using this index.
Report the measured break-even query count as ceil(build cost / per-query saving)
when saving is positive; otherwise report no measured break-even. Account for full
file authentication and any projection decoding that remains even with postings.

## Gate and decision

All correctness checks must pass, and every mutated checker must fail the intended
probe. Report distributions and paired results without a universal format-winner
claim. A candidate is interesting for this workload if median bytes are <=80% of
JSON and median projected query elapsed time is <=110% of JSON exact scan; evaluate
these gates per workload, never pool sizes or hide unfavorable query families.
Postings need positive measured per-query savings after authentication and report
amortization. A negative result closes the experiment without forcing a migration.
The S2 JSON prototype remains the baseline until evidence justifies another choice.

This measures a hybrid raw+projection layout, not fully shredded attributes, object
storage, a distributed query engine, kernel cold-cache latency or hardware offload.

## Timing implementation boundary (registered before the harness)

On this Linux host, phase CPU uses CLOCK_PROCESS_CPUTIME_ID alongside Instant wall
time. Record timer-call overhead with an empty calibration loop; do not subtract a
noisy estimate from samples. Whole-process CPU/RSS are reported separately and include
setup/checks. Gates using server/build/query CPU refer to the named measured phase,
not the sum of unrelated validation work. All results must retain these boundaries.

Pre-formal-run clarification from harness review: report pure postings construction
and durable index publication separately. Amortization uses their sum, including
index/digest file writes, file syncs and directory sync; it must not advertise that
sum as an in-memory construction time. Queries retain the index bytes and external
digest in memory, while reading the table file for each operation. Label this
resident-index condition. Include the independently retained table-anchor encoding
and index-digest encoding in storage totals; Parquet footer bytes are already part
of the Parquet file and must not be counted twice. JSON's per-block hash list is its
corresponding external metadata. These definitions were clarified using smoke and
source review, before formal timings or gate decisions.
