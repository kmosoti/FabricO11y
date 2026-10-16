# Registered soak exposure revision 3

Prospective registration, 2026-10-13, for the finite release soak. The owner
authorized a 7,500-second outer deadline; coordination comment
[C-062](https://github.com/kmosoti/FabricO11y/issues/59#issuecomment-6137557678)
admits registration, implementation and controls, **not a full run**. The
[reviewed proposal](https://github.com/kmosoti/FabricO11y/wiki/Experiment-benchmark-soak-r3-amendment-proposal-01)
is wiki revision `02f402a`. No R3 result is claimed here.

## Question and fixed contract

Does the exact packaged candidate remain correct and bounded during **7,200
seconds of offered workload**, including the unchanged 60-second warmup? R2's
60 + 9 × 600 = 5,460 offered seconds and 7,200-second command allowance retain
their original meanings in [R2](soak-protocol-r2.md). A timeout allowance does
not establish elapsed exposure. Historical results are not R3 acceptance.

Retain 100 identities and 100 workers, seed `0xA11FA001`, two 512-byte logs per
identity per second, 32 metric points per 15 seconds, 64 MiB journal files,
4 GiB journal, the exact packaged retention defaults, default query plan and
server/companion CPUs 0–1 versus simulator CPUs 2–3. Preserve production
self-observation and authenticated production access. No obsolete writer
selector, allocation-counting feature or alternative daemon is admitted.

Queries remain every 10 seconds, management every 60 seconds and RSS/CPU
sampling every 5 seconds; UI polling and resource sampling retain their
registered cadence. Preserve every query page, status, failed attempt and raw
answer. HTTP 429 is a refusal, never successful query or management evidence.
Delivery, query and rate oracle bytes stay unchanged. The source-matched Spool
inspection, complete prefix, ACK/retained-set and exact recovery requirements of
R2 remain mandatory, including refusal of inspection stderr even with exit 0.

## Exposure and nine cohorts

The literal measured cohort boundaries in seconds are:

`60, 853, 1646, 2440, 3233, 4026, 4820, 5613, 6406, 7200`.

Warmup is `[0,60)`; adjacent boundary pairs define nine nonempty half-open
cohorts. Classify integer-nanosecond timestamps against the simulator's actual
`began_unix_ns`. An exact interior boundary belongs to the following cohort;
60 s − 1 ns is warmup, and 7,200 s is outside exposure. Use the same boundaries
for ACK attempt starts, RSS and CPU. Latency remains actual attempt end minus
start. Do not use nine 600-second divisions or the harness's later launch time.
Sample backlog every 60 seconds and compare 60 s with 7,195 s. Preserve original
creation times, retry attempts and right-censoring separately from attempt
latency and creation-to-first-ACK delay.

The simulator command and original simulator summary must both specify 7,200
seconds, 100 identities and the registered seed. Bind those raw bytes and all
worker offer evidence. Record a monotonic exposure interval anchored to the
independently reported simulator begin, its conversion witnesses and uncertainty.
The interval must cover the entire 7,200 seconds, with server, queries,
management, UI and sampling active throughout. The final scheduled second is
7,199; its offer evidence does not by itself prove the full final second. A
simulator exiting early, missing worker offers, an observed interval one
nanosecond short, lost probes, or a forced/unconfirmed stop invalidates the
trial. A fabricated 7,200-second summary over a 5,460-second source is refused.

## Unchanged ten gates

Keep `soak_tier.evaluate` and its existing adverse controls unchanged. R3's
exposure, containment and custody are additional prerequisites to `oracle`;
they do not replace or relax any of the ten gate names or thresholds.

| Gate | Rule |
| --- | --- |
| `oracle` | unchanged delivery oracle, exact companion custody and containment all succeed |
| `exits` | simulator and production server both exit 0 |
| `ack_p99_le_1s_every_window` | all nine cohorts are nonempty and ACK p99 ≤ 1,000 ms in each |
| `no_growing_backlog` | backlog at 7,195 s ≤ backlog at 60 s + 100 batches |
| `server_rss_le_2gib` | server VmHWM ≤ 2,097,152 KiB |
| `no_rss_growth` | median last-cohort RSS ≤ twice first-cohort median + 65,536 KiB |
| `queries_complete` | no failed query, no incomplete page, at least one query |
| `query_p99_le_2s` | query p99 ≤ 2 seconds |
| `management_ok` | no failed management call and at least one succeeds |
| `sealing_caught_up` | no sealed journal remains and at least one Segment exists |

The historical 300-second seal upper bound is retained. R3's shorter shared
drain/seal tail is a separate hard prerequisite: its exhaustion fails the run
and grants no additional time or seal-only exception for a full trial.

## Phase boundaries and admission headroom

The global 7,500-second deadline begins at admitted launcher start, before
setup. Phases are ordered, disjoint, recorded with monotonic integer timestamps
and bounded by the smaller of their own remaining allowance and the global
remaining deadline. No phase borrows another phase's allowance.

| Phase | Ceiling |
| --- | ---: |
| `setup` — enrollment, browser readiness and fixture admission | 170 s |
| `exposure` — 7,200 s offers plus measured boundary observation/transition | 7,210 s |
| `drain_seal` — shared simulator drain and sealing | 60 s |
| `shutdown` — orderly production shutdown | 20 s |
| `checkpoint` — stopped raw tree and resource checkpoint | 20 s |
| `closeout` — remaining owned processes and temporary fixtures | 20 s |

These ceilings total 7,500 seconds with no overhead slack; they are failure
boundaries, not a guarantee that a run fits. Before and after each blocking
operation check remaining phase/global time. Bound socket reads, joins,
subprocess waits and stop escalation, and cancel cadence sleeps when stopping.
The simulator's watchdog sets a stop flag before joining workers; it is not
an absolute drain deadline. Forced shutdown, exhausted budget, phase overlap,
unexpected exit or unconfirmed cleanup fails and preserves evidence.

The prospective [C-081 amendment](https://github.com/kmosoti/FabricO11y/issues/59#issuecomment-6138310799)
keeps the actual offered interval and cohort edges exactly 7,200 seconds. The
ten-second allowance covers only native boundary anchoring, observation and
transition; it never excuses early simulator exit, shorter offered coverage,
rounding, delayed probes or borrowing drain time. The previous 180/7,200
allocation and its source-control results remain tied to their original
revisions. This separately registered change does not reinterpret them.

**C-062 S1 must be satisfied before any full run.** Measure each of the five
non-exposure phases on real R2 evidence or a short smoke at the full registered
data rate. Post each observed maximum, source receipt/hash and ceiling. Require
the ceiling to accommodate **at least 25% margin**: `ceil(5 × maximum_ns / 4)`
must fit. A missing phase, synthetic-only measurement, unconfirmed stop,
unbound source or maximum one nanosecond beyond that margin blocks full
admission. Use same-filesystem rename of the stopped trees for checkpoint;
hardlinking is an alternative only with complete link/inventory accounting.
Do not copy the raw dataset into the 20-second checkpoint phase. If any measured
phase plus margin does not fit, request a concrete larger deadline from the
owner with the actual numbers; do not infer approval or shave exposure.

Also require a separate real `exposure_transition` sample with the same source
receipt/hash and confirmed-stop requirements, maximum at most eight seconds
before the 25% margin. Setup maximum is at most 136 seconds. The transition is
part of exposure, not a seventh phase or additional outer allowance. Missing
transition evidence blocks FULL even when all five other samples fit.

Register the literal `PhaseTransitionControls` before correction: the prior
allocation must differ from the current table, and five fitting samples without
transition evidence must be refused. The independent literal controls retain
each phase/global boundary and check the six maxima at the exact 25% boundary
and one nanosecond beyond it. No runtime schedule or helper authority changes
in this registration. Native helper authority is registered separately; no
simulator build, smoke or FULL is admitted here.

## Stopped checkpoint and separate grading

Complete production shutdown first. Record actual empty production/browser/
simulator groups, clean exits and no forced shutdown before moving stopped
state. The measurement launcher is still live while it makes the checkpoint;
its final stop confirmation is a later, separate prerequisite for grading.
Move the complete stopped server state, Spool, simulator source/events/answers,
query/management/UI logs and resource observations outside launcher-owned
automatic-cleanup scratch on the **same filesystem**. Refuse existing
destinations, links, mounts, changed identity and cross-device fallback copies.
Record source/destination device and inode, method and complete member manifest.
Preserve partial or uncertain checkpoint state; failure grants no deletion.

The measurement receipt is `pending_acceptance`; the closed runtime receipt is
`closed_pending_acceptance`. Neither is rewritten into a pass by grading.
After the complete measurement launcher has stopped, run frozen Spool inspection
(existing 60-second/128-MiB bound), server dump and unchanged oracle grading in
separate default 1,800-second contained cells. Admission requires the exact
measurement unit, command, cgroup, 7,500-second allowance, matching exits and
confirmed empty stop. Bind actual immutable measurement/runtime receipt hashes,
candidate, canonical package receipts, source, helper, console, freeze manifest
and every complete raw input to the owner-admitted expected tuple. Candidate
names or a self-consistent forged tuple are not independent authority.

Write a fresh grading receipt at a separate exclusive path, binding those input
hashes and the graders' own command/exit/resource/stop receipts. Missing stop,
changed raw bytes, changed source/package/helper/manifest, mutated pending
state or incomplete input blocks grading and acceptance. Measurement completion
alone remains pending, even if all descriptive measurements look favorable.

## Whole-parent containment and preservation

Require actual kernel readback of the **whole parent**, not only child groups:
`memory.high=5368709120`, `memory.max=6442450944`, `memory.swap.max=0`,
`pids.max=1536`. Record parent and all existing subgroups at admission, sampling,
phase boundaries and final stop, including current/peak charge, memory events,
swap, current/peak tasks, CPU/I/O accounting and population. OOM/oom_kill,
nonzero swap, cap mismatch, missing parent observations, remaining population or
unconfirmed stop invalidates the run. Outer requested 16/20-GiB settings cannot
substitute for the tightened actual readback. Existing server/companion and
browser/supervisor/edge subgroup limits remain unchanged; parent charge does not
replace the server-alone RSS gate.

Keep 5 GiB combined live data, 1 MiB command/evidence allowance and a fresh strict
namespace census/reservation below the owner's 95 GB admission stop and 100 GB
ceiling. Increased raw volume is not assumed to fit. Archive reservation is the
actual complete stopped tree's logical bytes plus 1 GiB. Require exact private
archive/member hashes and verified preservation before any owned removal.
Reject a mount at or below the owned root using fully decoded mountinfo before
inventory and again immediately before the first unlink. Same-device bind
mounts are refused; `st_dev` equality is insufficient. Require process/ownership/
identity checks, and preserve every original failure until verified archival.

## Prospective executable controls and integration boundary

[`test_soak_exposure.py`](../../../tools/qualification/test_soak_exposure.py)
registers literal positive/negative vectors before a `soak_exposure.py` mechanism
exists. Its initial availability assertion is an honest missing-mechanism red,
not a product defect; mechanism-dependent controls are explicitly skipped until
available. Direct existing-source controls retain the actual 5,460-second and
missing-parent counterexamples. No shortened timer establishes two hours.

The minimal shared mechanism contract is: `cohort_ns(timestamp, began)`,
`validate_exposure(sim, interval)`, `remaining_ns(phase, launcher_begin,
phase_begin, now)`, `validate_phase_headroom(measurements)`,
`validate_checkpoint(measurement, runtime, outer, expected)` and
`validate_parent(groups, stopped=False)`. These consume raw observations and
caller-supplied owner-admitted expectations; they do not authenticate those
expectations. The literal test records define field names and refusal reasons.
Checkpoint arguments wrap caller-read JSON as
`{"sha256": actual_file_hash, "value": parsed}`; the observed hash is not a
receipt's self-asserted field. Expected hashes come from root's separately
pinned admission record, never the untrusted input.
`checkpoint_tree(source, destination, processes_stopped=True)` must use rename,
retain the stopped inode and refuse cross-device copying. This boolean is not
stop evidence: native callers must derive it from bound actual group observations.
The existing preservation inventory/cleanup entry points receive decoded mount
guards before inventory and immediately before unlink, with zero deletions in
the injected same-device mount controls. R2 historical meaning is preserved.

Before smoke/full admission root must separately verify real call-site binding:
both cohort consumers, simulator command/raw evidence and uninterrupted probe
lifetime; phase handling of progressive blocking I/O; measured headroom;
pre-checkpoint production stop; post-checkpoint outer stop; immutable exclusive
grading output; actual parent sampling/readback; and both live mount checks.
The pure controls alone cannot establish these integration properties. Retain
all existing gate/custody/freeze/archive controls and inject changed/missing
bindings into the candidate-specific adapters. Freeze every loaded and archive
dependency, exact clean source, package/helper/console and plan/driver hashes
before any new smoke. Full admission additionally requires fresh candidate
#41, #42, #45, #43 and #46 evidence, a complete admitted smoke and S1 headroom.

### Simulator witness location

The simulator requires `--begin-witness` to be a child of its exact `--out`
directory. R3 uses `raw/sim/begin.json`; the stopped checkpoint reader must
reparse that same file and join its original path to the recorded helper argv
and observer receipt. A witness beside `raw/sim` is invalid. Keep the actual
failed S1 attempt from 2026-10-16 (helper refusal before publication) and cover
the caller's output/witness relationship and checkpoint path substitution with
regressions. This path correction changes no exposure, phase, syscall, identity,
resource, oracle or acceptance requirement; repeat S1 in a fresh namespace.

Root alone runs the prospective control cell; **not run at registration**:

```sh
python3 -B tools/resource_group.py -- python3 -B -m unittest discover \
  -s tools/qualification -p 'test_soak_exposure.py'
```

The unchanged gate, companion, freeze and archive suites remain additional
checks. Publish actual commands, failures, pins and results in the wiki. One
finite same-host loopback trial does not qualify deployment, default-scale
retention, physical power loss or universal long-term stability.
