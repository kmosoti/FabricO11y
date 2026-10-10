# Bounded service residency and storage recovery research

Prospective registration, 2026-10-09, following the owner's revised
[engineering goal](../../research/cross-system-performance-program.md#revised-engineering-goal).
This is a finite diagnostic round, not the registered long soak, C5 matrix or
target-host qualification. No production algorithm, default or oracle changes
are nominated. The two tracks below exhaust this round's experimental scope.

## Questions and competing explanations

S1 asks whether current native service memory settles after multiple publications
with consumers still active. H1: releasing journal/sealer work restores a stable
resident working set. H0: RSS remains high or grows because retained allocation,
tail/query state or unrelated owners dominate. Distinguish anon/file/kernel
cgroup memory, process RSS and allocation history; these observations cannot
identify exclusive allocation owners. Compare Scan and Walk descriptively on
the same offered shape; one ordered trial per plan cannot support a causal
speedup, winner or default promotion.

S2 asks whether current bounded storage preserves its source and recovers exact
files across injected write/read/sync/publication failures and repeated crashes.
H1: after removing the injected fault, cleanup and retry restore the same complete
manifest as a fault-free build. The adverse case is partial/published state or
interrupted recovery that makes a retry lose custody or accept a different file.
Injected ENOSPC/EIO and SIGKILL do not establish physical disk exhaustion or
power-loss durability.

## Fixed workloads and boundaries

S1 uses twenty real local Spindles, authenticated TLS, 900-byte source bodies,
15-second host metrics and the existing deterministic source construction
(seed prefix 2703163393). Offer one line per node every 100 ms for 60 seconds,
three for 60 seconds, then one for 60 seconds: 200/600/200 aggregate logs/s,
60,000 exact offered logs. Follow with 180 seconds without source writes while
the server, collectors and query consumers continue. Use one sealer, its normal
16 MiB run cap, 16 MiB journal files, 512 MiB journal/retention limits and 24-hour
retention. The smaller journal size exercises publication with bounded evidence.
No comparison with the older 1000/3000 logs/s small profile follows.

Run Scan then Walk in separate fresh state directories and processes. Reuse the
recent-log, absent-text and CPU-metric query rotation and exact live sentinels
from [consumer observation](dev-small-observation-protocol.md). Grade quiescent
complete chains with the unchanged Python query oracle over the entire replay;
reject missing/changed answer controls. Compare all offered log identities/body
hashes and all acknowledged Batch hashes with durable replay. Check whole-source
sequence continuity. Success-only Spool cycles measure accepted batches/s;
do not invent an append-attempt acceptance denominator.

Use separate attested cgroups: server high/max 768/1024 MiB, two CPU equivalents,
256 tasks; all Spindles together 384/512 MiB, two CPU equivalents, 512 tasks;
zero swap in both. All remain descendants of the 16/20 GiB laboratory envelope.
Require authenticated application readiness before load. One absolute cell
deadline of at most 750 seconds covers startup, workload, consumer joins,
shutdown, replay, validation and preservation; reserve time before each phase.
Limit replay output to 128 MiB and monitor the complete live tree at 384 MiB.
Full failed state must remain within the inherited 512 MiB raw / 192 MiB archive
envelope; if preservation fails, retain original scratch and report the failure.

Record one-second process and cgroup observations, CPU and IO, source lag,
ACK/query latency distributions, visibility clocks, successful Spool/ACK rates,
source/Spool backlog and journal/Segment progression. Record observed query
cadence/lateness and observer CPU. Full-chain grades are stronger evidence than
individual live sentinel observations and remain separately labeled.

S1 semantic gates: exact logs and ACK hashes, contiguous recovered sequences,
no unexpected gaps or changed live rows, complete independently graded final
chains and rejected defects, graceful process exits, verified child limits,
no OOM/swap, at least two observed Segment publications and no final pending
Batches/source backlog. Reuse the existing one-second collection-to-ACK and
ingestion p99, two-GiB server RSS, 64-MiB individual node RSS and five-ms clock
offset checks as diagnostic gates; explicit cgroup maxima additionally apply.
Report query latency without a new performance acceptance threshold.

Prospective residency discriminator: compare median sampled server RSS in the
first and final 60 seconds of the 180-second quiet period. Record H1-compatible
only if final <= first * 1.10 + 16 MiB, while publication/progress and semantic
gates hold. Report slope, peak and resident/file-cache behavior separately.
A finite plateau is not a proof of bounded long-term memory or the soak gate.

S2 reuses the 16 MiB steady generator and syscall interposer from
`tools/bench/labs/completion/faults.py`: fault-free and unmatched-path controls,
then its seventeen spill/merge/table/filter/manifest/read/sync/rename/partial-IO
and SIGKILL cases. Require an actual injection marker, expected failed exit,
unchanged original journal, exact complete retry manifest and no building
directories after retry. Add a repeated-recovery case: kill during spill, kill
again before publication on two retries, then recover without injection.
Keep per-case command/exit, file maps, source hash and retry result. Discard
successful temporary states only after checks; preserve exact failed states.
The classifier must reject missing injection, changed input, altered manifest
and leftover-state controls. This covers selected cuts, not every crash stage.

## Admission, reproducibility and completion

Reuse the native coordinator ledger and its unused allocation: no new time,
scratch or evidence allowance. Charge the goal-edit documentation check's
20.063403844833374 seconds before admission (it has its separate launcher
receipt). Maximum planned allocations: preparation/build 600 seconds, harness
controls 120, S1 800 each, S2 400, final checks/documentation 700 and closeout 60.
These are admission ceilings, not promises to consume them. Reserve at least
192 MiB in the memory category for each service/fault job and serialize all
heavy work through the resource launcher/coordinator. Keep 8 GiB aggregate
scratch, 16 GiB free reserve, zero swap, two build/test threads and 30-minute
outer deadlines. No remote workload is included.

Freeze source and dirty-tree identities, protocol, tools and binary hashes before
the first measured case; recheck binaries around runs. Sources and negative
controls remain independent of model reviews. Preserve failed attempts and
original exits; a corrected retry has a new record. A semantic failure stops
that track for investigation. New candidates require separate preregistration.

Complete this round when each admitted track has a graded result or a preserved
counterexample, documentation/appropriate checks have run, resource/time/evidence
accounting is reconciled and owned scratch/processes are cleaned after verified
preservation. Update current state and the research queue with results, remaining
qualification gaps and the smallest next discriminator. No release qualification
or completion of the broader engineering goal follows automatically.
