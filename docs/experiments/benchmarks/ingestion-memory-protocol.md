# Ingestion memory reduction protocol

Registered on 2026-10-04 before candidate measurements, following the owner's
request to prioritize ingestion memory growth. This is implementation and finite
exploratory verification, not an installed-default change or qualification run.

## Mechanism and invariants

Baseline: the current whole-file `segment::read_sealed` plus `segment::build`.
Candidate: one frame at a time, capped sorted runs for logs, metrics and spans,
at most sixteen merge readers, bounded Arrow chunks, no whole-file raw-entry
clone. Preserve schemas, manifest semantics, exact original Batch bytes, stable
row order, filters, two-sync ACK and publication/checkpoint/reclaim order.
Private spill encoding must preserve floating-point bits, including non-finite
metrics, rather than using JSON floating-point numbers. Spill files are not a
new durable format. Stable ties retain journal order, including unusual inputs
with repeated row keys. Release sorting buffers before merging. Delete owned
build directories on errors; leave the journal unchanged.

ADR-0022's opening claim of unchanged bytes is interpreted through its explicit
row-group byte-cap exception and BS-1: semantic equality and deterministic files
are required; raw/gap files and byte-capped row groups may have different hashes.
The implementation includes spans (ADR-0025) and bounded fan-in, which the older
two-sorted-table diagram omitted. It does not claim the full bounded-sealer
milestone, its unchanged soak, or its registered four-shape suite completed.

## Measurements

Use fresh processes, fixed seed 42, shuffled 1,024-byte log bodies with the
existing fixture's attributes, 128 logs per Batch. Screen 16, 64 and 256 MiB
encoded journal files with one builder, three repetitions of each baseline and
candidate, alternating order. Record actual journal/record sizes, not nominal
raw-body sizes. Count incremental live Rust heap above the post-fixture baseline
over read-and-build, wall/CPU time, whole-process peak RSS, spill/output bytes,
and leftovers. Fixture creation and exact-answer grading are outside the timed
build, and retained fixture memory is reported. RSS may include earlier fixture
creation and allocator retention; it is not the incremental heap metric.

Hypothesis: candidate incremental peak heap is at least 50% below baseline at
64 MiB and no more than 80 MiB at all three screened sizes. Failure does not
permit changing the ceiling. Report timing/IO regressions, do not discard them.
Keep unstable and failed trials. This finite screen is not proof for arbitrary
record sizes, shapes, node cardinalities or concurrent ingress/query loads.

Compare exact raw Batch bytes and every extracted row against the frozen
whole-file reference; run unchanged independent-oracle HTTP/history tests over
the product path, including traces, metrics/rates, scan/walk and restart.
Inject reversed-run ordering and truncated/corrupt input as negative controls
for merge equality and error cleanup. Exercise more than sixteen runs with
small test-only limits, stable ties, non-finite numbers and big rows.

Run sequentially in verified cgroups (20 GiB local cap, no swap), using the
mounted data drive for all scratch and build outputs. Record actual revisions
and working-tree/binary hashes. Persist summaries, failures and cleanup receipts
before deleting owned trial directories. Do not run remote loads or soak as
part of this screen. Run fast checks and report actual exits.

## Measurement prerequisite correction

The pre-existing page-compression probe could not compile because Parquet's
compression module is private without its experimental feature; enabling that
feature required an uncached new dependency. Use the already-present Zstd bulk
API for a **standalone** compression/decompression measurement of decompressed
Parquet page bytes instead. Its labels must say standalone; it does not isolate
the exact Parquet codec implementation or claim native writer codec costs.
This correction is registered before running that population.
