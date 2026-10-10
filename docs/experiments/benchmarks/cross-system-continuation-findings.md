# Cross-system dissection and diagnostic census

The [registered continuation](cross-system-continuation-protocol.md) downloaded
and dissected the remaining supplied FOSS, then ran bounded Fabric comparisons.
The [source synthesis](../../research/cross-system-source-synthesis.md) links the
three detailed dissections. Source mechanisms are hypotheses for Fabric; no
upstream implementation was benchmarked or adopted as a dependency.

## Source retrieval and evidence integrity

Twenty-one full archives were downloaded and their regular members hashed.
ClickHouse exceeded the 128 MiB compressed cap; Vector and FoundationDB's older
pins returned HTTP 404. Selected-file retrieval covered all three, using explicit
new immutable pins for the latter two. Supplement 02 includes actual FoundationDB
simulated persistence and Vector disk-buffer recovery paths; earlier wrong-path
404s remain recorded. Turso's earlier successful retrieval is separate.

All 24 retrieval records and both supplementary archives passed the
[independent evidence checks](data/cross-system-run-01/coordinator/source-checks-01/receipt.json),
exit 0. This validates retained artifacts and recorded failure prefixes, not
successful complete retrieval of the three failed archives. Rejection controls
covered altered, missing, duplicate, unsafe and linked archive members, invalid
pins, interrupted downloads and oversized response prefixes.

The review found an interruption-preservation defect in the new supplementary
fetcher: it removed completed temporary responses before the aggregate archive
existed. Responses and per-file receipts now stay on disk until exact aggregate
readback; the deterministic interruption control preserves the failed prefix.
Historical successful artifacts were rechecked and remain unchanged.

## Memory: physical layout and observer ownership

`memory/census-01` completed six native children but failed its wide-row metadata
comparison: reference filter groups 1, bounded groups 3, with identical 1024 log
rows and ordered custody. `FileEntry.rows` means physical group count for a
filter, logical row count for Parquet. ADR-0022 permits the byte cap to close a
group early. The failed run remains failed, exit 1.

A separately registered checker correction authenticates files, decodes each
Parquet layout with pinned PyArrow 22.0.0, reconstructs exact FTF1 Bloom bytes
from its row strings in Python, and requires own-layout alignment before
comparing logical metadata. Missing, misaligned and corrupted filters must be
rejected. This adds independent filter evidence; the ordered row ledgers still
use the same Rust reader and are not a full independent query oracle.

[Census 02](data/cross-system-run-01/memory/census-02/complete.json) passed all six
cells and these checks. Single-pair build-only measurements at 16 MiB input:

| Shape | Reference requested-live peak | Bounded requested-live peak | Instrumented build time, reference → bounded |
| --- | ---: | ---: | ---: |
| Steady | 98.99 MiB | 34.33 MiB | 346.1 → 369.3 ms |
| Adversarial | 99.28 MiB | 34.36 MiB | 375.6 → 393.0 ms |
| Wide rows | 95.04 MiB | 32.06 MiB | 262.4 → 260.0 ms |

These show 65–66% less requested-live build peak on these small fixtures; they
do not replace historical results at different sizes or establish a service
memory ceiling. Reservations are not observed. Native CPU/wall and Python
validation time are recorded separately.

Census 02's parent imported PyArrow before spawning native cells. Its bounded
`wait4` RSS rose to 93–106 MiB while post-exec sampled high-water remained around
51–57 MiB. This confounds a native-only peak interpretation. The registered
repair puts decoding in a separately reaped child, records spawning-parent RSS,
and reuses old archives only after exact path/size/digest equality. Historical
RSS values remain labeled; they are not silently corrected.

[Census 03](data/cross-system-run-01/memory/census-03/complete.json) completed the
repair and all six exact archive reuses, with unchanged native binary/fixtures:

| Shape | Reference whole-worker RSS | Bounded whole-worker RSS | Reduction |
| --- | ---: | ---: | ---: |
| Steady | 112.21 MiB | 54.23 MiB | 51.7% |
| Adversarial | 112.12 MiB | 51.53 MiB | 54.0% |
| Wide rows | 105.92 MiB | 57.20 MiB | 46.0% |

The spawning parent stayed below 35 MiB without PyArrow imported. Post-exec
sampled high-water and wait4 peaks agreed within about 0.33 MiB, supporting the
observer-contamination explanation for census 02. Whole-worker RSS includes
fixture generation and native validation; it is not build-only memory. These
remain one pair per shape, and `/proc` sampling can miss short peaks. Build
times in this replay changed +9.2%, +2.8% and −4.9%; no universal speedup follows.
The 405.3 MiB whole-job cgroup peak includes the isolated decoder and cache.

## Query: full-chain evidence and remaining block work

[Query census 01](data/cross-system-run-01/query/census-01/cleanup.json) completed
eight plain/counted variants: **512 complete chains, 3520 pages**, and 72 rejected
negative-control verdicts. All source and raw-data archive readbacks completed.
The unchanged oracle grades every continuation, not just the first page.

At 2048 rows with 1024-byte bodies, counted sealed selective queries requested
about 11.94 MB despite returning one row. Parquet loading/projection accounted
for about 92% of their counted query CPU. Scan and Walk were similar on these
selective fixtures; existing pruning strongly helped the empty-tail case.
Counted/plain timing differences also changed fault and system-CPU costs, so
they do not isolate observer overhead. Nested spans are not additive. Pure
plain full-chain query timing, controlled cold caches and service QPS remain
unmeasured.

The existing borrowed-log path is the next small-profile ablation. Its earlier
[CR2 experiment](catalog-borrowed-log-findings.md) already saved allocation at
65536 rows but failed CPU/wall guards and was not nominated. The new 128/2048-row
screen keeps that rejection, existing corruption controls and unchanged oracles.
It cannot nominate production using an easier allocation-only criterion.

The [small-profile ablation](data/cross-system-run-01/query/census-borrowed-01/cleanup.json)
completed 16 variants, **1024 accepted complete chains, 7040 pages and 144
rejected controls**, exit 0. All eight registered selective wide-body calls saved
2,110,304–2,110,340 requested bytes, meeting the at-least-1-MiB prediction.
This is allocation churn, not live peak or RSS. Sealed selective plain repeated
CPU medians changed +2.15% for Scan and +0.06% for Walk; their wall changes were
+1.01% and −0.64%. Other sealed populations were slower in this sample, up to
13.3% CPU. An already-borrowed tail control also varied by 18.5% CPU, limiting
causal interpretation of the timing differences.

Each cell has one fresh process per arm and three repeated calls; balanced order
across cells does not create independent within-cell replications. The allocation
mechanism reproduces at smaller scale, while a compelling CPU benefit does not.
CR2's no-nomination remains; no default changed. The useful next discriminating
question is avoided block decoding and its integrity/conversion costs, rather
than another claim that string-copy removal alone will accelerate queries.

## Operations: useful mechanism, inconclusive universal gain

The native Store/intake comparison uses the actual candidate sealer and an
example-local historical group-join baseline. Every case has 128 stream
identities, three initial journals and 96 live offers. Shapes are balanced three
workers, skewed three workers and a one-worker mechanism-negative control.
The three seed labels perturb body text, not independent scheduling distributions.

`prefix-load-03` completed 18 cells but used a 2 ms observer instead of the
registered 10 ms. It remains diagnostic. The corrected
[run 04](data/cross-system-run-01/operations/prefix-load-04/receipt.json) completed
18 cells at 10 ms: **144 accepted complete query chains, 36 rejected controls**,
exact custody/restart retry, and 96/96 accepted Batches in every case with zero
Unavailable. No HTTP/TLS or sustained-capacity claim follows.

Only one skewed pair had non-overlapping journal byte-time bounds, with
11.61–42.88% savings. The other skewed pairs and all balanced/one-worker pairs
overlapped. Skewed ACK p99 improved 15–20% in run 04, but one one-worker pair
worsened 6.3%; the 2 ms diagnostic had different adverse ACK outcomes. With
96 ACKs, nearest-rank p99 equals the maximum. These are finite observations,
not stable tail-latency estimates or a universal scheduler speedup.

The 10 ms observation widened byte-time uncertainty and did not resolve early
release before the middle builder's publication directly. Polling can also
perturb the workload; its interval width is not the only possible cause of
different runs. Exact checkpoint service duration remains unmeasured. A future
test should timestamp lifecycle events with measured observer cost and vary the
offered cadence around the commit thread's quiet interval.

## Reproducibility, failures and cleanup

Commands, exits, source/protocol snapshots, cgroup observations and cleanup are
under [`coordinator`](data/cross-system-run-01/coordinator); machine-readable
descriptive results are in [`report.json`](data/cross-system-run-01/report.json).
All project workloads used the resource launcher, 16/20 GiB high/max, no swap,
serialized admission and data-drive scratch/cache. No remote server was used.
Successful source archives/fixtures were removed only after exact preservation.
Failed retrieval prefixes and native failed-case archives remain. The
[cleanup receipt](data/cross-system-run-01/coordinator/cleanup-1791499378565043382.json)
removed only empty owned failure directories and charged the 3.280-second
pre-admission refusal to the continuation ledger.

Other preserved preparation failures were the example's use of a private clock
(compile exit 101), redundant nested coordinator lock, and absent output parent
(both exit 1 before a native cell). The example now implements the public clock
port locally; the runner respects the outer lock and creates its owned parent.
No production API was widened and no historical verdict was overwritten.

The finite census is partial M1/Q1 evidence. Mixed service memory reservations,
long soaks, persistence-fault campaigns, alternate formats, metrics/traces query
census and deployment qualification remain separate work. Verification and the
registered diagnostic follow-ups are recorded below as they complete.

`cargo xtask checks --profile fast` ran through the continuation launcher and
[passed all 17 gates](data/cross-system-run-01/coordinator/final-fast-01/receipt.json),
exit 0, 279.3 seconds outer service time, 575.1 MiB cgroup peak, zero swap/OOM.
This includes the workspace and existing rejected-row/corruption controls.
Later changes in this round are diagnostic Python drivers and documentation;
no subsequent Rust/product change invalidates that receipt.

The first manual documentation attempt returned exit 3, environment unavailable
because Bun was not on PATH. That result and all three unavailable receipts are
preserved in `final-docs-01`. Reusing the existing authenticated runtime archive
in data-drive scratch, the [retry](data/cross-system-run-01/coordinator/final-docs-02/receipt.json)
passed all three unchanged manual checks, exit 0. The runtime copy was verified
and removed; no hook or CI job was enabled. The
[resource summary](data/cross-system-run-01/resource-summary.json) and copied
verification receipts retain the current census of commands and limits.

This round's next research priorities are avoided block decoding with exact
integrity/fallback costs, completion-owned memory under simultaneous service
load, and event-based persistence/reclamation timing. Alternative formats and
failure-image campaigns remain unexecuted hypotheses. The source dissection and
the registered finite census are complete; the broader research goal is not.
