# Root-cause investigation simulations

Status: packet preparation and a separate
[native journal/restart matrix](rca-journal-findings.md) have completed.
Fifteen encoded Batches, 56 query recipes and four preparation controls passed;
the later native pilot and fourteen case/plan cells checked actual Fabric answers.
The worked interpretations below remain **tabletop reasoning**. Published-storage
and interpretation grading have not run; no new product promise is made.

The owner requested RCA flows after reviewing the unequal log/metric/trace
pressure coverage. The question is whether an investigator can use Fabric's
evidence to distinguish plausible incident mechanisms, recognize missing
information and revise a conclusion when late evidence arrives. Fabric currently
supplies storage and queries; it does not implement automatic causal diagnosis.

## Concrete tabletop flow

An API request times out after 1,000 ms. An investigation follows this sequence:

1. Establish the affected node, request/trace identity and time window. Read
   collection gaps, retained boundaries, freshness and unavailable sources.
2. Fetch the relevant logs and **all pages** for the trace. Assemble parent-child
   relationships using trace/span/parent IDs, rather than timestamp order alone.
3. Query pool occupancy/capacity, host context and counter history/rates.
4. Compare competing explanations and list the evidence that would distinguish
   them. Report the immediate mechanism separately from its underlying cause.
5. Propose a bounded intervention or additional instrumentation. Refresh the
   investigation when new evidence arrives, retaining the original snapshot's
   result and the reason for the revision.

Two deliberately similar incidents produce different interpretations:

| Synthetic evidence | Incident A | Incident B |
| --- | ---: | ---: |
| Request span duration | 1,000 ms | 1,000 ms |
| Connection acquisition duration | 800 ms | 5 ms |
| Downstream call duration | 20 ms | 800 ms |
| Pool occupancy/capacity | 2 / 2 | 2 / 16 |
| Log clue | Pool capacity changed from 16 to 2 | Deployment version changed |
| Supported immediate mechanism | Waiting for a connection | Waiting on the downstream call |
| Still unresolved | Why capacity changed; whether restoration fixes this workload | Network, execution, retries or queueing inside the downstream interval |

In A, acquisition occupies 80% of the request's elapsed time in this constructed
non-overlapping trace. Pool occupancy is at its limit and downstream time is
small. This supports investigating pool saturation. It does not establish that
raising the pool limit is a safe universal fix. A controlled restoration at
unchanged offered work is the proposed discriminator, **not an executed result**.

In B, downstream duration occupies 80%, while acquisition is short and the pool
has room. The coincident deployment log is insufficient to blame deployment.
The next useful evidence is inside the downstream interval. Neither example
justifies summing arbitrary child durations: real spans can overlap, be sampled,
be missing or include asynchronous work.

A third flow begins with the same timeout but lacks the downstream span. The
correct initial answer is **insufficient evidence**. After a delayed 800 ms span
is delivered, a fresh query supports downstream waiting. An already-open page
chain must keep its original data boundary. That change of conclusion is a
successful investigation if it cites the new evidence; silently rewriting the
old snapshot is a correctness failure.

## Scenario packet

The [scenario deck](../../../tools/bench/labs/rca/scenarios.json) records known
injected conditions, available evidence, allowed conclusions, remaining unknowns
and discriminators. The [preparer](../../../tools/bench/labs/rca/prepare.py) uses
the existing independent Python fixture encoder to produce actual OTLP payloads
inside existing version-one Batches. It changes no wire format or oracle.

| Scenario | Expected investigation outcome | Counterexample to reject |
| --- | --- | --- |
| Pool wait | Acquisition delay and full occupancy support pool waiting; deeper cause remains conditional | Inferring saturation after the occupancy evidence is removed |
| Downstream wait | Long downstream duration supports investigating that dependency | Blaming a coincident deployment without discriminating evidence |
| Counter reset | 100 to 160 over 10 s gives 6/s; the next value 8 with a new start time produces a reset | Reporting −15.2/s or proving traffic disappeared |
| Missing evidence | Explicitly abstain when the required downstream evidence is absent | Treating `complete=true` and no rows as proof of a healthy dependency |
| Late span | Abstain initially; revise a fresh investigation after the delayed span arrives | Allowing the late span into an old paginated snapshot |
| Clock skew | Use parent identity and the 800 ms local duration; cross-host wall-clock order remains unknown | Reversing cause and effect because the child's clock is 2 s behind |
| Host-pressure confounder | Report correlation and request missing application evidence | Declaring low available memory to be the cause of an otherwise unexplained stall |

The reset variant with unchanged start time must also produce a reset on a
decrease under the existing counter contract. Negative-control variants are
specified here; the preparer emits the seven base cases only. They must be
implemented and rejected before claiming the future investigation checker works.

The preparer separates:

- `controller/producer.jsonl`: exact synthetic Batch bytes, source identities, sequence,
  digest and initial/late delivery phase; no invented ACK or receive timestamp.
- `investigator/playbooks.json`: eight supported query templates per case and required
  metadata/pagination handling. Investigator-visible case IDs are opaque and
  do not contain the fault name.
- `controller/truth.json`: injected conditions and permitted conclusions.
- Source copies and a digest manifest under `controller/`, explicitly marked
  preparation-only. The source deck also contains the answers and must never
  be included in investigator input.

Only the `investigator/` directory is intended for an investigator, alongside
query responses. Access to any controller input or source deck invalidates a
claimed blind evaluation. Separate directories are not process isolation; a
future agent evaluator needs restricted tools/context and a held-out case order.
Source-only review caught and corrected an initial layout that would have
included the answer-bearing source deck with the playbooks. The preparation
checker now verifies the restricted file set and rejects an injected truth file.
This checks packet layout, not access isolation for a future investigator process.

The seven cases describe fourteen initial Batches and one delayed Batch. Custom
pool/request metrics are fixture payloads sent through the Batch interface;
Fabric has **no general application OTLP metrics receiver**. A later native
exporter experiment must use the actual trace endpoint and separately state how
application metrics enter. The dependency emits a current metric even when its
span is absent, deliberately exposing another trap: **node freshness does not
prove freshness or completeness of every signal from that node**.

## Query and causality boundaries

The [current query contract](../../architecture/retained-history.md) supports
log substring, named metric history, counter rate and span trace-ID/name queries.
The playbook performs correlation in the investigator client; no SQL joins,
arbitrary attribute predicates or histogram analytics are assumed. Request and
trace IDs are included in the fixture's log body because current log search
filters the body, not a dedicated trace-ID column.

Each logical query must retain its full envelope: `complete`, `gaps`,
`unavailable`, `freshness`, retained boundaries and snapshot/page tokens.
`complete=true` establishes completeness of the retained scan, **not** of the
application's instrumentation or the whole trace. Omitted spans can reflect
sampling, exporter loss, lateness, missing instrumentation or absence of a call.

Gap filtering uses Batch **receive time**, whereas metric/log/span predicates
use their specified telemetry times. The packet therefore adds a separate
source-coverage query with a broad receive-time-compatible range. Otherwise a
replayed historical event window could hide a recently reported collection gap.
Gap text alone need not identify an exact missing event-time interval.

Span range filtering is by **start time**, not interval overlap. A caller needs
an appropriately widened window for long-running or skewed spans. The fixed
fixture window includes the clock-skew case; an outside-window negative control
must still remain excluded under the contract.

Different queries have their own snapshots. Current page tokens do not provide
a shared transactional snapshot across logs, metrics and spans. A native fixture
can pause intake between investigation steps to establish a controlled common
population. A live investigation must report that boundary or reconcile new
evidence; it must not assert atomic cross-signal consistency.

In a synthetic experiment, known injection history supplies causal ground truth.
In a production incident, telemetry commonly supports an immediate mechanism
or a ranked hypothesis, not proof of the ultimate root cause. Score calibrated
abstention, falsification and useful next probes alongside correct identification.

## Native execution design and lab ownership

Before native execution, register its precise checker/negative-control outcomes
in a separate change. Initial preparation could not register or execute under
the read-only session. Access has since been restored; the separate
[fixture preparation protocol](use-case-preparation-protocol.md) is registered
in commit `f6e8276`. It does not register the native matrix below.

1. **Collection/reliability lab:** deliver exact synthetic Batches to an isolated
   TLS server using existing interfaces, record actual ACKs and receive times,
   replay duplicates and deliver the delayed span. Later extend to an actual SDK
   → loopback OTLP/HTTP → Spool → server path; direct Batch injection does not
   test that path.
2. **Storage/memory lab:** preserve producer identity/bytes through active
   journals, publication and graceful restart. Require actual manifests and
   journal reclamation before calling a view sealed. Base fixtures are small;
   explicit bounded padding and its producer ledger are needed to cross the
   existing minimum 64 KiB journal-file threshold.
3. **Query lab:** retrieve full chains under Scan and Walk; grade them against
   the unchanged Python oracle after independent producer/recovery byte equality.
   Evaluate the investigation's evidence references and allowed conclusion. Keep
   the oracle verdict separate from the causal interpretation verdict.

The proposed matrix is seven scenarios × two plans × four observations
(initial, after delayed delivery, after publication, after restart): **56
investigation episodes**, plus held-old-snapshot checks and negative controls.
An initial admission should be one scenario and one plan; expand only after its
controls and resource checks work. An agent-based evaluation is a later option:
use query-only access, exclude truth files, hold out variants, and record actual
model/token usage. No language-model judgment becomes a product acceptance gate.

Record per-episode query count, returned/read bytes where available, elapsed
time, CPU, RSS/cgroup memory, visibility delay measured by the driver, snapshot
changes, missing evidence, correct conclusions and unsupported attributions.
For agents, additionally record input/output tokens when available. Reuse the
existing 4 GB server cap, 20 GiB aggregate laboratory, zero swap and 100 GB owner
storage ceiling. Start with an 8 MiB fixture/evidence bound, at most 32 HTTP
requests per episode and a 180 s first native trial deadline. Admit against the
existing remaining campaign allowance; this preparation allocates no new time.
Avoid remote execution until the local case passes; later edge use retains the
host-specific containment and cleanup rules.

## Execution status and access failure

The initial containment preflight was:

```text
python3 -B tools/resource_group.py -- true
exit 2
resource group: NOT RUN: [Errno 30] Read-only file system:
'/run/media/kmosoti/data/FabricO11y/scratch/work-lh2_61zl'
```

It failed before starting a service or creating the scratch directory. No RCA
workload, fixture generator, validator or documentation check ran in that attempt.
There was no temporary RCA fixture to clean. Preserve this original failure;
restored access does not change its outcome. Earlier pressure-round cleanup
remains outstanding.

The newly registered preparation and checker command is:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/rca/job.py \
  --id rca-preparation-01 --lab query --seconds 60 --reserve-mib 8 -- \
  python3 -B tools/bench/labs/rca/verify_preparation.py \
  --out docs/experiments/benchmarks/data/hammer-reference-01/query/rca-preparation-01
```

It stages data under launcher-owned data-drive scratch, generates at most 1 MiB
of packet documents plus manifest, and reserves 8 MiB for authenticated inputs,
source and validation evidence in the existing pressure ledger. It does **not**
start Fabric, grade a diagnosis or run the proposed 56-episode matrix. A subsequent
[native journal/restart matrix](rca-journal-findings.md) now supplies real-server
query evidence; published-storage and interpretation grading remain unrun. The broader
[application use-case plan](../../research/application-use-cases.md) adds
deployment, APM, browser and job investigations around this foundation.

## Executed fixture validation

The command above ran on 2026-10-09 with exit 0. The
[validator report](data/hammer-reference-01/query/rca-preparation-01/result.json)
records all fifteen payload digests/decodes, fourteen initial and one delayed
Batch, seven scenario identities, metric units and values, source sequences,
span identity/duration/skew, and 56 supported query recipes. The unchanged Python
oracle returns 6/s followed by reset for the counter fixture and two then three
spans as the delayed Batch is included. Receive timestamps used for these oracle
calculations are synthetic in-memory inputs, not measured server observations.

All four deliberate defects were rejected for their expected reason: payload
mutation, missing initial Batch, delayed Batch relabelled initial, and a
truth-bearing file in investigator output. The generator/validator snapshots,
small packet and hashes are retained under the report's directory; no independent
oracle was modified. This proves neither native query exactness nor a diagnosis.

The [coordinator receipt](data/hammer-reference-01/coordinator/rca-preparation-01/receipt.json)
records 4.66 s of charged time and 85,897,216 bytes (81.9 MiB) cgroup peak memory,
including coordination. The packet evidence was 83,115 bytes before its result
report. Work ran beneath
`/run/media/kmosoti/data/FabricO11y/scratch/work-z_8ijp1g` with 20 GiB memory max
and swap disabled. The launcher receipt at
`target/resource-containment/runs/fabric-work-cbc12051a1e54596a4245ce4317d1121.json`
records cleanup; the scratch path no longer exists. No Fabric server, remote
workload or native query ran. The earlier failed preflight remains unchanged.
