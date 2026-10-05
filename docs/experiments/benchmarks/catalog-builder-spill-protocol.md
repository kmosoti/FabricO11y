# CR1 full-builder confirmation: reusable private spill workspace

Status: **registered before execution; no outcome claimed**.
Parent [codec findings](catalog-spill-findings.md) nominate combined reuse,
not a sealing speedup. This compares the production bounded build algorithm
with experimental private spill workspace reuse; no format/oracle/default change.
Historical C2 large-row pruning equivalence remains failed.

H1: reuse reduces full-builder cumulative requested allocation bytes >=10% in
every matched pair, with <=5% regressions in uninstrumented build CPU/wall and
counted incremental peak heap. H0: codec savings disappear in whole-builder
allocation or violate CPU/time/peak guards. Allocation traffic is the primary
cost, expressly not a throughput or service-memory claim.

The build-time `FABRIC_SPILL_WORKSPACE_EXPERIMENT` variable is absent for baseline
and set to1 for candidate; rustc tracks `option_env!` as a build dependency.
Default builds retain original spill calls. Four new frozen binaries cover
baseline/candidate times plain/counted. `responsibility-alloc-probe` selects the
existing System-delegating benchmark allocator; plain heap counters are null and
marked uncounted, never interpreted as zero allocations. Successful counted
alloc/realloc requests report full requested sizes and call count.
Each merge reader owns <=256KiB byte workspace: sixteen readers <=4MiB;
one spill/intermediate writer adds <=256KiB. Decoded owned heads, row vectors,
IO buffers and output state remain separately charged. Oversized rows use
discarded temporary storage and retain the existing16MiB private-row ceiling.
No unsafe candidate code; unsafe allocator observation is benchmark-only.

## Fixtures and executable guards

Use the original C2 generator and seed0xA11FA001, not the codec screen's seed:
steady64/256MiB, adversarial64MiB and bigrows64MiB. Regenerated input hashes and
every final manifest must equal their historical bounded counterparts. Three
alternating fresh-process pairs per fixture, separately plain and counted:
48 measured builds. Input cache is warm and shared within each cell; no eviction.
Match build settings, CPU affinity and host conditions. Record interruptions.

Candidate unit controls run first through actual production merge/codec: fixed
scalar signedness/float bits, every projected field, malformed/truncated frames,
maximum-row-then-small capacity, changed-field rejection, stable ties and real
16/17/18-run boundaries; existing tests also cover higher full/partial boundaries
and cleanup on corrupt input. Byte-capped tiny runs provide boundary controls
without huge fixture generation. New harness negative controls reject changed
manifest, ledger, pruning and escaped scratch while accepting unchanged rows.
Performance gate controls reject a91% requested-byte ratio and106% peak/CPU/wall
ratios while accepting85% allocation and unchanged other costs.
No control expectation or existing acceptance gate is edited.

Each measured pair requires identical complete manifest/file hashes, ordered
row ledgers/counts, physical filter validation, all registered pruning windows,
raw Batch custody and empty spill/build leftovers. Readback/validation occurs
after build snapshots; an error preserves owned state and exact command receipt.
Differential equality is against the bounded baseline, not the whole-file
reference whose historic bigrows pruning failure remains a separate result.

## Measurement and decision

Build phase wall and process-jiffy CPU include read/decode, projection, sort/spill,
merge, Parquet/filter creation, sync/manifest publication; exclude generation and
independent readback. Report SC_CLK_TCK resolution and avoid short-cell claims.
Whole-child CPU/wall additionally include validation and are retained separately.
Counted heap/cumulative/call snapshots stop before readback; post-build live
heap includes allocator-requested ownership, not RSS or page-cache reclamation.
Retain phase proc-IO logical/physical counters, output bytes, worker count1,
whole-job cgroup anon/file/kernel/pressure/events/swap, peak scratch and cleanup.

Nominate only if ALL shapes/pairs preserve exactness and candidate cumulative
requested bytes <=90% baseline in every counted pair; candidate incremental peak
<=105% baseline and <=80MiB every counted pair; candidate plain phase CPU AND
wall <=105% baseline every pair. Record medians and individual ratios. No blanket
winner if one shape fails. Scaling at steady256 must remain <=110% worst steady64
candidate peak. These are new confirmation guards, not historical C2 replacement.
Performance failures complete collection with explicit false nomination gates;
command exit0 means completed collection, not a passing nomination decision.
Counterexamples include sixteen oversized retained workspaces, fewer allocations
but more peak memory/CPU, output drift, changed error/cleanup behavior and
allocation savings with negligible build-time gain. Failures remain evidence.
Spool/ACK/query and fixed-demand service guards remain required at F1 before any
production nomination. This experiment supplies no service latency result.

## Serial execution and artifacts

Root registers before measurements, then uses this contained command:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-builder-spill-01 --lab memory --stage capacity --seconds 1500 -- python3 -B tools/bench/labs/catalog/builder_spill.py --out docs/experiments/benchmarks/data/catalog-builder-spill-run-01
```

Driver builds/freezes all four binaries before measurements and checks mode
markers/binary hashes. Preserve the existing historical frozen completion builder
archive; new shared target builds must not overwrite that evidence. Source hashes,
revision, flags, commands/exits, raw stdout/stderr, pairs and compressed new binaries
belong to the new artifact root. Compilation/controls are part of this budget.
No workload admission if1500s exceeds remaining shared/stage time. Individual
controls/builds <=600s, fixture generation <=300s, measured children <=180s;
outer coordinator deadline wins. Maximum scratch8GiB, free reserve16GiB; default
20GiB/no-swap containment and mounted data-drive scratch, no fallback. Driver
removes successful states/fixtures/frozen scratch after archival; coordinator
archives observations/failures and records whole-job cleanup. Root reports
environment unavailable, failed or interrupted exactly and does not shorten a
launched cell or change its guards to obtain nomination.
