# Native collection, TLS custody and query lifecycle

Registered before executing this fixture. This is a finite correctness screen
for development/small application behavior, not a profile-capacity comparison or
deployment qualification. Follow the
[startup recovery investigation](catalog-startup-recovery-protocol.md); hold the
recent collection/projection algorithms fixed except for the separately
reproduced TLS startup-order correction.

## Invariant and discrimination

Every locally committed producer Batch must survive durable TLS ACK, exact retry,
node/server restart and journal publication/reclaim exactly once. Complete HTTP
query chains must agree with the unchanged independent Python oracle. Failure
of any custody, source-fidelity, completeness or query check rejects the fixture;
process exit alone is insufficient. No latency or throughput threshold is tested.

Before each delivery, capture exact committed Spool frame bytes and their Strand/
sequence identity into a cumulative producer ledger. Before server replay can
supply receive timestamps to the query oracle, require a bijection of recovered
identities AND exact bytes with that producer ledger, with no duplicates. A
deliberately missing recovery record must fail this comparison. This prevents a
lost Batch from vanishing from both the answers and their expected population.

## Fixed fixture

Use one actual Spindle with fixed host fixture files, one real local TLS server,
and a generated private CA. Input is exactly 3 MiB of ASCII `x`, then newline,
`normal-λ🦀` and newline. Collect four bounded log passes and deliver each, with
committed offsets 1/2/3 MiB then EOF. Require exactly one initial oversized-line
gap and one normal row; reopen both sides after pass two, restoring ACK 2 and the
same Strand. Resend the exact second producer Batch after restart and require
duplicate ACK 2 with no additional logical record. This simulates an exact retry,
not actual transport loss injection.

Commit one valid OTLP span through public `Spindle::commit_traces`, deliver it,
and grade the journal-only state. This exercises trace custody/projection; the
OTLP/HTTP listener and SDK exporter remain outside this fixture. Append exactly
40 tagged Unicode lines of 2,048 body bytes each (plus newlines), collect/deliver
them, then collect/deliver a real metric cycle to cross the 64 KiB journal rotation.
Wait at most 30 seconds for a published Segment AND absence of sealed journals.
Grade the published state and a subsequent restarted published state.

For each of the three stages, run both Scan and Walk servers and drain five
query chains: all logs (limit 7), Unicode substring logs (limit 3), network receive
counter history (limit 2), its rate, and spans (limit 1). The six plan/stage cells
give 30 full chains. All pages must be complete; both plans' whole answers must
match. Require actual nonempty collected log/metric output and exactly one span.
Delete a row and duplicate a row in the all-log answers of each cell; the unchanged
oracle must reject all 12 mutations. Expected final producer log bodies come from
the independently constructed source-line list: 41 exact bodies and one gap.
Persist every command, actual exit, verdict and page transcript.

Node Spool and server journal ceilings are 2 MiB; server rotation 64 KiB, retention
4 MiB/one day, one sealer worker. These deliberately small rotation settings
exercise publication with bounded inputs. They do not establish retention under
pressure or the default 64 MiB rotation's performance. Plans use unchanged
semantics. Fresh complete chains after restart are tested; continuation of an old
page token through publication, cancellation and power loss remain outside scope.

## Dispatch, preservation and resource limits

This amendment supersedes only the middle job's 10 MiB reserve/8 MiB driver cap
in the startup registration: `catalog-native-lifecycle-01` reserves **18 MiB**
after coordinator snapshots and permits at most 18 MiB driver evidence, including
up to 16 MiB complete raw fixture plus archive framing, sources and receipts.
This does not increase aggregate evidence or resource caps. Additional exact
protocol reclamation must finish and admission must fit before execution.

The fixture checks a 16 MiB raw-tree guard, 240-second internal deadline, 64-page
chain limit and 256 KiB response limit. TLS request timeout is the existing ten
seconds; deliveries/oracle children have 15-second bounds. Root's preparation
job has 360 seconds, with a 320-second child deadline and remaining cleanup
margin. Stop, preserve and report any excess; never discard unmatched failed
state to meet the evidence bound. No seeding of large histories or C5 workload
is included.

Root runs corrected startup tests, the native fixture and the existing delivery
integration target serially. The fixture retains one raw tree when the registered
driver requests evidence. Root archives every file and verifies complete member
coverage, lengths and exact decoded bytes against originals before removing it.
Unexpected failure retains its original command/error and attempts the same
full-state preservation; a preservation failure keeps the raw tree and both
errors. Successful ordinary workspace tests remove their own temporary tree.
No nested success archives or duplicated unpacked state are needed.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-native-lifecycle-01 --lab coordinator --stage preparation --seconds 360 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 18 -- python3 -B tools/bench/labs/catalog/native_lifecycle_run.py --mode lifecycle
```

The final `catalog-native-lifecycle-checks-01` remains the registered 600-second
verification job: fast plus manual documentation profiles, 1 MiB reserve after
snapshots and 512 KiB driver cap. Exact sources/protocols/lockfile and relevant
working-tree diff accompany results. All descendants share the default 20 GiB
max/16 GiB high/no-swap containing cgroup and 30-minute outer deadline, on the
mounted data drive. No remote work, new oracle, format/sync change or qualification
is included; all elapsed time stays in the existing frontier and stage budgets.

Lab assignment: operations supplies the startup counterexample; query supplies
the lifecycle regression; capacity supplies exact evidence reclamation. Root
reviews and executes serially. Agent preparation uses small existing assignments
with bounded handoffs; output limits are coordination targets, not measured token
billing or runtime-enforced token budgets.
