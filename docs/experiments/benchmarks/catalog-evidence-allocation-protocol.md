# Catalog evidence allocation review

Registered before further workload admission, 2026-10-05. This is a storage
allocation amendment, not a change to correctness/performance acceptance.
The frontier plan permits a separately reviewed evidence allocation. Root's
aggregate audit exposed per-slice accounting that exceeded the original 256 MiB
query allowance. The original overshoot and `catalog-evidence-compaction-01`
exit 2 remain recorded. Lossless compaction verified every transformed byte but
left 285,798,257 unique file bytes (286,859,264 allocated blocks), plus its receipt.
No subsequent workload was admitted before this review.

For the remaining catalog campaign, persistent evidence is capped at **1 GiB
for query**, **512 MiB for capacity**, and **2 GiB aggregate catalog evidence**,
including shared pools, frozen executables and archive-transformation receipts.
Earlier completed-campaign evidence is separately inventoried and included in
the reported total repository footprint; it is not deleted to satisfy this cap.
The query allocation includes CQ1, CQ2, precursor metadata and preboundary
binaries. Capacity includes CR1, CR2 and CR3. Coordinator and recovery archives
remain in the aggregate and retain existing per-directory guards.

Reason: complete independently checked pagination and frozen binaries are
valuable reproducibility evidence. Full query slices take about six minutes;
repeatedly redesigning archive encodings to save a few hundred MiB costs time
and risks losing provenance. The mounted data drive has over 430 GiB free at
review. The finite query allocation comfortably covers the observed roughly
200 MiB per slice even without cross-slice compression, while a shared object
store and exact JSON compression reduce duplicate storage. CQ2 will retain
byte-identical row chunks and enforce its declared bounds. Every future driver
must account for its full campaign tree/shared store, not only its leaf directory.
Stop admission if projected or observed retained bytes exceed the reviewed cap;
missing evidence cannot be converted into a successful experiment.

Unique inode file lengths and allocated blocks are both reported; aliases count
once physically. Logical path-size sums are also reported and never relabeled as
physical storage. Representation receipts preserve old compression hashes and
unchanged decoded bytes. The compactor may accept an explicit `--cap-mib 1024`
for this query allocation; its default stays 256 and the original exit stays failed.
The CR2 aggregate evidence check uses the reviewed capacity allocation of 512 MiB,
including its shared objects and completed slice outputs. This is not a relaxed
oracle, fewer trials or a changed performance threshold.

**Unchanged:** 16/20 GiB cgroup memory high/max, zero swap, one workload at a time,
8 GiB owned scratch, 16 GiB free-space reserve, existing stage limits, shared
14,400-second frontier execution budget and 30-minute outer deadline. Builds and
temporary fixtures still use the mounted data drive. Clean success scratch;
byte-verify preserved failures before cleanup. This allocation authorizes no
qualification run, release, persistent-format change or production default change.
