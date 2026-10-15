# R3 bounded lifecycle guardian amendment

Prospective registration, 2026-10-15. This supersedes only the final observation
topology in [parent stop](soak-parent-stop-protocol.md), not the ten independent
soak gates, exposure, phase or resource ceilings. Native results are not implied.

The old topology requires readable empty outer-service fields after its final
process exits. Two retained counterexamples show that systemd removes that
kernel cgroup despite RemainAfterExit; all thirty preopened field descriptors
then return ENODEV. No empty path or failed read counts as an observation.

`resource_group.py --observe-workload-stop --delegate --runtime-seconds 7500`
keeps one lifecycle guardian inside the original exact 5/6 GiB, tasks1536,
zero-swap service. It creates a dedicated workload root with the same caps and
CPU ceiling. The inside wrapper and every project process execute there. Only
the guardian may execute outside it; it performs lifecycle/provenance work, no
server, browser, simulator, oracle or test. Default launcher behavior is unchanged.

Three separately named claims are required: (1) after the actual independently
identified wrapper exits and is reaped, every workload subgroup is empty and
all required kernel fields are read against the held workload inode; (2) while
only the exact guardian remains, actual aggregate outer fields and complete PID
inventory are read against the held outer inode; (3) the outside launcher reaps
the guardian/transport and independently confirms whole-service stop. Outer
accounting is explicitly before guardian exit; no post-guardian peak is claimed.
The same outer aggregate cap bounds both branches throughout. A gate requiring
post-last-control-plane outer accounting remains unavailable under this topology.

Bind real unit activation, service MainPID to guardian PID/start/boot identity,
held outer/workload device/inode, actual wrapper PID/start/boot identity, real
wait exit and completed command/unit/workload receipt. Keep raw field bytes and
PID inventories. Reject changed identity, nonzero OOM/swap, missing fields,
populated workload, incomplete/unexited wrapper, any task outside workload other
than the guardian, failed service, exhausted deadline or unconfirmed outer stop.
Failure preserves evidence and stops the entire owned service; forced cleanup
does not convert it into successful observation. All phases still share the
single activation-based 7500-second deadline.

Before acceptance run source controls plus finite native cases through this
opt-in launcher, fresh DATA output/reserve and independently pinned driver:

| Case | Actual action | Required result |
| --- | --- | --- |
| positive | finite command returns0; archive real identity/caps | empty workload, guardian-only outer, complete fields, launcher0 and whole stop |
| unexited | publish otherwise completed wrapper receipt while wrapper remains alive | no snapshot accepted before real wait; bounded failure/whole stop |
| populated | leave owned idle descendant in workload after wrapper returns | reject actual populated workload; whole stop |
| wrong-identity | replace empty workload pathname after its held FD was acquired | reject inode mismatch; whole stop |
| escaped | owned idle task moves into guardian control leaf | reject actual extra outside PID even if workload empty; whole stop |

The two control-plane faults use the explicit `--guardian-control unexited` or
`--guardian-control wrong-identity` option before `--delegate`. The former caps
the actual wrapper wait at three seconds; the latter replaces the empty root
pathname after wrapper reap. Both permanently mark the observation control-only.
These are finite negative-control mechanisms, never campaign admission modes.

Source controls inject every individual missing field, dirty field, wrong cap,
wrapper/guardian identity and inventory disagreement. Native outcomes and exact
commands/pins are recorded before any S1; these controls qualify no product.

## Runtime and stopped-evidence mapping

R3 activation now requires this topology and an inherited held workload-root
descriptor. Bind actual unit/guardian via systemd and `/proc`, use the workload
root for delegated project groups, and sample the actual outer aggregate. Final
offline joins reparse the guardian's raw fields/inventory against admitted
outer/workload inodes, all runtime subgroup caps, actual command/wrapper identity,
and the independently stopped outer receipt. A control-only record is refused.
The historical removed-parent topology remains a counterexample, not fallback.

Use unchanged NativeTrace source3b3794abbd539085f02585356a035d5b37ec3a04.
Launch its held ELF directly after authenticating the caller's actual held
supervisor cgroup; inherit exact CPUs2–3 using a temporarily narrowed calling
thread affinity, restore it in finally and check actual target affinity/cgroup.
Its live trace directory starts under the result tree, then receives a stopped
same-filesystem rename to `native-trace`, included in the immutable checkpoint
manifest. The raw begin file retains its inode through the separate raw rename.
Offline `NativeTrace.validate_stopped` reparses actual trace/receipt/begin bytes
before ten-gate evaluation. Observer facts carry no invented exit fields; the
observer's own raw receipt must bind target/tracer exit0 and actual identities.
The frozen harness inventory includes the guardian and native observer modules;
these harness changes do not refreeze product packages.

## Actual terminal closeout boundary

The closeout phase includes the final runtime receipt I/O, guardian snapshot,
guardian receipt I/O and exit, and actual whole-service stop. A runtime phase
record written before its own final I/O is an intermediate observation, never
the complete closeout duration. The outside launcher records a CLOCK_MONOTONIC
stop-before/after bracket and actual kernel boot ID; stop-after is captured once
the owned service's stop is independently confirmed. Subsequent outer receipt
writing and analysis are outside that terminal measurement, with no recursive
receipt layer. The stopped evidence join requires matching host boot/clock,
guardian-after ≤ stop-before ≤ stop-after and stop-after within the same global
7500 seconds and closeout-start +20 seconds. S1's actual closeout maximum is
conservatively stop-after minus closeout-start, including all final I/O/stop
work; the unchanged25% margin applies to it. Missing brackets, another boot,
forged elapsed or20 seconds +1ns refuse admission/acceptance. Existing phase
ceilings and all ten gates remain unchanged.
