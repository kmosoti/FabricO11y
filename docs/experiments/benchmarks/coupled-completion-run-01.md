# Coupled labs completion execution

Status: admitted work recorded and verified; larger service cells remain deferred by evidence admission.
Owner authorized implementation with “Proceed”. The [completion plan](coupled-completion-plan.md)
and [dispatch](coupled-completion-dispatch-protocol.md) retain the existing frontier,
20GiB cgroup/no-swap envelope and mounted data-drive scratch. No qualification or
production overlap enablement is part of this run.

## Operations: availability and pruning

The availability companion independently preserves producer-derived retained
metadata while excluding only missing projections. The original whole-record
Python oracle is unchanged. Nine checker tests passed in
`catalog-coupled-o6-checker-01`; partial projection loss passed six Scan/Walk
complete chains in `catalog-coupled-o6-projection-02`. Its evidence was moved
byte-preservingly from Cargo's crate-relative directory to the intended absolute
root; [path correction](coupled-availability-path-protocol.md) records that fix.

Raw-table loss reproduced an actual defect:48 readable log rows were returned
with `complete=true` after `batches.parquet` disappeared. Raw-red01 exited101.
Implementation commit73447a2 checks raw presence, size, schema and footer row
count before claiming completeness, preserving readable projections/metadata.
Raw-fixed01 passed six chains. Integrity01 passed24 chains covering truncation,
same-size damaged footer, wrong valid schema and wrong manifest row count across
logs/metrics/spans and both plans. Source-errors01 passed the selected missing-gap
incompleteness and missing-Manifest interrupted-source tests. These checks do not
establish arbitrary page-corruption detection: no whole-file checksum or full raw
page decode runs on every query. The added footer-open cost has not yet been
isolated in a performance comparison. Companion raw-loss grading is scoped to matching,
nonempty queries. See [availability decision](coupled-availability-protocol.md),
[counterexample registry](../../formal/counterexamples.json) and
[retained evidence](data/catalog-coupled-availability-01).

O7 now decodes actual physical groups and independently checks producer identities,
order and conservative bounds. The first native fixture failed because64 large
rows exceeded the Batch limit; its replacement uses32 rows/Batch and35 Batches.
Native02 passed12 chains. The subsequent empty-window discriminator and selected
8/16/32MiB variants each passed14 chains (42 total):1100 rows,16KiB bodies, late
arrival, equal timestamps across groups and an empty window admitted by a broad
group bound. Every final manifest file entry (length, rows, SHA256) agrees across
variants, verified by `catalog-coupled-o7-variant-compare-01`.

The checker rejected omitted admitted groups, narrowed bounds, missing groups
and changed content; unchanged query oracle also rejected incomplete/duplicated
chains. Actual group sizes are508/508/84. The empty internal window matches zero
rows while conservatively admitting508. This is footer-bound soundness, not
measured query IO or proof for every signal/workload. Historical C2 physical-layout
equality remains failed. [Prospective discriminator](coupled-pruning-false-positive-protocol.md)
and individual `catalog-coupled-o7-r{8,16,32}-01` datasets retain native binaries,
producer records, page chains, SHA-checked gzip archives and verdicts.

## Collection: one-successor overlap

The experimental Rust method keeps Spool, cursors, metric history and ACK
persistence on the main thread. A scoped worker sends immutable Batch N; at most
N+1 can be committed during its wait. Retry/backlog cannot prepare N+2, and an ACK
cannot be legitimized by a newly prepared unsent successor. Production CLI stays
serial; trace listeners are outside this candidate's scope.

Six deterministic controls passed in controls02. Controls01 failed because its
6000-byte line was correctly skipped by the4096-byte source limit. Three valid
2000-byte lines now establish the full-Spool precondition; original ACK/cursor
assertions remain. Three TLS harness attempts were retained: missing `serve`, a
too-short fixture credential, and an invalid `limit` field on a Rate request.
These are harness failures; only pair1-04 completed the registered matched pilot.
Frozen native executables remained identical through those driver corrections.

[Pair1-04](data/catalog-overlap-pair1-04/comparison.json) used one actual node per
arm,250 deterministic512-byte offered log bodies over15seconds,20seconds of node
operation and a matched50ms delayed TLS Batch response. Independent custody/body
checks and three final logs/metrics/rate chains per arm passed, with no retries,
full ACK drain and explicit service cgroups (node64MiB/one CPU, server384MiB/two
CPUs, no swap). Parent131.9MiB peak includes harness/cache; it is not node RSS.

| Observed quantity | Serial | One-successor overlap |
| --- | ---: | ---: |
| Accepted source logs |250|250|
| Committed and ACKed Batches |151|153|
| Encoded Batch bytes |248153|249154|
| Source-to-client-ACK median |67.33ms|67.75ms|
| Source-to-client-ACK maximum |92.28ms|134.13ms|
| Post-Spool-to-client-ACK median |58.92ms|59.17ms|
| Node CPU seconds |1.019565|1.039365|
| Server CPU seconds |0.590973|0.585832|
| Node cgroup memory peak |3.56MiB|4.01MiB|
| Server cgroup memory peak |7.05MiB|7.10MiB|
| Node CPU seconds per logical log MiB |8.3523|8.5145|
| Successor commits before previous ACK |0/150|2/152|

Both service groups recorded zero memory-high/max/OOM events and zero CPU quota
throttling. Cgroup peaks differ from RSS: shared executable/cache pages can be
charged to the parent, where the harness unpacked binaries. Five-second per-PID
RSS/HWM and IO samples are retained separately. Source acceptance was250/250,
with128000 logical log bytes in each arm and no refused/retried Batches.

The one pair shows no useful light-load benefit: median latency+0.63%, node
CPU/accepted-byte+1.94%. Only2 successors actually overlapped. This does not refute
an overlap benefit under backlog, establish a repeatable regression or support a
throughput/p99 claim. The next useful discriminator is a bounded backlog large
enough to keep preparation busy, before multiplying this workload across20 nodes.
No additional burst/small cell is represented as complete.

Visibility polling retained every answer and first-seen interval at100ms cadence.
ACK-to-first-seen upper differences ranged up to59.19ms serial/57.07ms overlap;
119/127 negative values mean data was visible before the delayed ACK. These are
interval observations, not an instantaneous freshness or30second guarantee.
Detailed controls/metrics are in the [pilot protocol](coupled-overlap-protocol.md)
and per-arm result files. CPU and RSS/HWM samples, IO and memory pressure remain
in the coordinator's resources ledger; sampling does not equal exact native peaks.

## Capacity preparation and admission

The explicit compile selector accepts8/16/32MiB and defaults to16MiB. The same
spilling/invalid-value/oversized-row controls passed under all three selected
builds in `catalog-coupled-c5-selectors-01`. The cap is a per-signal resident
estimate, not a whole-builder heap ceiling. O7's finite output check passed above.

The proposed service adapter holds real pending journals until measurement starts,
observes `.building-*` concurrency and preserves query-on/off schedules. A
reversible evidence codec preserves original JSON framing, order, timestamps and
actual Batch hashes while regenerating only verified deterministic body hashes.
Its archive, measurement, custody and cadence controls passed in
`catalog-coupled-c5-controls-01` (exit0). The exact source-input projection passed
count/hash readbacks for300000 lines and measured111046844 compressed bytes,
111083520 allocated bytes (105.94MiB), before seed/journal/Spool/receipt costs.
Projection01 exited0 and removed its owned temporary files. Admission01 exited0
for completing the assessment, explicitly `admitted=false`; no native service
child started. See [projection](data/catalog-borrowed-c5-run-01/projection.json) and
[admission](data/catalog-borrowed-c5-run-01/admission-only.json). The no-query
control, four query grid cells and32MiB holdout remain unrun. This storage result
is specific to per-node gzip preservation, not a lower bound for every codec. [C5 registration](coupled-c5-protocol.md) leaves the20-node native
grid conditional on preserving failure evidence within its20MiB assignment.

## Containment, cleanup and remaining work

Every workload ran through `resource_group.py` and the serial completion launcher;
commands, exits, working-tree sources/diffs and cgroup CPU/memory/IO observations
are retained under [coordinator receipts](data/lab-completion-run-01/coordinator).
All temporary paths use the mounted data drive. Failed scratch is archived with
file-by-file byte verification before deletion. The [catalog cleanup protocol](coupled-cleanup-protocol.md)
uses the already reconciled aggregate/query/capacity assignments; the historical
legacy-cap failures are not regraded. Shared caches remain, and no remote host was
loaded. The unchanged tighter aggregate ceiling is1,894,879,232bytes; the final
snapshot includes accumulated failed pilot binaries and receipt overhead.

C4 and Q4 remain closed at their earlier inconclusive/no-nomination outcomes.
C5 input preservation/admission is complete with a rejected native dispatch as
recorded above. Fast01 exited1 (15 gates passed, workspace tests and Clippy failed).
Its legacy raw replay test still expected complete raw custody through the old
whole-record oracle; the companion routing now retains all five original queries
in both plans and the replay-error assertion. A nested overlap-test conditional
was collapsed for Clippy. The [retry registration](coupled-completion-regression-retry-protocol.md)
records the separate fixture decision. Original failure files and verification
receipts were preserved before Fast02. Fast02 exited0 in255.932seconds: all17
fast gates passed, including the complete workspace tests and Clippy. Its cgroup
peak was0.87GiB with no swap, and owned scratch was removed.
The remaining manual documentation/audit command is
`resource_group.py -> run_job catalog-coupled-completion-closeout-01 -> coupled_admit --reserve-mib 1 -> coupled_closeout.py`,
verification stage120s. The wrapper copies exact fast receipts, runs the manual
documentation profile, checks this completion's failed scratch is archived/removed,
and reconciles unchanged evidence caps. No further native performance cell is admitted. No default optimization or
alpha qualification follows from this finite laboratory run.


## Final closeout

`catalog-coupled-completion-closeout-01` exited0 in27.533seconds.
Manual documentation links/syntax, checker probes and hook tests all passed; this
did not re-enable automatic Bun hooks or CI. Exact fast receipts were copied and
byte-checked. The audit found all39 preceding completion jobs terminal and their
owned failed scratch archived/removed; this final job also removed its scratch.
Shared caches and unrelated historical failure evidence remain. No remote tests
ran. These final receipt-only status updates follow that documentation check.

At the admission wrapper's last snapshot, aggregate allocated evidence was
1,893,703,680bytes versus1,894,879,232; capacity847,093,760 versus872,415,232;
query upper1,046,609,920 versus1,073,741,824. Final receipt/log blocks follow the
snapshot; roughly1.1MiB headroom cannot admit another native campaign. Historical
768MiB capacity violation remains. The shared frontier used10611.680 of14400seconds
(3788.320remaining); storage admission, rather than wall-time or the20GiB
memory envelope, limits further dispatch.

The completed scope is availability repair, finite pruning/output preservation,
selector/archival controls and a correct light-load overlap pilot. C5's six native
cells and O8's small/backlogged follow-up remain unrun. A scoped preservation plan
that fits measured failure costs is needed before restarting them. The tested
working tree passes all17 fast gates; no release or deployment qualification is
claimed. See the final [audit](data/catalog-coupled-completion-closeout-01/result.json)
and [queue](coupled-lab-queue.json).

## Continuation

Owner requested continuation. Three existing labs prepared a lossless evidence
reclamation, a backlog discriminator and a small-profile pair. Root reviewed,
registered and serialized execution; lab agents did not run competing workloads.
[Reclamation registration](coupled-reclamation-protocol.md), its
[retry](coupled-reclamation-retry-protocol.md), and the
[backlog](coupled-overlap-backlog-protocol.md)/[small](coupled-overlap-small-protocol.md)
fixtures preceded their respective execution. Aggregate/query/capacity ceilings
and the14400-second frontier were not increased.

`resource_group.py -> coupled_reclaim.py` reclaim01 exited1 at180.905seconds,
after147 exact diff and1094 decoded-gzip hardlink replacements. All1241 had
matching durable prepared/replaced receipts. The timeout remains failed.
Reclaim02 exited0 in19.106seconds, found no additional replacements necessary,
and finished the inventory. Every path remains; gzip representation changes have
old/new compressed hashes and exact decoded comparisons. Changed payload,
truncated gzip, missing canonical and symlink controls rejected. Frozen binaries
and failure archives were not compacted. Net allocated aggregate savings between
reclaim01's before and reclaim02's after snapshots were123678720bytes
(117.95MiB), including transformation-receipt overhead. Capacity saved101089280
bytes; conservatively assigned query/coordination saved22589440. Later experiment
receipts consume headroom again. Historical overages retain their original result.

Both new native commands used the existing frozen node/server/dump binaries,
actual HTTPS delivery, delayed ACK relay, real Spools and recovered raw custody.
Both exact source sets were fully ACKed with no normal-trial retries; original
query oracle graded six complete chains per pair and rejected omissions and
duplicate rows. Source, producer/recovery, requests, query pages, clocks, CPU,
memory/events, IO and sampled RSS/Spool sizes are retained.

| Fixture, serial / overlap | Backlog pair | Small pair |
| --- | ---: | ---: |
| Logs per arm |512 /512|25000 /25000 across20nodes|
| ACKed Batches |4 /4|3031 /3035|
| Source-to-ACK median |248.323 /276.267ms|86.445 /87.035ms|
| Post-Spool-to-ACK median |72.889 /115.669ms|74.983 /75.527ms|
| Actually prepared before prior ACK |0/3 /2/3|0/3011 /35/3015|
| Instrumented node CPU |4.145 /4.214s|54.055 /53.911s across20nodes|

Backlog execution `catalog-overlap-backlog-pair1-01` exited0 in48.110seconds,
parent cgroup peak218.9MiB. Small execution `catalog-overlap-small-pair1-01`
exited0 in60.434seconds, parent peak615.8MiB. No swap/OOM was observed. Service
maxima remained384MiB/server64MiB/node for backlog and3GiB/server256MiB/node
for small, beneath the20GiB parent. Small sampled server RSS was42.20/41.40MiB;
its service cgroup peaks57.46/56.46MiB are a different metric. Small offered
1000/3000/1000logs/s for three5-second phases, then quiet drain. This is an exact
fixed-demand screen, not sustained capacity. Timestamp-derived Spool/ACK
acceptance bins are retained separately in the new closeout, including metrics
Batches and before/after offer windows.

There is a useful causal distinction: backlog source-to-Spool median fell from
170.160 to64.131ms while source-to-ACK worsened. Earlier durable preparation
creates an earlier queued successor; it does not by itself accelerate the later
send. The small pair scarcely exercised preparation (1.16% of transitions).
Neither median ACK result supports promotion; one pair cannot establish a stable
regression or speedup. CPU measures the instrumented example. It calls
`Spool::inspect` every loop, decoding/validating retained frames before discarding
already-recorded sequences, then sleeps5ms. Actual small resource samples put
34.11/34.31 node CPU-seconds in the final approximately4.2 quiet seconds,
about63% of totals. Repeated inspection is a plausible major cause; attributing
all quiet CPU to it requires ablation. Synchronous ledger hex rendering is also
on the critical path. A separately registered observer-cost ablation is the
highest-information follow-up before another demand increase.

All60 small visibility sentinels were seen. Their maximum ACK-to-observed-visible
upper bounds were3.551/3.551seconds;60 round-robin polls spaced at least50ms plus
request time naturally produce a roughly3-second revisit interval. These bounds
do not measure actual server visibility latency or visibility of every unpolled
row. Backlog covered all512 records and had negative differences where query
visibility preceded the deliberately delayed ACK. No30-second freshness promise
or alpha qualification follows.

`catalog-coupled-continuation-cleanup-01` exited0 in20.519seconds, preserving and
byte-verifying reclaim01's retained launcher scratch before deletion. Its inner
control scratch was already removed by the failed helper. Both native pairs
removed their owned scratch after grading; no remote workload ran. C5 review
remains read-only: [full-state preservation and lifecycle findings](coupled-c5-preservation-review.md).
Its no-query control, four query grid cells and32MiB holdout remain unrun;
reclaimed duplicate objects do not establish the original20MiB preservation
bound. C4/Q4 remain closed at their historical outcomes. New independent
transformation readbacks, manual docs, syntax, launcher receipts and inventory
ran in `catalog-coupled-continuation-closeout-01`: exit1 in31.069seconds.
All1241 readbacks and changed-hash control succeeded; archived byte-exact
protocol copies had two broken relative links. Checker probes and hook tests
passed. [Closeout retry registration](coupled-continuation-closeout-retry-protocol.md)
preserves those source copies and adds exact context files, with no check waiver.

Cleanup02 exited1 in15.014seconds before any deletion: rearchiving the failed
audit's79.5MB Bun runtime exceeded prospective query evidence room. The exact
runtime was already preserved in a historical failure archive. A
[new reference-preservation registration](coupled-bun-reference-preservation-protocol.md)
preceded `catalog-coupled-bun-preserve-01`, exit0 in16.015seconds. It rejected
changed-source/missing-member controls, verified full archive and payload hashes,
compared all source/member bytes, persisted a complete source-manifest reference,
rechecked originals and inactivity, then removed the owned failure root. This
avoids duplicating the same runtime; unknown failure bytes are not excluded.
Cleanup03 preserves cleanup02's own retained scratch. Closeout02 independently
checks the canonical runtime archive/reference alongside transformations and
reruns documentation. Original failed receipts and manual-check receipts remain;
closeout02 subsequently exited0 in31.538seconds. All1241 transformation
readbacks, canonical archive/member/source-manifest verification, changed-hash
control, Python syntax and manual documentation/checker-probe/hook checks
succeeded. Its nine preceding continuation jobs were terminal, inactive, and
their owned scratch removed with preservation checks. Its own launcher receipt
also reports temporary removal and no retained failure root. No automatic Bun
hook or CI was enabled. The protocol-context copies retain exact original bytes.

Timestamp-derived small acceptance bins in the
[final readback report](data/catalog-coupled-continuation-closeout-02/result.json)
separate Spool commit from client ACK. The5–10s peak slices recorded202.0/202.6
durable Batches/s and202.0/206.6ACKed Batches/s serial/overlap; associated log
ACK rates were2999/3020rows/s. Overlap's boundary spillover accounts for the
apparent rate above3000, while all totals remain25000. These are finite slice
averages with metrics Batches included, not sustainable throughput. All raw
clocks remain available for a different retrospective slice.

Final helper inventory before its last receipt/log blocks: aggregate1795559424
allocated bytes below1894879232, capacity746004480 below872415232, and query
upper1049554944 below1073741824. Later admission output measured aggregate
1795579904/query1049575424. Capacity remains the same. The frontier now records
11054.909seconds consumed of14400 (3345.091remaining); no budget was reset.
These are current snapshots, not historical capacity recertification. C5's
full failed-state preservation and lifecycle prerequisites remain unresolved;
the original six service cells were not executed. This final receipt/status
entry follows the successful documentation run; no new implementation followed.
