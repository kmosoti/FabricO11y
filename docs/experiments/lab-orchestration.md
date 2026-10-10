# Standing labs and experiment orchestration

Status: adopted coordination workflow for development and small-deployment
investigations. The experiment queue below is a draft, not a registered protocol
or an executed result. Product contracts, independent oracles and qualification
gates remain owned by their existing documents.

## Ownership and continuity

Maintain at least three standing labs. A lab is a persistent responsibility and
evidence record; it does not require a continuously running agent.

| Lab | PI responsibility | Initial question |
| --- | --- | --- |
| Capacity and lifecycle | Bound working sets, queue growth, disk use and lifecycle costs | Which of retained history, ingestion rate and query activity drives remaining memory growth? |
| Query and availability | Preserve exact answers, completeness and visibility across storage transitions | Does data remain available and affordable to query through the first seal, recovery and retention? |
| Recovery and operations | Preserve custody under failures and establish usable installed behavior | Do spill/publication failures recover safely, and does the supported package work within its service limits? |

The inline coordinator owns admission, cross-lab synthesis and the consolidated
decision log. Each active lab names one PI, with GPT-6.1-SOL medium as the current
preference. GPT-6 Luna assistants receive bounded research/preparation tasks.
The coordinator may act as a lab PI for a small task. Roles persist in records;
agent instances activate and retire as work requires.

Use the available concurrency slots flexibly. Four slots are currently available;
root + one PI + two assistants is one useful arrangement, not a required topology.
Several labs may prepare independently when useful. Do not create idle agents or
duplicate assignments to fill slots. Future slot availability does not grant a
larger spending or workload budget.

## Budget and dispatch

Before a campaign, record its estimated token/cost envelope, task allowance and
machine-work budget. Reserve approximately 20% of the agent envelope for
unexpected findings. The coordinator sizes the initial envelope to the scoped
work rather than treating all available tokens as a spending target.

Track actual usage where telemetry is available. Otherwise record estimates as
estimates; output length and agent count are not billing measurements. A written
budget is a dispatch control, not a claim that the runtime enforces a token cap.
At each task completion, reconcile usage and remaining budget before dispatch.
Escalate from Luna when an unresolved ambiguity warrants PI attention; do not
repeat the same assignment unchanged after an uninformative result.

Each task packet contains:

- Lab, accountable PI, task ID, dependencies and owned paths.
- One question, hypothesis/null where applicable, and the decision it informs.
- Relevant source/evidence pointers and required invariants.
- Allowed actions, estimated token allowance, bounded output and stopping rule.
- Expected artifact and executable validation; no request for private reasoning.
- Workload resources, storage, deadline and cleanup obligations if execution is involved.

Typical assistant outputs are a source-grounded finding, fixture, independent
accounting artifact, minimized counterexample or bounded comparison. Prefer
targeted context and concise artifact handoffs over full-history forks and
repeated conversational summaries. The PI reviews evidence; model agreement is
never a correctness or release gate.

## Experiment admission and evidence

Agent preparation and workload execution have separate admission queues. Initially
serialize project workloads so multiple 20 GiB service ceilings cannot multiply
the owner's aggregate allowance. Parallel workload admission requires verified
aggregate resource enforcement and a registered contention design when comparing
performance. Each build, test, experiment and validator still uses
`python3 tools/resource_group.py -- COMMAND`.

Use the mounted data drive for caches and owned scratch, fail closed on missing
containment/storage, preserve meaningful failures before cleanup, and retain
commands, exits, source revisions, resource observations and cleanup receipts.
The default local envelope remains 16 GiB memory high, 20 GiB maximum, no swap
and a 30-minute deadline. It is not a development/small deployment budget.
Remote limits must fit actual host resources. Longer runs and qualification,
destructive faults, installation or specification changes retain their existing
explicit-scope requirements in [AGENTS.md](../../AGENTS.md).

Queue states are proposed, preparing, ready, running, recorded and reviewed,
with deferred/cancelled states carrying reasons. Run outcomes are separately
passed, failed, interrupted, inconclusive, not run or environment unavailable.
A retry has a new run ID; it does not replace the original outcome.

Before a measured comparison, register seeds, fixed workload, varied parameter,
baseline, measurement boundaries, guards, repetition/order and decision rule.
Existing registered protocols are not edited after a result. Checker or protocol
revisions require their separate provenance and policy commit. A proposed queue
entry does not authorize changing an oracle or executing qualification.

## Initial queue

The [development/small campaign plan](benchmarks/dev-small-lab-plan.md) expands
this queue into aims, hypothesis/null pairs, sequencing, budgets and admission
conditions. It is written planning, with no new experiment result.

Generalize across development and small deployment profiles: logs, metrics and
supported traces, with declared traffic/retention budgets and concurrent queries.
Development includes quiet periods, bursts, restarts and first sealing; small
includes sustained offers, accumulating history, retention and recovery.

| ID | Owner | Proposed experiment and hypothesis/null | Deciding evidence |
| --- | --- | --- | --- |
| C1 | Capacity | Vary history at fixed ingress, then ingress at matched history, with query-off/Scan controls; Walk follows in Q2. Hypothesis: history/query work drives much of remaining memory; null: costs remain stable or another component dominates | Exact custody/answers; stable clocks; CPU, RSS/HWM, cgroup anon/file/kernel, IO, bytes and backlog; confirm promising comparisons with fresh alternating repeats |
| Q1 | Query | Exercise a declared near-rotation fixture, followed separately by a fresh development lifecycle. Hypothesis: journal-to-Segment transitions preserve availability; null: transitions introduce missing answers or excessive visibility delay | Independent query/delivery grading, observation-to-query and ACK-to-query distributions, sample counts, seal overlap and latency by query kind |
| Q2 | Query | Compare Scan/Walk on matched demand and separately test retention/completeness. Hypothesis: a plan reduces work without changing answers; null: gains fail guards or answers differ | Fresh confirmation, cold/warm costs, independent snapshot/retention/index-fallback checks |
| C4 | Capacity | Enforce candidate development/small application limits under ordinary load and overload. Hypothesis: custody, boundedness and drain hold; null: a profile fails | Actual service cgroups, memory/CPU/queue/IO accounting, mixed-signal load and expected-gap ledger |
| C2 | Capacity | Complete bounded-builder registered shape/repetition/scaling acceptance. Hypothesis: bounds and exactness survive all registered inputs; null: a workload violates a bound or invariant | Existing BS acceptance criteria; separately recorded resolution of the physical filter-count checker mismatch |
| R1 | Recovery | Prepare stage-specific IO/space and actual process-kill fixtures. Hypothesis: custody and exact answers survive supported failures/retry; null: a stage loses data, duplicates answers or leaks unbounded scratch | Preserved fault traces, independent oracles, cleanup/custody accounting and restart convergence; execution scope must cover injected faults |
| C3 | Capacity | Unchanged soak and outage recovery on the candidate revision. Hypothesis: memory/backlog settle and queues drain; null: sustained growth or a registered gate failure | Frozen protocols, resource receipts and original gates; deferred until longer deadline and qualification scope are established |
| R2 | Recovery | R2a: native mixed-signal integration before C4; R2b: supported installed profile. Hypothesis: signals preserve semantics and package services enforce bounds; null: a behavior fails | Exact telemetry/restart checks; installed UID/limits, authorized/denied sources, TLS/control; disposable supported host for R2b |

These entries are not new results. The starting evidence and limitations are in
[lab run 01](benchmarks/readiness-labs-run-01.md), [bounded-sealer acceptance](../milestones/bounded-sealer.md)
and [qualification](../QUALIFICATION.md). Keep the existing freshness targets;
30-second processing headroom does not amend them. Deliberate overload bursts
test bounds/correctness; steady latency gates apply only within their registered
conditions. Low traffic must remain queryable without waiting for journal fill.

## Synthesis and exploration

At each checkpoint, record observations, deductions, hypotheses and uncertainty
separately. Compare across labs for interactions: history versus input rate,
query work versus sealer contention, and reclaim progress versus disk pressure.
Nominate an optimization only when evidence identifies a mechanism and a
controlled comparison can distinguish it from alternatives.

An exploratory branch records its triggering observation, competing explanations,
smallest discriminating experiment, allowance and stopping condition. Spend the
reserved exploration budget on these branches; defer others with their rationale.
Preserve null results and failures. Publish a tested operating envelope only for
the revision/profile measured; neither a lab review nor a finite screen qualifies
Fabric for release.
