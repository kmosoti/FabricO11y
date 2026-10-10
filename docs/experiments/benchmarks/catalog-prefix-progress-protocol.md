# Reclaim a published prefix inside a worker group

Prospective D1/O1 mechanism investigation, 2026-10-08, following the
[cross-system readiness slice](catalog-cross-system-readiness-protocol.md).
The six existing sealer controls exited zero before this registration.

## Hypothesis and adversary

The current scheduler joins every worker in a group before reclaiming any
journal. A slow later worker therefore retains an earlier durable prefix.
Hypothesis: join handles in oldest-label order and reclaim after each successful
join. This removes the group barrier without adding a completion channel or
another queue. Waiting for the oldest missing label is necessary for prefix
reclamation; receiving later completions sooner cannot make that prefix eligible.

H0: this rearrangement does not permit earlier prefix progress in the controlled
schedule. The discriminating regression uses one group of three builders:
labels 1 and 3 finish, label 2 waits at a barrier. Require reclaim of label 1
before releasing label 2, never label 3 across the hole, then final order 1,2,3.
Release blocked workers even when the assertion fails. The unchanged regression
must fail on the original scheduler and pass on the candidate. Watchdog timeouts
bound the test; they are not benchmark latency thresholds.

A second adversarial schedule injects reclaim failure while a sibling is still
building. The sealer must retain and join all started workers before returning,
return the reclaim error and start no later group. Existing build panic/failure,
retry and exact checkpoint-custody tests remain unchanged. Errors still select
the first failed label deterministically; reclaim error retains precedence.
No checkpoint, sync, publication, wire, query or retention contract changes.

## Execution and resource accounting

Three serialized jobs use `python3 -B tools/resource_group.py -- python3 -B
tools/bench/labs/catalog/prefix_progress.py PHASE`, where PHASE is `baseline`,
`candidate` or `checks`. The baseline and candidate each receive at most 120
seconds in the recovery stage. Baseline runs the new discriminating regression
and preserves its expected nonzero result. Candidate runs all `sealer::tests`
plus the existing `history` integration target with the independent query oracle.
Checks receives at most 200 seconds and runs the unchanged fast profile and
manual documentation checks. Stop a phase on an unexpected exit or deadline.

Prospective resource-only transfer: move 200 seconds of unused preparation
allowance to verification (preparation 3540 -> 3340; verification 3660 -> 3860).
The total 86400-second and frontier 14400-second limits are unchanged. This is
not a reset of historical consumption or a change to a correctness gate.
The standalone driver records actual consumption in the existing coordinator
ledger and checks these limits under its exclusive lock before each phase.

Reserve 192 KiB evidence per phase; admission uses the existing aggregate and
query caps. Reuse the immutable completed-fast source archive after checking
every current Rust/Cargo/oracle file except the explicitly varied `sealer.rs`.
Save that entire current file, the new driver and protocol with gzip readback.
Record exact commands/exits, source hashes, cgroup memory/CPU/I/O/events, elapsed
time and cleanup. Reuse build cache on the mounted data drive; use only fresh
launcher-owned scratch. Keep memory high/max 16/20 GiB, swap zero, at least
16 GiB disk free and the unchanged 1800-second outer deadline.

Preserve expected baseline failure output before removing owned test scratch.
Unexpected failures retain their full owned scratch under the launcher's failure
policy; do not call them successful or delete them to fit the evidence allowance.
No upstream builds, service benchmarks, destructive fault tests or qualification
are part of these commands. Their remaining budgets are not pre-authorized by
this registration.

## Decision and limits

Admit the small scheduling change only if the original is rejected by the
unchanged regression, every candidate control passes, the independent history
oracle accepts and final checks succeed. Update source comments and architecture
documentation with the actual ownership boundary. Reject any proposed channel
whose complexity buys no earlier reclaim under oldest-prefix ordering.

This establishes a finite scheduling property. It does not establish lower RSS,
ACK latency or higher sustainable throughput: checkpoint work still competes
with ingest on the commit thread. Measure those interactions before claiming a
performance gain. Source inspiration does not establish algorithmic novelty.
