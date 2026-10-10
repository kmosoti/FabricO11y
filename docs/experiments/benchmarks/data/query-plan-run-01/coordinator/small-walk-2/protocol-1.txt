# Development and small-deployment lab campaign

Status at registration: **written plan; experiments not run**. Execution evidence
now lives in [screen run 02](dev-small-labs-run-02.md): eight initial native cells
completed and C1-6 was deferred. The procedures below retain their registered
content; later campaign work remains unrun. This plan expands the adopted
[lab orchestration](../lab-orchestration.md). Proposed screens below require
their own frozen execution protocols before measurement. They do not revise
existing acceptance, oracle or qualification rules.

Review outcome: the initial screen has an explicit matched matrix; query
performance and retention correctness have separate cells; mixed-signal load
and deployment memory limits have their own follow-ups. Only M0, C1 and Q1
belong to the initial 60-minute machine-work allocation.

## Objective and starting evidence

Establish usable operating envelopes for general development and small
deployments: supported logs, host metrics and loopback OTLP/HTTP trace exports;
exact delivery and query answers; bounded
memory/disk; usable query availability; restart and outage recovery.

The [first lab wave](readiness-labs-run-01.md) measured a large reduction in
builder heap and passed nine selected recovery checks. Its Scan comparison was
invalidated by a clock discontinuity. Whole-system memory attribution, full
builder acceptance, sustained lifecycle and installed-profile evidence remain
incomplete. Historical results are not qualification of a new revision.

## Labs, staffing and campaign budget

| Lab | Accountable role | Assistant work | Deliverable |
| --- | --- | --- | --- |
| Capacity and lifecycle | Capacity PI | Source/ownership inventory, fixture preparation, resource accounting | Memory/queue attribution and measured workload envelope |
| Query and availability | Query PI | Query fixture preparation, clock review, independent answer accounting | Visibility distributions and exact transition/retention behavior |
| Recovery and operations | Recovery PI | Failure-stage inventory, reproducible fixtures, packaging/integration inspection | Custody/recovery evidence and deployment gaps |

The inline coordinator admits work and synthesizes cross-lab findings. Use
GPT-6.1-SOL medium for active PI work and GPT-6 Luna for bounded assistance.
Activate roles only when tasks require them; share artifact pointers rather
than entire conversations. Four available slots are a runtime backstop.

Initial preparation/screening budget: an estimated **30,000 delegated-agent
tokens**, with 24,000 for planned tasks and 6,000 reserved for unexpected
findings. This is a soft dispatch estimate, not measured billing or a runtime
cap; coordinator usage is reported separately where available. Initially allow
two assistant packets and one PI synthesis per lab; target concise reports of
at most 600 words plus file pointers. Reconcile available usage/estimates before
each new packet. Do not automatically refill the budget or start later waves.
Use a coordinator-owned dispatch ledger with assigned allowance, actual usage
when exposed, estimate otherwise, outcome and remaining allowance. When token
telemetry is unavailable, the finite task allowance is the scheduling bound.

Initial machine-work budget: at most **60 minutes cumulative measured workload
runtime** across the diagnostic screen, with at most 900 seconds per cell.
Stop/defer remaining cells on budget exhaustion. Builds and validators count
toward this budget. Acceptance, confirmation and long qualification campaigns
have separate budgets; this screen does not authorize them automatically.
Reserve 15 minutes for setup/validation/cleanup and admit timed cells only while
their estimate fits the remaining allocation. A 900-second per-cell cap is a
safety limit, not an expectation that every cell can consume it. Planned elapsed
allocations are M0: 5 minutes, C1: 30 minutes, Q1: 10 minutes, shared overhead:
15 minutes. Preserve completed observations and defer unfinished cells if a
stage exhausts its allocation; never shorten a measured trial after launch.

## Profiles and common measurements

Use the established [native observation profiles](dev-small-observation-protocol.md)
as starting shapes: development has one real Spindle and 10/30 logs/s;
small has 20 real Spindles and 1,000/3,000 logs/s across normal/burst/recovery.
These are offered test rates, not sustainable-capacity claims. Retain the
900-byte repetitive/entropy body mix, real 15-second host metrics, TLS and
64 MiB journal threshold. A change requires a separately declared screen.
Existing native profiles contain no spans: trace tests are additional declared
fixtures, not an inferred result from the log workload.

For new diagnostic cells freeze seed **2703163393**, 60/60/60-second normal,
burst and recovery windows, five-second settling and twenty-second drain unless
the cell explicitly specifies another schedule. Preserve registered seeds and
durations for acceptance/qualification. Fix node count, CPU placement, worker
count, binary, payload distribution, query schedule and cache preparation within
each matched comparison; record shared-host activity and perturbations.

The harness currently hardcodes node counts/rates and fresh state in
[`run_dev_small.py`](../../../tools/bench/run_dev_small.py). Seeded-state and
rate-only cells need private, validated harness extensions before execution.
Use a new campaign artifact root/queue: the previous coordinator is tied to
run-01 receipts and its budget. Do not append to old run records.

Record these dimensions for every applicable cell:

- Demand: offered events and encoded bytes/s, source lateness, and actual emissions.
- Acceptance: successful Spool batches/logs/bytes/s; attempts and errors only
  where instrumented; server admitted/committed/ACKed rates and retries.
- CPU: user/system seconds and core equivalents for server, individual/aggregate
  Spindles, producer, query clients and observers; phase and whole-run boundaries.
- Memory: allocator heap for builder-only tests, sampled RSS and HWM, cgroup
  current/peak, anon/file/kernel, pressure, events and swap. Keep them distinct.
- Storage: Spool, active/sealed journals, Segments/indexes, scratch, free bytes
  and inodes, logical/physical IO and pending/reclaimed journal counts.
- Queues: offered-minus-committed work, pending ACKs, unread source bytes,
  oldest pending age, backlog slope and time to drain.
- Availability: query latency by kind, errors, observation-to-query and
  ACK-observed-to-first-exact-answer distributions, unresolved targets and counts.
- Correctness: exact source/ACK/recovered Batch identities and bytes, independent
  query/rate oracle results, gaps/completeness and pagination behavior.

An ACK stdout observation and polling response are measurement boundaries,
not internal commit/visibility timestamps. Preserve signed differences and
missing measurements. Successful Spool batches/s is throughput; without a
complete attempt denominator it is not a percentage acceptance rate.

## Common decisions and execution order

| Stage | Experiments | Admission and result |
| --- | --- | --- |
| Initial screen | M0, then C1, then Q1 | Short diagnostic evidence inside the first budget; nominate causes and expose defects |
| Focused follow-up | R2a, Q2a/Q2b, then C4 | Mixed-signal fixtures, query confirmation, retention correctness and deployment limits; separately sized budget |
| Acceptance and faults | C2 and R1 | Existing acceptance plus scoped stage faults; preparation may proceed earlier |
| Sustained and installed | C3 and R2b | Candidate checks, supported host and longer-run scope/deadline established |

Each experiment has one accountable PI. Query supplies independent answer review;
Capacity supplies resource accounting; Recovery owns custody/fault interpretation.
Supporting roles do not duplicate experiment ownership or execute competing workloads.

In ordinary finite pilot cells retain the existing exact-delivery guards, server
RSS at most 2 GiB, each Spindle RSS at most 64 MiB, collection-to-ACK-observation
p99 at most 1 s, producer lateness p99 at most 100 ms, no unexpected exits/gaps
and the 5 ms clock-stability guard. Report collection-to-receive separately.
These pilot guards do not qualify a registered operating profile. Controlled
overload/fault cells instead require explicit refusal, retry, gap and completeness
behavior; expected injected errors are not judged as ordinary successful traffic.

Report each failed guard without pooling it away. Invalid clocks, missing samples,
unstable offers or an unavailable environment make performance undecidable;
they do not establish its null hypothesis. Correctness failure rejects a candidate
and stops dependent comparisons. Independent labs may continue preparation.

Percentiles retain counts, maxima, timeouts and unresolved targets. A few dozen
visibility probes are a sampled screen, not a general p99 guarantee. Confirmation
uses three fresh alternating pairs, every pair reported, with no screen samples
reused. Disagreeing pairs remain inconclusive. These finite repeats establish
neither statistical significance nor a universal operating bound.

## Wave 0: establish trustworthy measurement

**M0 — clock and observer preflight.** Query lab owns this dependency.

Hypothesis: clock/phase alignment and sampling can distinguish a valid trial
from interruption without fabricating metrics. Null: a discontinuity, missing
phase or observer cost makes the comparison undecidable.

Prepare a short native trial, record realtime/monotonic and available boottime,
compare source timing to phase membership, and replay the preserved
clock-discontinuity/missing-phase fixtures. Retain the existing 5 ms stability
guard; use null plus reason for absent phase metrics. Do not revise a frozen
checker to obtain a pass. Measure observer CPU and late requests separately.
Apply clock/phase guards throughout every later cell; a clean preflight does not
validate future clock behavior. Separate elapsed-time scheduling from Unix
telemetry timestamps and retain both. Suspend-like changes in boottime versus
monotonic invalidate affected timing even if Unix offset is otherwise stable.
New instrumentation must reject archived counterexamples before use. If observer
cost prevents stable offers, prepare a separately specified observer-on/off
calibration. No performance comparison proceeds after a failed preflight.

## Wave 1: diagnostic experiments

**C1 — memory ownership: retained history versus ingestion and query work.**

Aim: locate the remaining memory cost before choosing an optimization.
Hypothesis: increasing retained history amplifies query CPU/RSS at fixed ingress.
Null: matched-history query costs remain stable or another component dominates.

Use these six cells, each with fresh owned state. H256 is a deterministic
256 MiB encoded historical prefix fully published as Segments before timing;
H0 has no historical prefix. Record actual Segment bytes, rows, groups and age.
All cells start with an empty active journal. Explicit seeded pending-journal
comparisons belong to Q1; live seals are recorded in every C1 cell. This fixes
the initial representation of history instead of leaving it as a confounder.

| Cell/order | Real Spindles | Normal/burst/recovery logs/s | History | Timed queries |
| --- | ---: | --- | --- | --- |
| C1-1 | 20 | 1,000 / 3,000 / 1,000 | H0 | Off |
| C1-2 | 20 | 1,000 / 3,000 / 1,000 | H256 | Scan |
| C1-3 | 20 | 1,000 / 3,000 / 1,000 | H0 | Scan |
| C1-4 | 20 | 1,000 / 3,000 / 1,000 | H256 | Off |
| C1-5 | 20 | 100 / 300 / 100 | H256 | Scan |
| C1-6 | 1 | 10 / 30 / 10 | H256 | Scan |

C1-1 through C1-4 examine history/query interaction at the small offered load.
C1-2 versus C1-5 changes rate with the same 20 nodes, history and query mix.
C1-6 checks development with old history; its different node count makes it a
profile observation, not a rate-only comparison. Prioritize the first four
cells if the stage budget cannot fit all six. This fixed interleaved order is
a screen; fresh alternating confirmation is required for reproducibility.

Use the existing consumer mix: recent ten-second logs, absent-text full retained
history and CPU metric history. The history query must include seeded records.
Freeze prefix timestamps, attributes, node identities and query selectivity
before generation. Require identical prefix hashes for matched H256 states.
Query-off disables timed HTTP/visibility consumers but retains sampling and
post-timing independent answer grading; its visibility is unmeasured.

History grows during live trials: record retained rows/bytes and publication/
reclaim at each phase boundary. Avoid incidental retention eviction using
declared test ceilings within the disk budget; redesign before timing if planned
state cannot fit. Q2b tests retention. Standardize preparation/warm-up, record
cache conditions and avoid machine-wide cache flushing. Account for prefix
generation, startup and post-run grading separately from timed application work.

Decision: identify a mechanism only if repeatable differences align with source
ownership and measurements. A candidate improvement must later survive three
fresh alternating baseline/candidate pairs, exactness and resource/backlog guards.
One screen pair nominates work; it does not establish causality or a winner.
Report the history/query difference of differences alongside raw cells without
treating it as causal proof. A proposed 15% CPU or sampled RSS effect nominates
fresh investigation; it is not a product gate. Any correctness or resource-bound
failure warrants investigation regardless of effect size.

**Q1 — first seal and pending-journal availability.**

Aim: show whether low-rate users can query data before final materialization and
whether the first seal introduces contention or visibility holes.
Hypothesis: acknowledged data remains exactly queryable through storage transitions.
Null: the transition causes missing/duplicate answers or unacceptable visibility.

Use two declared conditions: fresh development state, and an independently seeded
near-rotation journal receiving development traffic. Keep the 64 MiB threshold.
The seeded condition exercises a transition cheaply; it does not substitute for
the fresh default lifecycle. If fresh state does not seal within the screen,
record that limitation and schedule its natural lifecycle separately.
Query tagged records before/during/after sealing and follow complete snapshot
page chains after quiescence. Keep oldest/newest tags and unresolved probes.
Seed near-rotation state through verified native custody or a separately validated
fixture path, preserving identity/sequence continuity. Use a frozen deterministic
jitter schedule so visibility probes are not all phase-locked to collection.
Verify actual build overlap; if the build finishes between probes, report overlap
unobserved. A private barrier fixture may then test transition exactness, but
its artificial delay supplies no performance/freshness result.

Decision: zero oracle discrepancies is mandatory. Report visibility distributions
and seal-overlap effects. The registered 5-second observation-to-query p99 applies
only when its registered workload is actually run; a 30-second diagnostic
observation horizon does not replace it. Build speed alone cannot decide success.
For this diagnostic retain the earlier screen's maximum 30 seconds from source
write to an exact tagged answer, no unresolved targets or HTTP failures. Report
polling censoring and sample counts; this remains separate from the registered
freshness gate.

## Wave 1 follow-up: query choice, signals and deployment bounds

These jobs are outside the initial 60-minute allocation and need their own
prepared protocols and finite budgets.

**Q2 — query-plan and retention correctness/cost.**

Aim: identify affordable queries over accumulating history and verify honest
answers when retention removes data.
Hypothesis: Walk reduces work on declared shapes while preserving exact answers;
null: it offers no material benefit, regresses another shape or changes answers.

Q2a compares Scan/Walk on identical recovered input, fixed query shapes, offered
cadence and CPU placement, with eviction disabled. C1's query-off observations
are context, not confirmation. Record scheduled/started/completed requests, late
starts and concurrency: a slower closed-loop client must not appear cheaper
merely because it issues fewer queries. Report CPU per completed query as well
as total server CPU and offered demand.

Q2b is a separate correctness/lifecycle fixture: byte-driven and age-driven
retention, concurrent sealing, quiet sources, collection gaps, snapshot pages,
missing/corrupt optional filters and missing/corrupt raw Segments. Use small
declared retention limits and controlled timestamps to reach boundaries within
budget; this does not validate 24-hour/20-GiB defaults at scale. Grade retained
rows and expected Gone/incomplete responses against an independent retained
ledger. Legitimate eviction is not missing delivery. Snapshot expiry, raw
Segment corruption and index corruption have distinct expected outcomes.
Timestamp age alone does not prove liveness.

Include recent logs, absent/present text, metric history/rates and supported span
queries. Freeze trace payload/selectivity in R2a fixtures before span comparisons.
Separate first-query index construction, warm queries, post-seal invalidation
and post-restart rebuild: Walk's derived blocks/caches can trade CPU for memory.

Decision: independent exactness and completeness guards precede speed claims.
For Q2a propose at least 15% lower balanced CPU or balanced median query latency,
at most 10% higher sampled RSS, no worse final/peak backlog, retries or ACK p99,
and all pilot guards met. Freeze one primary metric, phase/shape weights and
per-shape guard before timing; do not select whichever aggregate wins afterwards.
Compare matched request counts/starts or mark CPU savings confounded by consumer
throughput. Confirm with fresh alternating pairs. Missing/corrupt optional indexes
must trigger exact fallback. Q2b outcomes do not enter Q2a speed aggregates.

**C4 — deployment resource budgets and overload recovery.**

Aim: establish usefulness under explicit application limits below the lab ceiling.
Hypothesis: a profile keeps up under its candidate limits and recovers from bounded
overload; null: it violates a pilot guard, cannot drain or exceeds its budget.

Initial proposed memory-max targets: development server 512 MiB, node 128 MiB;
small server 3,072 MiB and 256 MiB per node, matching installed service maxima.
Development targets are hypotheses, not recommended defaults. Apply the actual
installed aggregate slice limits on each relevant small-deployment host; do not
pack 20 modeled edge hosts into a one-node slice. Generators, observers and graders
have separate accounting inside the 20 GiB outer envelope. Verify nested service
limits and swap enforcement before launch: RSS/address-space limits cannot stand
in for cgroups. Freeze CPU quota/placement and task limits in the protocol.

Run ordinary load, a finite burst and drain, then separately intentional journal/
Spool pressure with an independent expected-gap ledger. Require exact custody of
accepted data, explicit backpressure/refusal and recovery when pressure clears.
Normal-load OOM or missing ACKed data fails the profile. Report achieved versus
offered throughput, queue slopes, pressure, latency tails and all live bytes.
A failed target stays failed; testing more memory creates a new candidate.
Include R2a's mixed-signal fixture before publishing a three-signal envelope.

## Wave 2: acceptance and recovery

**C2 — bounded-builder acceptance.**

Aim: establish that the memory benefit survives input scaling and all registered
shapes without weakening query pruning or determinism.
Hypothesis: existing BS criteria hold; null: any criterion fails.

Execute the [bounded-sealer acceptance](../../milestones/bounded-sealer.md):
registered shapes/sizes, three runs, exact rows/order, applicable byte equality,
80 MiB heap ceiling, no more than 10% heap growth at 256 versus 64 MiB,
pruning equivalence, deterministic files and leftovers accounting.
The earlier physical filter-cardinality mismatch needs a separately committed
checker/protocol interpretation before new acceptance; preserve its original
failure. Do not label reconstructed diagnostic fixtures full acceptance.

**R1 — stage-specific disk faults and process crashes.**

Aim: test the new storage boundaries that selected recovery checks did not cover.
Hypothesis: supported stage failures retain custody and restart/retry converges
to exact answers; null: any stage loses acknowledged data, duplicates logical
results or leaks unsafe/unbounded state.

Prepare deterministic read/write/ENOSPC fixtures at spill, merge, table/filter/raw
output, manifest/sync/publication and checkpoint/reclaim boundaries. Add actual
owned-child kills at publication cut points and independently grade restart
answers. Use an owned limited filesystem or syscall injection; never fill the
shared data drive. Preserve traces before cleanup. Execution requires explicit
fault scope and successful containment preflight. Process kill plus successful
sync assumptions does not establish physical power-loss durability.

Decision: zero custody/query mismatches, correct failure visibility, recoverable
state and verified cleanup. Representative negative controls must be rejected.
Counterexamples get deterministic regression fixtures rather than silent reruns.

## Wave 3: sustained operation and installed deployment

**C3 — unchanged soak and outage recovery.**

Aim: show that finite memory savings become sustained bounded operation.
Hypothesis: memory/backlog settle and buffered work drains under the registered
load; null: sustained growth or any original gate fails.

Run the unchanged 5,460-second [soak](soak-protocol.md) and registered outage
protocol on the candidate revision. Preserve the memory-growth, ACK-window,
correctness and drain gates. Qualification requires its target profile.
These exceed the current 30-minute launcher deadline: separately establish
authorized run scope/deadline before admission. Do not shorten them and claim
equivalent evidence. Deliberate overload measures bounds/correctness, not steady
latency acceptance.

Also budget a separately registered fresh development lifecycle through at least
two natural journal rotations. Estimate its duration from observed encoded byte
rate before admission. A 100-identity soak does not exercise the same duty cycle
as one low-rate Spindle. Report how much real time and how many seal/reclaim
cycles were observed. The frozen soak does not reach default retention limits;
use Q2b/C4 for bounded retention behavior and disclose the untested duration/scale.

**R2 — installed profile and generic signal integration.**

Aim: make Fabric usable as a supported service rather than only a benchmark.
Hypothesis: packaged services enforce their limits and supported application
signals preserve semantics through disconnect/restart; null: a required behavior
fails. An unavailable environment is an undecided result, not evidence of failure
of that hypothesis.

**R2a runs in the focused follow-up wave**, before C4 and Q2 span comparisons.
Use native configured newline logs, real host metrics and a pinned representative
SDK's loopback OTLP/HTTP protobuf traces. Start with trace-only correctness, then
compare log/metric-only versus concurrent logs/metrics/traces at declared rates.
Freeze encoded byte rates, span size/cardinality, attributes and SDK batching/
retry settings before timing. The purpose is to expose concurrent buffer/CPU
costs and backpressure, not to infer that equal counts of logs and spans cost the
same. Measure exporter-to-local-Spool response separately from server ACK and
query visibility. Compare fields, units, timestamps, IDs and parent relationships
through restart and delivery; grade projected query semantics and exact retained
payload fidelity separately. Known host metric values can vary: use an independent
decoded payload ledger plus deterministic metric reset/rate fixtures.

**R2b runs on a disposable supported Debian-family/systemd host** under the
existing installation protocol. Exercise actual UID/permissions/cgroups,
authorized/denied log sources, TLS and revocable credentials, rejection of the
wrong credential role, control application, graceful shutdown/restart and
data-preserving uninstall. Include a documented fresh install/start/query/stop
path and a supported previous-state compatibility check, or explicitly declare
fresh-install-only scope. Preserve operator data while testing restarts/reinstall.
No general OTLP receiver or unsupported deployment claim follows from these cells.

Installed memory/task limits come from the product contract, not the local
20 GiB experiment cap. Report process RSS separately from cgroup charges.
Run the frozen installation acceptance only under its authorized scope; missing
supported host/containment is environment unavailable, not success.

## Deciding whether a profile is ready for use

The final report gives one outcome per profile and signal set: **supported for
the tested envelope**, **blocked by a measured defect**, or **inconclusive/missing
evidence**. It names the source revision, host, enforced memory/CPU/task limits,
node count, offered/achieved rates, query mix, retained bytes, duration and actual
seal/retention cycles. Log/metric results never imply trace support under load.

For controlled dev/small use, require all of the following within that declared
envelope:

- Exact custody and independent query answers, including retention completeness,
  successful replay/restart and supported injected-failure recovery.
- Ordinary-load ACK and source-timing pilot guards above, no growing backlog,
  and zero pending/unread source work after the declared twenty-second finite
  drain. Outage tests use their own frozen drain windows. Any alternative drain
  bound must be registered before its run.
- Proposed use criteria, frozen before follow-up measurements: sampled
  observation-to-query p99 at most 5 seconds, per-kind query p99 at most 2 seconds,
  no unresolved probes or HTTP errors, and every probe inside the 30-second
  diagnostic horizon. Report counts/censoring; these finite profile observations
  do not establish general tail percentiles. Passing only the looser Q1 diagnostic
  guard is insufficient for the use decision.
- Verified application cgroups in C4, no normal-load OOM, bounded live disk
  including spill/cleanup, and the required sustained lifecycle evidence.
- Successful supported installation and generic integration for the advertised
  signal set, with a repeatable operator procedure and recorded limitations.

A profile that meets these scoped criteria may support a controlled trial. The
registered observation-to-query p99 of 5 seconds, query p99 of 2 seconds,
target-host workloads, other qualification gates and their stated conditions
still decide the full release promises. Near-instant builds and
Walk being faster are optimization goals; neither is a prerequisite if the
chosen implementation already meets the applicable envelope and contracts.

Close the candidate with required fast/extended checks on the final revision,
retained counterexamples, actual command exits, clean owned scratch and the
evidence-to-claim map. A default or code change after measurement triggers the
affected checks and fresh comparisons; historical green results are not inherited.

## Storage, execution and completion

All builds/workloads/validators use `python3 tools/resource_group.py -- COMMAND`.
Initially run command trees sequentially: 16 GiB memory high, 20 GiB maximum,
zero swap, 30-minute outer deadline. Initial diagnostic cells have a 900-second limit,
4 GiB owned scratch ceiling, 4 GiB free-space reserve and at most 50 MiB retained
evidence per lab. Monitoring is sampled; report possible transient overshoot.
Caches/scratch stay on `/run/media/kmosoti/data/FabricO11y`; no fallback storage.
Any larger/longer campaign needs its own scoped budget and admission.

Exact command arrays, source/binary/config hashes, seed, host/controllers,
measurement intervals, raw populations, oracle controls, exit and cleanup receipts
belong in each run record. Exclude credentials. Preserve original outcomes and
compact failures before removing owned temporary data. Remote edge forwarding
is an optional follow-up with limits appropriate to both hosts, not qualification.

After each wave, the coordinator publishes supported findings, null results,
unresolved uncertainty, cross-lab interactions and the next decision. Reserve
exploration for discriminating tests of observed patterns, such as history read
amplification, query/build contention or reclaim scheduling; do not nominate
mechanisms solely because they sound promising.

Completion yields a revision-specific operating-envelope report, deployment
guidance, unresolved blockers and links to actual checks. A broadly distributed
release still requires the [qualification](../../QUALIFICATION.md) evidence and
release process; agent reviews and this plan establish no release readiness.
