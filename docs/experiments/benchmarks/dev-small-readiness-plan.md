# Development/small readiness: coordinated experiment plan

Owner requested a multi-perspective investigation, explicit hypotheses/nulls,
isolated agent labs, a controlled experiment queue and inline review of results.
This plan precedes delegation and new measurements. Three subagents use
GPT-6.1-SOL at medium effort; with the inline coordinator the team has four
agents total. They do not spawn further agents.

## Objective and fixed boundaries

Determine which memory, latency, custody and deployment claims the current
candidate can support, and which mechanism deserves the next optimization.
The [product contract](../../PRODUCT-CONTRACT.md), independent delivery/query
oracles, durable ACK ordering, exact Batch bytes, query completeness and
snapshot semantics stay fixed. A speed result cannot compensate for a custody
or answer mismatch. This campaign does not itself authorize a release/tag or
replace the [qualification ledger](../../QUALIFICATION.md).

Starting evidence is the [dev/small observation](dev-small-observation-run-01.md),
[sealer speed study](sealer-speed-run-01.md) and
[bounded-sealer acceptance](../../milestones/bounded-sealer.md). The initial
launcher probe exited 0 (`fabric-work-889c67a85ddb4b248ba614cf840b0f35`), confirming
20 GiB memory max and zero swap. Prior read-only data-drive and Git restrictions
are retained as environment observations; every actual workload must preflight
again rather than assuming access persists.

## Perspectives and discriminating questions

| Perspective | Current evidence / uncertainty | Discriminating experiment |
| --- | --- | --- |
| Allocation ownership | Builder heap fell sharply, but whole-server RSS still reached 485 MiB | Vary row expansion, disorder and file size; separate live heap, retained RSS and cgroup file cache |
| Queueing and conservation | About 20 Spool batches/s carried both ordinary and 3x burst loads | Account logs, bytes, attempts, ACKs and all backlog separately; test whether pressure moves upstream |
| Query algorithm and growing history | Server CPU rose in recovery; absent-text queries were slowest | Same offered data with no timed queries, Scan and Walk; retain the memory cost of caches |
| Lifecycle and critical path | Five small-profile seals succeeded; development never sealed | Repeated build/drain/reclaim boundaries, then unchanged long soak and default development first seal |
| Durability and failure isolation | Existing fault evidence predates the new spill path | Interrupt owned builds and exercise read/write failures; verify retained custody and clean retry |
| Deployment and operator experience | Running install was inconclusive on a legacy hierarchy | Inspect a disposable supported cgroup-v2 host, effective limits, identity, restart and data-preserving uninstall |
| Measurement and inference | Fixed-phase visibility sampling and a repaired accounting defect | Independent raw-event recomputation, negative controls, randomized poll phase and explicit observation bounds |

Competing mechanisms remain open: query scans, allocator retention, decoded
row expansion, file cache, scratch IO or scheduler contention can dominate in
different regions. More threads, smaller journals and delayed materialization
are candidate trades, not assumed improvements. Queryable committed journals
already separate visibility from final Segment construction. The 30-second
headroom idea remains an experiment target, not an amended freshness contract.

## Labs, hypotheses and nulls

| Lab / queue | H1 and fixed comparison | H0 / rejection | Evidence and initial decision |
| --- | --- | --- | --- |
| M: memory/shape | The bounded builder preserves exact output with the existing 80 MiB/10% acceptance bounds across the registered shapes | Any mismatch, leftover or bound violation; a missing shape is unrun, not passed | Map and execute existing applicable checks first; implement missing finite fixtures in a private lab. Full acceptance uses its existing seed/shapes/repeats and remains distinct from screening |
| Q: ingestion/query | At equal small-profile input, Walk lowers median query latency or server CPU by at least 15% relative to Scan, without more than 10% RSS growth or worse custody/backlog | Improvement below the practical threshold, resource trade-off above guard, any fidelity failure or unstable offers | First three sequential cells: query-off, Scan, Walk; one trial each nominates mechanisms only. Fresh paired repetitions are required before an optimization claim |
| R: recovery/deployment | The current candidate retains all ACKed data and recovers cleanly through the existing applicable fault/restart tests; the installed-profile gate is executable in the available environment | Missing/changed/duplicate data, unsafe cleanup, hidden incomplete answer, failed enforcement; unavailable installation is not a product pass | Run scoped existing failure tests; map coverage to new spill/publication stages. Inspect installation prerequisites read-only before any privileged/disruptive action |
| L: lifecycle follow-up | Repeated seals stabilize resources and the unchanged registered soak passes all ten gates | Growth/backlog/latency or correctness gate fails | Queue after M/R preflight and frozen candidate; a short smoke is never substituted for the 5,460 s soak |

The Q screen holds seed 2703163393, 20 native Spindles, 900-byte bodies,
60/60/60 s ordinary/burst/recovery, 1,000/3,000/1,000 logs/s, one seal worker,
64 MiB journal files, two server CPUs and two other client CPUs. Query mix and
resource observations follow the existing observation protocol. Query-off has
no timed HTTP queries; post-run exact recovery is still mandatory. A new lab
protocol must specify any visibility jitter before execution. Do not compare
an old trial on another host as the causal baseline.

Each lab writes its concrete protocol, commands, limits, expected outputs and
negative controls before queue admission. Unsupported sensors remain null with
a reason. Practical H1/H0 labels are screening decisions, not statistical
significance claims. Confirmation uses three fresh alternating pairs, reports
each pair and medians, and does not reuse screening trials as confirmation.
If those pairs disagree, retain the result as inconclusive rather than claiming
a universal winner. Root may reject expensive confirmation if the screen
reveals a correctness failure or no plausible useful effect.

## Isolation and execution ownership

- Each lab owns only `tools/bench/labs/readiness/<lab>/` and
  `docs/experiments/benchmarks/data/readiness-labs-run-01/<lab>/`. A lab may
  propose an additional named source file, but the coordinator assigns its
  ownership before edits. Existing dirty work is preserved.
- Runtime scratch belongs under the mounted data drive's
  `FabricO11y/scratch/`, within a launcher-owned directory and a lab/job subdir.
  Freeze source/diff, build settings, binary hashes and fixture hashes for each
  run. Shared build-cache mutation belongs to the coordinator's queue.
- Agents inspect, prepare and review in parallel. **Only the coordinator starts
  project builds, tests, benchmarks or validators**, one command tree at a time
  through `python3 tools/resource_group.py -- ...`. Subagents submit jobs and
  wait for receipts. Thus three labs never each consume a separate 20 GiB cap.
- Every initial-wave invocation retains memory high 16 GiB, max 20 GiB, swap 0
  and the current 30-minute deadline. Default per-job scratch ceiling is 4 GiB,
  free-space reserve 4 GiB, retained compact evidence at most 50 MiB per lab.
  Registered acceptance with a tighter bound keeps that tighter bound.
- Queue at most twelve first-wave jobs, with total scheduled runtime budget
  60 minutes (excluding agent preparation). Stop a job on incorrect output,
  unsafe resource state, unexpected child exit or its time/disk limit. Preserve
  the failure and record which dependent jobs it blocks.
- Full soak/outage/default-development-first-seal and privileged installation
  are separate queued stages: the present 30-minute launcher cannot execute
  the unchanged long protocols. Prepare the concrete invocation and required
  containment change; do not silently shorten the protocol or extend limits.
- No remote workloads in this wave. A later edge-forwarding trial needs actual
  remote host inspection, host-sized cgroup limits and owned cleanup; preserve
  the existing webserver and frozen network infrastructure.

## Queue and result contract

Jobs move through `draft`, `ready`, `running`, `passed`, `failed`, `inconclusive`,
`blocked_environment` or `deferred`. Each lab supplies `jobs.json` containing
job ID, hypothesis, prerequisite IDs, argument-array command, runtime/disk
bounds, expected artifacts, exact decision rule and cleanup ownership. The
coordinator records start/end, exit, cgroup receipt, source/binary hashes and
observations in the campaign queue. A missing artifact cannot yield `passed`.

Every result records offered/committed/ACKed logs, batches and bytes; CPU
user/system and core-equivalents; builder heap where instrumented; RSS and
cgroup anon/file/kernel/peak/events; IO and scratch bytes; source/Spool/sealed
backlogs; query latency and observation-to-visible clocks with sampling limits;
fidelity verdicts; errors, negatives, censoring and cleanup. An unavailable
dimension is explicit, never synthesized from an unrelated counter.

The coordinator checks the raw evidence, command exits, negative controls,
measurement boundaries and cleanup before accepting a lab conclusion. Agent
agreement is not an oracle. Conflicting evidence becomes a discriminating
follow-up job. Findings are classified as observed, inferred, proposed or
unresolved, and mapped back to the release gates. No runtime optimization is
merged into the measured baseline midway through a comparison.

## Consolidation

Look specifically for interactions: caches trading CPU for retained RAM;
bursts changing batch size rather than batch count; memory saved by spilling
moving pressure to IO/ACK latency; faster sealing reducing query-tail cost;
and reclaim allowing data-drive capacity to fall while Spool retains custody.
Report the useful operating points across CPU, RAM, disk and latency rather
than hiding trade-offs in one score. Produce a single run record with each
hypothesis/null decision, counterexamples, release blockers and the smallest
next test that would change the decision.

The experiment/protocol files remain isolated from implementation commits.
Git metadata was read-only in the prior session; if that persists, keep exact
pre-run content hashes and report the missing policy commit explicitly.
