# Closed-evidence reclamation and backlog discriminator

Status: prospective continuation registration. Historical outcomes, contracts,
oracles and negative-control expectations remain unchanged. This work uses the
existing14400-second frontier,20GiB/no-swap parent and mounted data drive. The
aggregate incremental ceiling remains1894879232bytes; capacity832MiB and query
1024MiB remain. It is not qualification.

## Closed evidence transformation

Hypothesis: exact duplicate evidence can be consolidated without losing any
decoded evidence, frozen source identity or direct evidence path. Null: no
verified savings, or any preservation/ownership check fails. No historical
resource violation is regraded by a later transformation.

The only eligible paths are terminal, inactive catalog coordinator jobs'
`working-tree.diff` and gzip objects in `catalog-borrowed-log-run-01` and
`catalog-many-segment-run-01`/`catalog-many-segment-run-02`. No failure archive or
executable is removed. Coordinator diffs require identical complete bytes.
Capacity objects require identical complete decoded bytes and length, verified
by streaming comparison, not hash alone. Choose an existing capacity-owned
canonical gzip representation; preserve filenames with regular-file hardlinks.
Keep capacity objects within their existing assignments. Never use symlinks or
the query-owned object pool.

Before each replacement, persist old/new compressed SHA256 and lengths, decoded
SHA256/length, canonical path and original inode identity in a transformation
manifest. Recheck bytes immediately before atomic replacement. Gzip header bytes
may change; historical compressed hashes refer to the preserved old identity in
the transformation manifest, while decoded evidence is unchanged. Reject changed
payloads, truncated gzip, missing canonical objects and unexpected links in
scratch controls. Record logical and allocated bytes before/after, ownership,
commands, exits, CPU/memory/IO, elapsed time, and cleanup.

Bootstrap uses a small hash-bound source/protocol receipt instead of the ordinary
duplicated whole-source snapshot: there is insufficient admitted storage to
create that snapshot before reclamation. The helper acquires the same serial
campaign lock, verifies containment and remaining preparation/frontier budgets,
and writes a normal catalog coordinator receipt, charging all elapsed execution
to those budgets. It must finish within180seconds and use only owned disk-backed
scratch. This exception applies only to this reclamation job. No performance
workload uses it.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/catalog/coupled_reclaim.py
```

## O8 backlog discriminator

After measured reclamation and fresh admission, reuse the exact executables in
[the existing freeze](data/catalog-overlap-freeze-01/freeze.json). Register a
new `--profile backlog` fixture without changing the earlier light pilot:
512 unique deterministic4000-byte log bodies preloaded before node start;
8MiB Spool,20-second operation, one node per arm, serial then overlap, matched
50ms response delay, unchanged metric/configuration cadence. This guarantees
more than one collection Batch of source data. Require positive actual successor
commit-before-prior-ACK exposure; otherwise report inconclusive. Compare exact
accepted logical/encoded bytes, Batch counts, retries/refusals, source/Spool ACK
latencies, CPU per accepted logical MiB, cgroup memory/IO, sampled RSS, backlog,
and visibility intervals. This one pair cannot nominate a default.

All offered bodies and accepted producer/server Batch identities must match;
the unchanged query oracle grades complete logs/metrics/rate chains. Retain
negative-control verdicts and failures. Visibility polling covers all512 logs;
it is an interval observation, not instantaneous latency. Server384MiB/node64MiB
service maxima, no swap, parent20GiB, scratch8GiB remain. Reserve16MiB aggregate/
query evidence including failure archives, recheck after dispatch. Deadline180s
for the wrapper,150s for the driver; all execution counts against existing O8
and frontier budgets. No remote service or new build is required.

```sh
python3 -B tools/resource_group.py --delegate -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-overlap-backlog-01 --lab query --stage query --seconds 180 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 16 -- python3 -B tools/bench/labs/catalog/coupled_overlap.py --stage pair --profile backlog --freeze docs/experiments/benchmarks/data/catalog-overlap-freeze-01 --out docs/experiments/benchmarks/data/catalog-overlap-backlog-01 --seconds 150
```

C5's registered full grid remains conditional: duplicate reclamation alone does
not establish an arbitrary failed-journal/Spool/partial-Segment preservation
bound. A reduced fixture would require its own registration and would not count
as completion of that grid.
