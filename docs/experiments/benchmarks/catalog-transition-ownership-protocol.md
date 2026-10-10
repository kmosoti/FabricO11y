# Ownership across cancellation and storage transitions

Registered before execution. The objective is a reliable application surface
for human and automated clients: stopping a caller must not strand storage,
continuing a snapshot must not expose later metadata, and request admission
must precede expensive ownership. These are finite correctness investigations,
not deployment qualification or a new throughput comparison.

## Competing explanations and fixed properties

The lifecycle hypothesis is that cancelling a listening `serve` future bypasses
the explicit stop/join block and leaves the journal owned indefinitely. The null
is same-process reopen within three seconds. An occupied TCP port is a control:
ordinary bind failure must still release ownership. Use a generated EC TLS key,
one MiB journal, 64 KiB rotation, two Tokio workers, and bounded startup/abort
waits. Record the actual listening/cancellation/error/lock outcomes before any
assertion. The first execution holds production unchanged.

The query hypothesis is that a Segment crossing an old page's upper group bound
contributes its whole manifest's freshness and receive bounds. The null is that
the complete answer envelope is unchanged. Use three actual FrameLog groups,
two known Unicode log bodies each, and receive/observation times 100, 200, 900.
Capture first pages at group 2, append group 3, then rotate, publish and reclaim
all three together. Continue old pages before and after publication and with
new readers, under Scan and Walk. The unchanged Python oracle grades ten full
chains; two missing-row answers must be rejected. Fresh queries must include
all six rows. After actual retention removes the Segment, both old tokens must
return Gone. Record all verdicts before the final assertion. The fixture bounds
each oracle to five seconds, chains to eight pages and the full tree to two MiB.

The admission hypothesis is that a per-request body limit and downstream byte
queue do not bound concurrent buffered HTTP bodies or abandoned blocking query
work. The null is that excess incomplete requests are refused before their
bodies complete, and that capacity remains owned until admitted work finishes.
The registered candidate has independent fixed pools of 16 batch requests and
two query requests. Excess requests receive an existing-form 503 response;
no admitted query changes its answer. Register the concrete pressure schedule
in an additive protocol before that baseline is run. Follow-up tests must cover
cancellation/reuse of body slots and retention of the query work permit inside
blocking execution. No latency/RSS/throughput benefit follows from permit counts.

## Candidate mechanisms and rejection rules

Lifecycle: an explicit Rust owner signals HTTP and sealer shutdown on drop,
joins workers off the executor, and reports worker panics on ordinary completion.
It must also cover partial startup and cancellation while joining. Do not abort
durable commits, weaken sync order, or change the sealer's individual-pass retry.

Query: manifest aggregates remain the fast path when the entire Segment lies
inside the snapshot. Only crossing Segments need snapshot-filtered raw metadata.
The raw scan must remain bounded, and unavailable/corrupt raw data must still be
reported incomplete. No cached descriptor or metadata grants a retention lease.

Admission: acquire before body extraction; transfer the query permit into the
actual blocking task, so cancelling its async waiter cannot release capacity
early. Keep ingestion independent from query saturation. A body-only limiter
or authentication-only change is insufficient. No generic framework, new core
dependency, persisted format, oracle, sync rule or qualification-policy change.

Reject a candidate if an unchanged regression, independent oracle, existing
custody test or fast gate fails. Keep meaningful original failures, exact
commands, sources, hashes, verdicts and full small fixture state. Read back
every archive member and its exact bytes before removing raw scratch. Corrupt,
missing and duplicate archive members must be rejected by existing controls.

## Execution and resource ownership

The three labs prepare isolated tests; root reviews, implements/integrates and
executes serially. No more than three assistants. Existing campaign/frontier,
stage and evidence budgets remain in force; none is reset. The mounted data
drive holds scratch/builds. Every workload and validator runs through
`tools/resource_group.py`, with 20 GiB max, 16 GiB high, no swap and the normal
30-minute outer deadline. No remote work or privileged installation.

| Job | Stage | Deadline | Reserve after coordinator snapshot | Driver cap |
| --- | --- | ---: | ---: | ---: |
| `catalog-transition-repro-01` | preparation | 180 s | 3 MiB | 3 MiB |
| `catalog-admission-repro-01` | preparation | 180 s | 3 MiB | 3 MiB |
| `catalog-transition-fixed-01` | preparation | 240 s | 3 MiB | 3 MiB |
| `catalog-transition-checks-01` | verification | 600 s | 1 MiB | 512 KiB |

Dispatch uses `completion/run_job.py` then `catalog/coupled_admit.py` with the
listed reservation and the new transition driver. The first job runs unchanged
cancellation and snapshot tests. An expected Rust exit 101 is recorded as a
failed Rust check; a coordinator success only means the exact counterexample
was established and preserved. Wrong failures stop the investigation. Driver
classification must reject a changed trace containing a successful reopen or
all-successful published snapshot verdicts.

The fixed job reruns the unchanged new regressions and relevant delivery/startup
tests. Final checks run the fast and manual documentation profiles, preserving
their exact receipts and cleaning the known Bun copy by exact archive reference.
Full native lifecycle coverage is also included by the ordinary workspace tests.
If a job fails unexpectedly, diagnose from its preserved state and register any
additional run before dispatch; no silent retry or deletion of failures.

The synthesis must distinguish demonstrated defects/fixes, source-only risks,
and untested operating limits. Further frontier work should target admitted
custody under cancellation, retention-boundary metadata, input pressure and
recovery debt. Long soak, installation and physical power loss remain separate.
