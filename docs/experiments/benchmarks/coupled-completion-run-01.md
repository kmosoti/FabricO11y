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
