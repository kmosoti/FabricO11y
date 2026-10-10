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

Registered before the first cross-family cell: report four distinct populations,
without substituting one population's starting boundary for another's:

| Population | Independently observable starting boundary |
| --- | --- |
| Offered edge logs and traces | The producer's observation/creation timestamp, joined by source offsets or span identity |
| Controlled metrics | The independent producer's creation timestamp and exact Batch identity |
| Native host metrics | Native collection-start timestamp; earlier physical counter changes are not measured |
| Companion diagnostics | Native source acceptance before durable Spool append; diagnostic creation before collection is not measured |

Apply ACK p99 ≤ 1 second and visibility p99 ≤ 5 seconds separately to each
population using its stated boundary and conservative same-boot clock brackets.
The companion result is acceptance-to-ACK/visibility, never diagnostic-creation
latency. Pre-Spool diagnostic logging remains best-effort under the existing
self-observation contract. Include every accepted companion Batch in custody,
query, backlog, drain and resource accounting; this clarification excludes no
accepted work or failed attempt. Missing required joins still fail admission.

New cross-family cells explicitly select
[backlog revision 2](service-backlog-protocol-r2.md): full interval-union clearing
within 5 seconds and positive empty time in each measured 5-second window.
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
