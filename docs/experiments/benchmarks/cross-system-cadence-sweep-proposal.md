# Native cadence and observer sweep proposal

Status: proposed; no build, execution or validation receipt exists yet. The root
coordinator must register this design before measurement. Historical
`prefix-load-03` remains diagnostic and `prefix-load-04` remains mixed: its
96-ACK p99 was the maximum, and its 10 ms observer often could not resolve
publication versus reclamation. This sweep asks whether the observer itself
changes finite intake and sealing behavior across arrival cadences.

The [new native example](../../../crates/fabric-server/examples/cadence_load_probe.rs)
derives custody and quiet query construction from `prefix_load_probe.rs`.
The [new driver](../../../tools/bench/labs/cross_system/cadence_sweep.py) reuses
the existing independent query oracle and archive helpers without changing them.
Every cell calls actual `sealer::pass`; historical scheduler comparison is outside
this sweep. Store grouping, retention, format, sync order and semantic oracles
stay fixed. There are no fault injections or destructive runs.

Run 54 cells: balanced with three workers, naturally dense-middle with three
workers, and the same dense-middle with one worker; each at 0, 2 and 5 ms
cadence, with quiet, events and polled observers, for fresh markers
2703204361 and 2703204362. Markers change payload identity; they are not random
skew distributions. Reverse observer order in the second replication. Each
fixture seeds three actual sealed journals with 128 Batches each. Dense middle
has 128 log rows per Batch; other files have two. Extend one legitimate OTLP
log body so every encoded Batch is exactly 8192 bytes. Group framing/labels
remain real and their actual file sizes are recorded.

Offer 512 further Batches: the same 128 strands in sequence rounds 4–7.
Targets use one absolute schedule clock; Intake submission does not await ACK.
At 0 ms, yield to ACK observers every 32 offers; this is a finite burst.
Retain actual target, offer and reply timestamps and lateness. Cadence2 coincides
with `CommitMode::GROUPED.quiet`, so timer jitter and group alignment are potential
causes rather than controlled facts. This is finite paced intake, not sustained
service capacity; only one initial-backlog sealer pass runs.

Use one phase-probe build. Quiet leaves the observer uninstalled and has no poll
thread. Events installs the existing bounded frame probe ledger without polling.
Polled adds the previous 2 ms filesystem/process observer. The callback supplies
zero-valued allocation/CPU samples, which are explicitly unavailable measurements;
timestamps, allocation, mutex and ledger work still cost resources. Charge setup
time and before/after RSS separately. Timed process CPU uses Linux ticks; HWM
includes fixture startup, and sampled RSS exists only in polled cells.

Existing `bounded_segment_build` hooks have thread identities but no journal
labels. Endpoints bound builder return, not rename time. `frame_directory_sync`
also occurs during appends and cannot identify checkpoint duration. Report
checkpoint duration as unmeasured. Without polling, bound all three initial-file
releases by the earliest successful builder return and pass completion (quiet:
zero through pass completion). Polled filename observations give interval bounds.
Do not compare these unlike uncertainty widths as measured gains. ACK p50/p95/p99,
max, finite offered/accepted rates, unanswered-offer debt, CPU and RSS accompany
initial-backlog byte-time. Separate ACK populations offered during/after sealing.

Every cell must preserve exact Batch replay, reopened retry/conflict behavior,
and eight unchanged pre/post paginated Scan/Walk oracle chains. Missing/duplicate
rows and telemetry defects must be rejected. Stop on disagreement or bounds failure.
Allow 900 seconds internally, 20 seconds/cell, state64MiB and whole raw128MiB.
Success evidence ≤100MiB: complete ACK/hook/poll telemetry, verdicts and whole-file
hashes; one exact-readback representative full fixture ≤24MiB compressed.
Other successes are summaries, not full archives. Preserve the entire failing
owned raw tree separately under128MiB, archive/readback before cleanup, and retain
the original if preservation fails. Root reserves228MiB within operations384MiB.
No work is admitted outside the coordinator and resource launcher.
