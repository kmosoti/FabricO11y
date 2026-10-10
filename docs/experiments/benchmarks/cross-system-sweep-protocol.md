# Cross-system matched workload sweep

Prospective registration, 2026-10-08, following the owner's request to repair
downloads, measure similar workloads broadly, preserve bugs and derive research
vectors. This extends the [continuation](cross-system-continuation-protocol.md);
its failures and decision rules remain unchanged. This is exploratory research,
not deployment qualification or authorization to change production defaults.

## Resource envelope

Allocate at most 7200 additional serialized execution seconds. Count historical
continuation time as well as this round against a 28800-second combined frontier
and the original 86400-second campaign. Each job remains below 1700 seconds and
uses the unchanged resource launcher: 16/20 GiB memory high/max, zero swap,
1800-second service deadline. Native jobs are serialized through the existing
readiness lock; Rust builds use two jobs. Scratch/builds use the mounted data
drive, with 8 GiB active scratch and 16 GiB free reserve. No remote workloads.

The new evidence root is `data/cross-system-sweep-01`, with 2 GiB additive
allocation: source 1024 MiB, memory 256 MiB, query 256 MiB, operations 384 MiB,
coordinator 128 MiB. Existing evidence remains untouched and separately charged.
Preserve failed state, commands, hashes, resource samples and exact exit status.
Passing generated fixtures may be removed only after semantic checks and a
complete file hash inventory; retain their deterministic generator and inputs.
This differs explicitly from the earlier round's full successful-state archive
policy. Failures retain complete state, or stop with scratch retained if the
archive allowance cannot fit. Never delete a failure to admit another cell.

## Source repair

Retry the three incomplete full downloads using the already verified immutable
revisions: ClickHouse `47907285810e618994aa703c31beb0edfc5d2271`, Vector
`47e05c4749b6b1af460af521279dc4681a912bb3`, FoundationDB
`daae46ce69b1fdddefdbb985e03e5a12576b3077`. The two old 404 revisions remain
historical failures. ClickHouse's old 128 MiB cap remains recorded; this retry
allows 512 MiB compressed per repository, 4 GiB decoded inventory, 16 MiB
selected source and 300 seconds network time. Stop at the first failure.
Retain complete compressed archives on success in addition to streamed regular
member hashes, selected source, licenses and exact selected-source readback.
Reserve enough source allocation for the next entire bounded failed download.
No upstream server code is executed by retrieval. A full archive is not a
recursive checkout of submodules.

Prospective retry after `source-repair-01`: ClickHouse downloaded completely
(361264396 bytes), then its compressed regular-file inventory exceeded 4 MiB.
Keep that failed receipt and full archive. Permit 16 MiB compressed inventory
per repository for the retry, inside the same source allocation. Reuse the
complete archive only after its recorded revision, size and SHA-256 match;
reference it without copying or changing the failed evidence. Vector and
FoundationDB still require their full downloads. All other limits are unchanged.

## Labs and decision rules

Lab proposals are registered below before their commands run. Storage will
vary existing external-sort run size at fixed semantic data, query will vary
physical locality and pruning on the same logical rows, and operations will
vary offered cadence and observer work on the real Store path. Each records
CPU, wall time, memory domain, bytes/IO and correctness separately. Rates must
state their denominators; ACK p99 with too few samples is not a tail guarantee.

Do not multiply wins from different fixtures. A useful research vector needs
a source mechanism, a causal hypothesis, a null/counterexample, a matching
workload, an observed tradeoff and a next discriminating experiment. A screening
win needs a fresh confirmation before promotion. Existing borrowed-log CR2
non-nomination remains unchanged. No production representation, custody, query
or retention contract changes are included.

Detailed prospective lab supplements are
[storage](cross-system-storage-sweep-proposal.md),
[query](cross-system-query-sweep-proposal.md), and
[operations](cross-system-cadence-sweep-proposal.md).
Each supplement must be registered before its own first execution. Frozen build
identities bind the corresponding supplement, so later labs do not change the
source identity of already measured work.
