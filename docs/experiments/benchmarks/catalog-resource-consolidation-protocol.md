# Final catalog resource and evidence consolidation

Status: **draft awaiting registration; not executed**. No new resource allocation,
workload, verifier policy, performance threshold or frozen measurement identity.
Root runs after campaign jobs and reviewed failure archival/cleanup complete.
The nonfrozen `campaign_summary.py` audits original coordinator resource receipts
and persistent evidence. Frozen borrowed/maintenance drivers remain unchanged.

H1: all completed catalog jobs retain the20GiB/no-swap containment receipts and
complete owned persistent catalog evidence fits the registered2GiB aggregate.
H0: malformed/missing evidence, wrong containment or an observed aggregate excess.
CPU/RSS/IO observations describe jobs; they do not establish service acceptance.

Inventory includes top-level benchmark-data `catalog-*` directories AND files;
legacy coordinator/memory/query/recovery `catalog-*` directories AND files;
`failure-catalog-*` archives/manifests; coordinator launcher-receipts `catalog-*`
files; and legacy-root `catalog*.json` cleanup receipts. Every regular path is
counted logically, physical lengths and512-byte allocated blocks once per
(device,inode). Known `objects` directory links alone are allowed: CQ1 pairs2/3 to diagnostic
objects; CR2 freeze/plain1–3/counted1–3 to run01 objects; CQ2 rates0/1/4
preflight01/diagnostic01/pairs1–3 and rate0preflight02 to maintenance-objects01.
The helper enumerates exact relative link/target pairs, requires a real owned
target with no internal symlinks and independently inventoried regular inodes.
It never recursively follows link paths. Unexpected, outside, indirect, dangling
or uncounted targets fail closed; control fixtures exercise these rejections.
Directory alias logical bytes and symlink allocated blocks are separate outputs;
regular logical totals do not expand directory aliases. Link blocks join the
aggregate allocated-byte guard and are conservatively charged to both labs. Report
storage-location categories without summing their shared physical inodes.
Also report the broader legacy campaign and all benchmark-data footprint;
these overlap catalog totals and must not be added to them.

Enforce 2 GiB against both complete catalog unique-inode lengths and allocated
blocks. Certify registered query 1 GiB and capacity 768 MiB caps using conservative
upper bounds: top-level capacity-exclusive dataset families are spill,
builder-spill, borrowed, many-segment and lifetime; query-exclusive families are
boundary, maintenance, metadata, preboundary, evidence-pool and evidence-compaction,
as assigned by the query allocation (shared stores with observed cross-lab aliases
still charge to both).
Exclude an inode from a lab ONLY when every observed alias belongs to the other
lab's exclusive dataset family. Every uncertain inode, baseline freeze, shared
pool, legacy job/archive/receipt and cross-lab alias is charged to BOTH labs.
The output records the exact prefix lists and both per-lab bounds. They fit only
when both unique lengths and allocated blocks fit. If either bound misses,
report inconclusive pending evidence-based attribution refinement, not an actual
allocation overshoot or a relaxed cap. No silent cap split. Frozen narrower-prefix
inventory omissions remain explicit. External retained failure scratch is outside
persistent evidence until root's byte-verified archive cleanup; original failures
and cleanup receipts remain preserved.

Before actual inventory, contained controls must reject wrong memory maximum
and nonzero swap allowance, plus representative unique-length AND allocated-block
budget excess. A private data-drive fixture hardlinks one3-byte object across
five dataset/archive/manifest/launcher/cleanup paths; require logical15bytes,
one unique inode/3bytes, allocated-block deduplication and exact category inclusion.
Add query-only/capacity-only files and a cross-lab hardlink to check exclusive
subtraction, uncertainty double-charging and shared-inode treatment. Remove this owned fixture
on success or error. Never rewrite production evidence for controls.

Exact root command, using existing budget/admission and a fresh job identifier:

```sh
python3 tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-campaign-summary-01 --lab coordinator --stage verification --seconds 120 -- python3 -B tools/bench/labs/catalog/campaign_summary.py
```

Stop on failed controls, malformed/missing completed-job resource files, wrong
limits or excess aggregate. Preserve failure; do not weaken caps. Original job
states/exits remain unchanged. Report active receipts, original scratch cleanup
and later cleanup separately. The summary's own final output/receipt appears
after its snapshot; root must account for those small final bytes explicitly.
Recorded cgroup CPU/peak include source capture while elapsed omits it; sampled
process-name RSS/HWM is an individual-PID maximum, not a concurrency sum.
Device stacks and independently timed component peaks must not be summed.
This audit supports resource attribution and archive completeness only, not
fullfast success, all-race proof, optimization nomination or qualification.
