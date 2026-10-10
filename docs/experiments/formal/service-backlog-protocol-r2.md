# Service backlog measurement, revision 2

Registered before revision-2 local main or cross-family cells. This changes only
the finite backlog decision for B3 in the [release plan](../../milestones/release-readiness.md).
The [local protocol](release-service-query-protocol.md) and
[cross-family protocol](cross-family-forwarding-protocol.md) otherwise retain their
workloads, populations, clocks, latency, custody, resource and cleanup gates.
Historical fleet, stress, outage and soak protocols are unchanged.

## Reason and claim

Revision 1 requires the final window's sampled pending count and bytes to be no
larger than the first window's. A periodic source with constant, short ACK latency
can fail that rule solely because polling intersects different in-flight Batches.
Exact time integration removes that aliasing but does not make two finite noisy
windows have identical means. Nonincreasing sample means therefore do not
characterize the intended operational property: accepted work must repeatedly
drain instead of accumulating indefinitely.

The first local dev trial remains **failed under revision 1**. Its receipt and
phase-sensitivity diagnostic must be retained. It cannot be regraded into a
revision-2 pass. Revision 2 requires fresh execution of every required cell and
an explicit `clearing-v2` decision identifier. This is a different finite claim,
not evidence of stationary occupancy or an asymptotic queue-stability proof.

## Decision

For each separately reported population, construct every accepted/created-to-first-
valid-ACK interval from the complete source/attempt ledger. Include warmup and
drain records; do not clip intervals before finding connected busy periods. Merge
overlapping or exactly adjacent intervals. A zero-length ACK interval consumes
no queue time. Any missing ACK, incomplete timing population, invalid timestamp,
missing clock mapping or unmatched source fails admission as before.

For every connected busy period intersecting the measured interval, require its
**full duration to be at most 5 seconds**. Additionally, every consecutive
5-second measured window must contain a positive-duration interval with no
outstanding work. This rejects the exactly aligned 5-second busy period that
would otherwise occupy an entire window. The 5-second horizon uses the existing
query-visibility service budget; it is not fitted to a candidate's queue size.
The separate per-population ACK p99 limit remains **1 second**, visibility p99
remains **5 seconds**, and final drain remains **zero unacknowledged Batches**.

Use source creation/acceptance timestamps, not send timestamps. For cross-guest
clock brackets, use conservative interval endpoints: earliest admissible creation
and latest admissible ACK. Missing bounds cannot be replaced with wall-clock
subtraction. The existing source-specific timing admission still applies.

Report the original sampled first/last means, exact time-weighted first/last
means, maximum outstanding count/bytes, longest busy period, queue-empty fraction
and every reconstructed sample. The original inequality remains a diagnostic
with its own true/false result. Rising peaks or mean occupancy can satisfy revision
2 when all work still clears within the stated bound; report that limitation
explicitly rather than claiming nonincreasing occupancy.

## Independent controls and evidence

Keep the revision-1 checker and its expected failures unchanged. Before fresh
revision-2 trials, independently constructed interval fixtures must show:

- phase-shifted constant-latency traffic changes sampled verdicts but not clearing;
- a continuous busy chain longer than 5 seconds fails even when every individual
  ACK is below 1 second and all work eventually drains;
- a busy chain starting in warmup or ending during drain retains its full length;
- adjacent intervals have no invented idle gap, and a whole-window busy interval
  fails even at the exact duration boundary;
- censored ACKs, negative time, omitted native timing and changed source bytes fail;
- increasing peaks with repeated clearing are accepted only as the documented
  bounded-debt claim, with increasing means/peaks still visible in diagnostics.

Freeze the checker, command and protocol identity with the candidate receipt.
The same cell names require new run IDs; preserve all prior failures. A passing
subset, altered sampler phase or retrospective calculation cannot close B3.
