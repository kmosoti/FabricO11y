# Development/small lab screen protocol

Registered for the first execution wave of the [campaign plan](dev-small-lab-plan.md).
Scope: M0, C1 and Q1, using private harness/fixture additions and unchanged
production custody, wire/storage semantics and independent Python query oracle.
No qualification, installed-profile or destructive host fault run is included.

## Fixed comparisons

Hypothesis C1: retained history and timed query activity explain a practically
meaningful part of remaining server CPU/RSS. Null: the nominated effect is below
15% or another factor explains it. One cell per condition only nominates further
investigation. Invalid clocks/offers or missing evidence are inconclusive; an
oracle discrepancy rejects the cell independently of performance.

Hypothesis Q1: live records remain exactly queryable before/during/after a real
first seal. Null: a transition loses/duplicates answers or exceeds the diagnostic
visibility bound. If no request overlaps a build, transition overlap is unobserved.

Seed 2703163393; 900-byte deterministic ASCII log bodies, half repetitive and
half entropy; real host metrics every 15 seconds; no trace-load claim. TLS loopback,
one sealer, 64 MiB journal threshold, 24-hour age retention. Live journal/Segment
byte settings are 1 GiB each to avoid incidental retention eviction with H256;
all actual live bytes must fit the 4 GiB trial ceiling. First two allowed CPUs
serve the server; next two serve producers/nodes/observers. CPU affinity is not
a quota or reservation. Every cell uses the same frozen release binaries.

Each cell: native setup, five-second settling, 60-second normal, 60-second 3x
burst, 60-second recovery and twenty-second drain, then graceful shutdown and
independent replay/query grading. M0 uses fresh development with timed queries.

| Cell | Nodes | Normal/burst/recovery logs/s | Seeded history | Timed queries |
| --- | ---: | --- | --- | --- |
| m0 | 1 | 10 / 30 / 10 | None | Scan |
| c1-1 | 20 | 1,000 / 3,000 / 1,000 | None | Off |
| c1-2 | 20 | 1,000 / 3,000 / 1,000 | H256 | Scan |
| c1-3 | 20 | 1,000 / 3,000 / 1,000 | None | Scan |
| c1-4 | 20 | 1,000 / 3,000 / 1,000 | H256 | Off |
| c1-5 | 20 | 100 / 300 / 100 | H256 | Scan |
| c1-6 | 1 | 10 / 30 / 10 | H256 | Scan |
| q1-fresh | 1 | 10 / 30 / 10 | None | Scan, jittered probes |
| q1-seeded | 1 | 10 / 30 / 10 | Near rotation | Scan, jittered probes |

H256 is 256 MiB encoded historical input (bounded final-Batch overshoot), fully
published as Segments before native timing, with an empty active journal. A
dedicated oldhistory Strand is committed through production Store APIs; native
nodes enroll with their own identities. Seeded input has deterministic old tags,
times, bodies, sequences and per-Batch byte hashes. This adds one historical
identity; it is disclosed as part of the history factor and not a live Spindle.
An exact independent decoded historical log ledger and Batch ledger must agree
after replay. Live cycles/ACKs are graded separately and cannot excuse lost history.
Near-rotation input leaves the active journal below 64 MiB by approximately
128 KiB; report exact before/after file lengths, commits, publications and times.
Seed credentials are disposable fixtures, never retained as evidence.

Match H256 logical Batch/source hashes across matching cells. Freeze the fixture
source and binary before its first timed cell; store input ledgers and actual
layout hashes. H0 and H256 start from fresh processes; use identical warm-up and
no machine-wide cache flushing. Prefix generation/publication and post-run oracle
costs are excluded from timed server metrics and included in total resource costs.
Do not pool c1-6 with c1-5 as a rate-only comparison: their node counts differ.

## Observation and guards

Reuse the existing observation consumer mix: one serial request per second,
rotating recent ten-second logs, absent text over all history and CPU metric
history, plus serial visibility probes. Record scheduled/started/completed counts
and lateness. Query-off has no timed HTTP/visibility consumers, but retains all
post-timing oracle checks. Its query/visibility metrics are null, never zero.
Q1 selects 36 targets at deterministic jittered offsets within five-second
buckets; preserve the schedule and actual tag writes. Poll at 250 ms and retain
unresolved targets, signed ACK-observed differences and response sample counts.

All ordinary pilot guards: exact source bodies/counts, exact seeded and live
Batch custody, contiguous sequences, no unexpected gaps/retries/exits; independent
quiescent query answers exact; changed/missing-answer controls rejected; server
RSS/HWM <=2 GiB, node <=64 MiB; collection-to-ingestion and ACK-observed p99 <=1 s;
producer lag p99 <=100 ms; realtime-minus-monotonic range <=5 ms and boottime-minus-
monotonic range <=5 ms. Missing phase populations invalidate derived comparisons.
Every selected tag must appear within 30 seconds of source write; no HTTP errors
or incomplete live answers. Query-off explicitly excludes only inapplicable timed
query/visibility guards. Require accepted live data to drain by shutdown; preserve
final sampled backlog separately from replay-custody evidence.

M0 first rejects the archived clock-step/missing-phase counterexamples and synthetic
clock/suspend/custody/metric defects. Same guards remain active throughout later
cells. A failed scientific guard stops dependent timed work; independent preparation
and evidence review may continue. Preserve failures before any correction/rerun.

Report phase CPU user/system/core equivalents, process RSS/HWM, cgroup peak/current/
anon/file/kernel/events/pressure/swap, Spool successful batches/logs/encoded bytes/s,
offers, send attempts/ACK/retries, IO, journal/Segment/Spool/scratch bytes, publication/
reclaim, queues, query p50/p99/max and visibility counts/distributions. There is no
complete Spool append-attempt denominator; acceptance percentage stays unavailable.
Allocator heap is not instrumented in this native binary. Integer clock deltas
precede unit conversion. Independent audit derives raw accounting without calling
the summary aggregation functions.

Balanced CPU averages normal/burst/recovery means; sampled peak server RSS compares
the same phases; retain every raw phase and query kind. Query-on/off CPU differences
also include visibility polling. Use c1-1..4 for history/query interaction, c1-2/5
for rate and c1-6 for development observation. Nomination needs >=15% effect with
stable workload and all guards, then a separately budgeted three-pair confirmation.
No performance winner, statistical significance or release readiness follows here.

## Execution, bounds and evidence

All commands use the resource launcher and coordinator:

```sh
python3 tools/resource_group.py -- python3 tools/bench/labs/dev_small/run_job.py \
  --id CELL --lab query --stage m0 --seconds 300 -- \
  python3 tools/bench/labs/dev_small/run.py --cell m0
```

The actual job selects stage/lab/cell appropriately. Build release binaries and
fixture with offline locked Cargo; controls and independent audits run in the
same containment. Every argument array, source snapshot/diff, protocol hash,
binary hash, environment and exit belongs in the receipt. Register this protocol
in its own commit before measurements. If that commit is unavailable, record the
condition and do not silently treat source hashes as a policy-compliant substitute.

Serialize workloads under 16 GiB memory high, 20 GiB maximum, no swap and the
30-minute outer deadline. Aggregate measured command runtime <=3,600 s, including
builds/validators; allocations: overhead 900, M0 300, C1 1,800 and Q1 600 seconds.
Individual workloads <=900 seconds subject to remaining allocation. A budget stop
is incomplete, not a shorter passing trial. Prioritize c1-1..4 before c1-5/6.
Owned scratch <=4 GiB, free-space reserve >=4 GiB. Data drive only. Retained compact
evidence <=50 MiB per lab; preserve equivalent gzip raw data, sources/hashes,
query verdicts and failures; if preservation cannot fit, stop before discarding it.
Clean owned successful fixtures only after grading/preservation; retained failures
need reviewed minimization before cleanup. Leave shared caches and unrelated data.

Evidence root: `data/dev-small-labs-run-02/`, with memory/query/recovery/coordinator
ownership. Harness/source/control changes after measurement require a separate
record and fresh affected cells; original outcomes remain unchanged. The coordinator
records agent tasks/estimated allowances, actual usage where available and uncertainty.
