# C5 workers and bounded run-size completion screen

Status: registered for controls and measured storage admission; no C5 service cell executed. This document freezes
prepared behavior but does not waive O6/O7 or authorize more storage/time. The
current executable admission rejects native execution under the20MiB evidence
assignment if measured source-input preservation alone exceeds that cap; a smaller
subset would still require a full failed-state bound. Completion
may therefore be **not admitted, concrete storage projection**, rather than a
fabricated finite-performance result. Historical C2 remains failed.

H1: changing bounded sort-run size and sealer workers changes the finite feasible
resource/latency point while preserving custody and exact query behavior. H0:
contention erases benefits, demand/measurement is invalid, or a hard bound fails.
No qualification, sustainable-throughput promise, fitted universal optimum or
three-pair speed claim is made by this discovery screen.

## Fixed source and fixture

Freeze separate plain server binaries for compile-time
`FABRIC_RUN_MIB_EXPERIMENT=8/16/32`; unset borrowed/spill/catalog selectors and
omit allocation/phase instrumentation. Server `--describe` acknowledges its actual
compiled cap and driver compares it with the cell before release. Freeze identical
node/server_dump/pending-seed helpers across variants. Capture exact build argv,
environment, toolchain, source/lock hashes and executable SHAs; driver hashes all
server/frame Rust sources, relevant Python observer/consumer/cgroup helpers and
workspace manifests. No production sealer gate: the experimental server waits
before its first sealer pass while HTTP/intake work normally.

`coupled_pending_seed` commits exactly128MiB encoded Batch bytes with900-byte
bodies,512 logs/Batch except the final size-fitting Batch, deterministic seed
2703163393, oldhistory namespace, and a declared final padding attribute. Native
replay verifies raw Batch SHAs, identities and complete prefix. Rotate every
remaining active prefix without building Segments. Before timed release require
>=2 actual pending journal files and zero published Segments; preserve their
names/sizes. Poll actual `.building-*` directories every5ms and require observed
concurrency >=workers. This is a sampled lower bound, not proof of continuous
worker utilization or simultaneous CPU execution.

Offer20 real nodes aggregate1000/3000/1000logs/s for60/60/60s. Every live body is
900bytes with tag `node:tick:index`, alternating repeated/entropy padding. Host
metrics cadence15s, journal files64MiB, no restart/outage/SDK source. Seed receive
clock uses actual commits; its log observation timestamps remain frozen. Seed
and live namespaces are separately verified against recovered raw Batch hashes,
source-body hashes and spool cycle/ACK identities. Offers count300000 live logs;
source schedule lateness/rate accounting and phase clocks stay visible. Throttled
source delivery is not reinterpreted as server capacity.

Query-on schedules200 serial fixed1Hz offered requests at offsets0..199s,
including20s drain: shapes cycle recent logs, absent text and CPU metrics.
Visibility uses the existing36 selected targets and3s observation delay. Require
exact cadence count/order/scheduled timestamps, no unexpected query errors and
unchanged measurement validity. Serial scheduling preserves lateness when calls
block; it does not create fixed completed demand. Phase CPU/latency refers to
normal/burst/recovery [0,60),[60,120),[120,180); drain requests remain recorded
separately. Query-off has no timed/visibility clients but performs identical
quiescent full-chain grading, including seed sentinels, with the unchanged
independent Python query oracle and missing/changed answer controls. It does not
grade a broad drain of every seed row; all seed bodies/raw custody are separately
checked. Original replay SHA is retained in oracle provenance; archives do not
claim to recreate arbitrary original Batch payloads from their hashes.

## Cells and decisions

First requested cell is `w1-r16-off`. Only after its admission, controls, semantics,
measurement validity, exact archival reconstruction and cleanup succeed may root
admit query discovery cells `w1-r16-on`, `w1-r8-on`, `w2-r8-on`, `w2-r16-on`.
One selected worker count at32MiB is an independent held-out prediction cell;
do not silently fit all three sizes first. All cells remain conditional on the
current time/evidence forecast; no partial or invalid cell is a zero-valued win.

One observer/control failure stops timing claims. Preserve exactness/custody,
no-gap/no-unexpected-retry, clean exits, clock/rate, original ACK latency and
RSS gates; cgroup bounds do not replace these stricter existing gates. Primary
output is a finite feasible/Pareto table of server CPU per accepted byte, sampled
RSS/whole-job peak, actual IO, journal/spool backlog/drain, ACK/query latency and
refusal/retry. Separate200-request cadence from completed-query counts and sampled
concurrent builders. No nomination follows without a new confirmation protocol.

## Storage and execution admission

Outer delegated20GiB/no swap; actual server child3GiB/2CPU, each of20 nodes
256MiB/1CPU, explicit disjoint2server/2client affinity. Build and temporary fixture
paths remain on mounted data drive. Native checks block-aware unique-inode scratch
<=4GiB and free>=4GiB while active; coordinator retains its8GiB/max/free envelope.
Persistent repository evidence has a **20MiB total C5 assignment including failure**.
Aggregate ceiling1,894,879,232bytes; capacity prior847,081,472 and prospective
ceiling872,415,232bytes. Driver admission uses max(unique lengths, allocated
blocks) and reserves both next successful archive and failed-state preservation.

`coupled_c5_archive.preserve` replaces reproducible body hashes with short markers
while retaining exact JSON line framing, ordering, timestamps and actual Batch
SHAs. Before deleting originals it reconstructs every source/seed/clock byte,
checks original SHA, cross-file tag partition and gzip readbacks, and persists
receipts. Wrong-seed/hash, missing/duplicate/order/timestamp controls are required.
This format reduces successful evidence; it does not preserve full failed journal
or Spool custody. Successful archive projection remains4MiB/cell. Before admission,
`--projection` generates the EXACT300000 offered lines into20 per-node gzip files
on owned data-drive scratch: rates1000/3000/1000,1800 ticks,15000 lines/node,
900-byte bodies plus newline. It measures unique file lengths and allocated blocks,
checks sample regenerated hashes and every decoded-file SHA, records counts and
source hashes, then removes all owned temporary files. No server, seed, journal,
Spool, build or native service workload runs in this projection. Maximum90s,
internal generation deadline80s, temporary1GiB, free16GiB.

These actual per-node gzip bytes are the source-input cost of the declared
full-failure preservation format. They exclude seed/state/receipt costs, and are
not a universal lower bound on other codecs or a guarantee about tar compression.
If this subset already exceeds20MiB, fail admission before any native child;
otherwise further full-failure bounds remain necessary. Do not leave failed
scratch as uncounted permanent evidence or invent a raw-Batch regeneration codec
to bypass this admission. A separately regenerated input exclusion requires a new preservation protocol;
a larger allocation requires explicit scope. Existing authorization covers
reversible preparation within the unchanged bounds. The original
64MiB guess is removed; `projection.json` is the observed basis, not invented data.

Root may register/run helper controls (<=60s) and the admission-only attempt
(<=30s), preserving its rejection receipt. Admission-only writes an explicit `admitted` boolean and exits0 if the assessment
completed; command success does not mean scientific/resource admission. It starts
no native child and requires no shell binary variable:

```sh
python3 -B tools/resource_group.py --delegate -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-coupled-c5-controls-01 --lab memory --stage capacity --seconds 60 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 4 --capacity-reserve-mib 1 -- python3 -B tools/bench/labs/catalog/coupled_c5.py --controls
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-coupled-c5-projection-01 --lab memory --stage capacity --seconds 90 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 4 --capacity-reserve-mib 1 -- python3 -B tools/bench/labs/catalog/coupled_c5.py --projection
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-coupled-c5-admission-01 --lab memory --stage capacity --seconds 30 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 4 --capacity-reserve-mib 1 -- python3 -B tools/bench/labs/catalog/coupled_c5.py --admission-only
```

No actual performance attempt is admitted by these commands. If preservation is
prospectively resolved, freeze a separate amendment before increasing the cell
allowance to300s. Six conditional cells<=1800s plus preparation/controls/audit
must fit the remaining4400s completion and shared frontier budgets. Root serializes
all builds, controls, admission and cleanup; never fall back to uncapped execution.
