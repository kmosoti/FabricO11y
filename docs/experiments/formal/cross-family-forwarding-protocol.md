# Registered finite cross-family forwarding cells

Status: revision 1 registered 2026-10-10 before execution. This implements the
four remote cells in the [release plan](../../milestones/release-readiness.md),
separately from the six main profiles, three longer outages and R2 soak. No
result is recorded here. Prior candidate or historical results are not reused.

## Inputs and placement

Freeze final Debian/RPM SHA-256, source bundle/manifest, exact console hashes,
native binaries, independently specified producers, Python delivery/query
oracles, and uninstalled spool_dump/server_dump helpers compiled from the same
source/sysroot. Preserve package identity: helpers never replace installed
executables. Pin Debian 13 and Fedora 44 official base image hashes as in the
[installation protocol](installation-release-protocol.md). Run four cells with
server/edge Debian 13/Debian 13, Debian 13/Fedora 44, Fedora 44/Debian 13 and
Fedora 44/Fedora 44; use seed 0xA11FA001 for these additional finite remote cells.

Each cell has two distinct booted OS guests, each with its own kernel, boot ID,
filesystem, services and cgroup tree. Traffic crosses the edge's virtual NIC,
QEMU network and the server guest's forwarded TCP listener. Separate identities
on host loopback do not count as this placement. Preserve network commands,
certificate SANs, CA validation, guest addresses and forwarding ports. Both
guests use the exact installed package; Fedora stays SELinux enforcing.

The explicit serve-legacy migration fixture may provision and query these
transport/custody cells. Edge forwarding still uses its native authenticated
TLS client and node bearer token. This fixture does not satisfy production
passkey/scoped-access acceptance; that remains a separate exact-package gate.

## Workload and accounting

After 15 s warmup, measure 120 s. Offer native logs at 2 records/s, 512-byte bodies,
alternating repetitive and the unchanged seeded high-entropy fixture. Record
offered file offsets, wall/monotonic observation timestamps and exact bodies
independently. Send 10 unique traces/s with 3 independently frozen spans per trace
to the edge's real loopback intake; freeze IDs, parents, timestamps and values
before submission. Count offered/admitted/rejected/uncertain exports separately.

A separate controlled producer identity on the edge guest supplies exactly 32
metric points every 15 s and durable source/ACK records. Encode one cumulative
monotonic OTLP sum `cross.counter` with 32 attribute-separated points
(`point_id` strings `0` through `31`), integer value `k × (p + 1)` at
15-second tick `k` for point `p`, one fixed initial start timestamp, and
independently recorded observation timestamps. This controlled schema/formula
is separate from native host counters. Its custody is separate from the native Spindle. Account
native host metrics and diagnostics, and the server companion, independently;
host-dependent point counts cannot be relabeled as 32. Missing controlled points leaves
the registered cell incomplete.

After the measured interval, interrupt only the edge-to-server data listener
for 60 s with an owned guest firewall rule; keep SSH and the server available.
Continue offered sources, record all refusals/retries/backlog, then remove the
rule and stop offers. Drain all admitted/committed work within 120 s. Preserve the
interruption/reconnection clocks and actual firewall counters. This is not one
of the separately required 30 min outage trials.

## Timing and decisions

Enable bounded native timing events. Join node ID/generation/sequence to exact
source bytes and send hashes. Capture collection start, source admission before
append, post-sync Spool commit, each actual send and each answer separately.
The operational event is sampled after durable append returns; it does not
claim the marker syscall's exact finish time. Trace producer intake intervals
and native Spool admission are separate observations. A valid durable ACK must
also satisfy exact custody/recovery checks; a printed answer alone is insufficient.

Use same-boot Linux CLOCK_MONOTONIC for producer/native joins, record boot IDs,
and retain each clock sample's before/after bracket. For query visibility, sample
the edge clock before issuing the host query and again after its answer: report
the resulting conservative visibility interval and SSH/correlation uncertainty.
Never subtract unsynchronized wall clocks or invent a point estimate. Missing
clock samples, lost timing events, unjoined sources and censored attempts are
explicit incomplete evidence. Record wall/monotonic correlation probes and
discontinuities; do not silently ignore them.

Steady B3 applies to admitted work observed during the 120 s measured interval:
creation-to-valid-durable-ACK p99 upper bound ≤ 1 s; observation-to-query-visibility
p99 upper bound ≤ 5 s; healthy configuration apply ≤ 30 s. Include waiting/retries in
creation-to-ACK. Report successful-attempt elapsed separately. Use nearest-rank
p99, publish population/counts and censoring, and never exclude a late item
because it crossed the measured end. Freeze a backlog rule before execution:
sample every second; compare the arithmetic mean of committed-minus-ACKed
counts and bytes in the first and last 30 s measured windows. A larger last
window mean in either coordinate fails the growth gate. Publish all samples;
missing samples make the rule incomplete. End-of-drain backlog must be zero.

### Prospective timing-population clarification and backlog revision

Report five distinct populations for subsequent cells,
without substituting one population's starting boundary for another's:

| Population | Independently observable starting boundary |
| --- | --- |
| Offered edge logs and traces | The producer's observation/creation timestamp, joined by source offsets or span identity |
| Controlled metrics | The independent producer's creation timestamp and exact Batch identity |
| Native host metrics | Native collection-start timestamp; earlier physical counter changes are not measured |
| Companion diagnostics | Native source acceptance before durable Spool append; diagnostic creation before collection is not measured |
| Edge diagnostics | Native source acceptance before durable Spool append; diagnostic creation before collection is not measured |

Apply ACK p99 ≤ 1 second and visibility p99 ≤ 5 seconds separately to each
population using its stated boundary and conservative same-boot clock brackets.
The companion result is acceptance-to-ACK/visibility, never diagnostic-creation
latency. Pre-Spool diagnostic logging remains best-effort under the existing
self-observation contract. Include every accepted companion Batch in custody,
query, backlog, drain and resource accounting; this clarification excludes no
accepted work or failed attempt. Missing required joins still fail admission.

The edge-diagnostic population is registered on 2026-10-22 after disposable
`split-debian-fedora-smoke-06` failed with `unexpected unregistered edge log
source`. Its original exit 2, source bytes and shutdown receipts remain failed
evidence. The retained edge Spool contains 390 offered logs and 36 diagnostics
from `/var/lib/fabrico11y/node/spool/diagnostics/spindle.log`. The native CLI
already opens that local diagnostic source; the four-population adapter had
incorrectly required every edge log to use the offered file. The
[counterexample](fixtures/cross-edge-diagnostics.json) pins this distinction.

For the existing fixed edge configuration, admit only that exact diagnostic
path in addition to `/var/log/fabric-cross/source.log`. Preserve offered-log
offset/body joins, reject every other path, and grade diagnostics separately
using their actual native acceptance/ACK brackets. Require a nonempty measured
population and the same ACK/visibility limits. Include these rows in the
unchanged custody, query, backlog, drain and resource accounting. Do not merge
them into offered observations or call acceptance latency creation latency.
Repeat the affected controls and a fresh full smoke before placement admission;
do not promote the failed run by retrospective grading.

New cross-family cells explicitly select
[backlog revision 2](service-backlog-protocol-r2.md): full interval-union clearing
within 5 seconds and positive empty time in each measured 5-second window.

### Producer and observation repair after the retained split smoke

Registered on 2026-10-22 before a fresh run. Diagnostic replay of failed
`split-debian-fedora-smoke-06` is not acceptance. All 652 exports with HTTP 200
are retained; 406 missing exports belong to uncertain attempts. The producer's
80 ms HTTP timeout is shorter than the native loop's 100 ms idle wake. Its
reconnections can consume the receiver's bounded connection permits. Preserve
the original uncertainty, source bytes and command exit; do not lower the
required 1950 exports or 5850 spans to the admitted subset.

Keep exactly 390 logs, 1950 trace exports with three spans each, and thirteen
32-point controlled metric Batches. Keep 15 seconds of warmup, 120 seconds of
measured offered work and 60 seconds of offered work during data interruption.
The existing producer may use eight HTTP workers, a 1950-export queue and
30-second request deadlines bounded by its final drain deadline. Record every
offer before enqueue and every original admitted/uncertain outcome. Queue
overflow or an unfinished worker is a failed producer, never permission to
drop work, retry an uncertain export or change its identity. Keep all workload
and independent delivery/query expectations.

Insert a fixed five-second guard with no new offers after the measured interval
and before the full sixty-second interruption. Subsequent offered coordinates
shift by exactly five seconds: 195 seconds of actual offered work occupy 200
elapsed seconds. This lets measured work use its existing five-second clearing
allowance before the intentional fault; it does not enlarge any latency or
clearing threshold. Preserve all late measured items and their actual ACKs.
The interruption covers the full final sixty seconds of offered work. Retain
the 120-second drain allowance after offers end and the outer resource limits.
The sampler's 330-sample horizon and measured phase remain unchanged. This
guard does not change any separately registered thirty-minute outage or soak.

The original complete per-name visibility sweeps and fixed query snapshots
remain. Add one bounded, independent observation loop during the existing
15-to-145-second visibility interval. It reads logs, spans and all metric names
for each native identity, plus controlled metrics, with the existing bounded
query envelope and independent oracle. Bracket each answer in its own guest
clock; retain errors, original statuses, all answers and completion. Queries
with a stale or missing bracket still fail. Joining observation streams uses
the earliest actually completed same-boot visibility witness, independently of
serialization order; no returned row or failed oracle decision is discarded.
The observer must finish before grading. It adds observations without removing
the original query work. Use the existing named metric query shapes: the legacy
admin API requires a metric name and gains no wildcard or new public format.
Up to eight HTTP calls may execute concurrently inside each fresh per-node
clock envelope. Every returned answer retains that conservative guest-clock
bracket; it is not an individual point estimate. Bound each sweep by the fixed
nineteen native or one controlled query shapes, preserve every error and wait
for all owned query workers before completion. Representative controls must expose the original
short-deadline, grouped-bracket and out-of-order-witness defects. Run the fresh
full smoke under the unchanged split limits before placement admission.

Retain the producer and outage units with `RemainAfterExit=yes` too. Completion
requires the actual loaded, active/exited unit, original exit code/status,
successful result and matching guest boot. A default inactive status from an
unloaded unit is not an original command receipt. Save the full status at each
completion probe; running units may continue only within the existing deadline.
Failed, absent or unfinished units refuse the cell. Reuse the literal manager
states registered by the [monitor diagnostic](cross-monitor-lifetime-protocol.md)
for success, failure and unloaded controls. Keep the source/end ledgers and all
independent oracles as additional requirements, and stop retained owned units
during cleanup after preserving completion evidence.

The full interruption is measured from the completed rule-installation bracket
to the start of rule removal, in the same admitted edge boot. Rule construction
time cannot consume the sixty-second hold. Require a conservative duration
lower bound of at least sixty seconds as well as nonzero firewall counters;
an absent, shorter or cross-boot interval fails. A deterministic delayed-rule
control must reject the former schedule that subtracted installation cost.

The queue bound and named concurrent observations supersede the initial repair
after failed `split-debian-fedora-smoke-07`. The observer's unsupported unnamed
metric request correctly received HTTP 400. The retained producer exited 1
after 294 overflows of its 256-entry queue during the outage; all 1656 admitted
exports are retained. A disposable clone recovered the failure ledger while
the original disk hash remained unchanged. The
[counterexample](fixtures/cross-producer-overflow.json) pins these identities.
The queue is now bounded by the entire fixed 1950-offer population so transport
waiting cannot silently remove offered work during the sixty-second fault.
The producer's 128 MiB cap, guest/host caps, all counts, deadlines and numerical
gates remain fixed; budget admission still requires a fresh full smoke. Keep a
representative 256-entry overflow control and reject an extra offer beyond the
finite bound. Persist observer errors immediately, including original HTTP
status, and reject incomplete observation rather than waiting to discover an
unsupported query after the campaign. This registration changes no product API.

Retain the original one-second samples and first/last means as diagnostics.
The original rule above remains the revision-1 specification; no earlier result
is regraded and no historical fleet, outage or soak rule changes.

Fresh stopped-process Spool/server dumps feed unchanged independent delivery
and query oracles. Check exact logs, metric values/rates, trace IDs/parents,
pagination/completeness before interruption, after reconnection and restart.
All accepted sources and companion traffic remain in custody accounting.
Inject dropped recovered data and a mismatched query row: each oracle must
reject its named defect. Missing retained native source bytes cannot be filled
from server output and remains incomplete.

## Containment and receipts

Use the mounted DATA drive and resource launcher. One cell at a time: outer
8 GiB max/7 GiB high/no swap; two 2-vCPU TCG guests, server 4096 MiB and edge 1536 MiB;
6 GiB virtual disk each. Reserve both full disks plus 1 GiB overhead below the 95 GB
admission stop before allocation. Do not shrink the official Fedora 5 GiB base.
Verify all actual limits before loading either guest and preserve source/artifact,
command/exit, clocks and sampled resources. Guest service caps remain unchanged.

B5 requires server RSS ≤ 2 GiB and real edge RSS ≤ 64 MiB on this workload, no OOM,
swap or unaccounted children. Record server/companion separately and combined,
edge, producers, guests and outer cgroup; CPU time, I/O, live storage and peaks.
TCG placement is a local OS/network acceptance measurement, not WAN or cloud
deployment qualification. Stop/reap both guests, remove owned firewall rules,
preserve failures/receipts and record cleanup before admitting the next cell.
