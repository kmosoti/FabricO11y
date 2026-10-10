# Registered soak measurement, companion compatibility revision 2

Prospective registration, 2026-10-09, under RC-SOAK in the
[readiness continuation](../formal/readiness-continuation-protocol.md). This
revision accounts for the mandatory dedicated Spindle. The original
[protocol](soak-protocol.md) and [failed run 01](soak-run-01.md) remain historical
evidence with their original meanings. No new result is claimed here.

## Fixed workload and gates

Retain the original 100 simulator identities, seed `0xA11FA001`, two 512-byte
logs/s per identity, 32 metric points/15 s, 100 workers, 64 MiB journal files,
default retention, 60-second warmup and nine 600-second measured windows.
Retain query/management/sampling cadence, all ten gates and every numerical
threshold in `soak_tier.evaluate`, including the first-versus-last RSS rule.
Use CPUs 0–1 for server and companion, 2–3 for the simulator. Freeze both daemon
binaries, simulator, server/spool dump examples, harness dependencies, protocol
and source identities before execution; recheck SHA-256 values afterward.

For RC-GROUP service acceptance, build these binaries with
`FABRIC_ROW_GROUP_CHUNKS_EXPERIMENT=1`, without the allocation-counting feature.
The admitted aligned writer is the sole experimental storage selector; retain
the default query plan and all workload parameters. Preserve the build command
receipt with the frozen binary hashes. This run does not promote the selector
to the production default.

Hypothesis: the current storage implementation satisfies the original memory
drift and correctness gates with native self-observation present. The adverse
case is memory growth, latency/backlog degradation, incomplete recovery or an
independently unobservable companion source prefix. One trial is finite evidence
on one host, not deployment qualification or universal stability.

## Companion evidence and containment

Use the explicit `--companion` adapter. It supplies the generated issuing CA via
`self_spindle_ca`; the sibling `fabric-node` remains the production child. There
is no telemetry suppression or alternative server composition root.

After orderly production shutdown and an empty combined cgroup, run the frozen
`spool_dump` against `server-state/self-spindle/node.conf`. This independently
inspects committed Spool batches and its durable identity/ACK cursor. Require
exactly one identity, a complete ordered source prefix from sequence 1, an exact
retained sequence set and a nonzero ACK cursor within that prefix. Reject any
inspection stderr, including interrupted-append/recovery uncertainty that the
diagnostic currently reports with exit zero. Require each acknowledged companion
source in durable server recovery with identical SHA-256; reject duplicates,
unknown sequences and changed bytes. Unacknowledged sources must remain in the
inspected Spool. Never derive a source from server recovery or synthesize a wire
response from the Spool ACK cursor.

Append those independently inspected companion sources to the simulator ledger,
then append **every** recovered server batch, hashing bytes on both sides as in
R1. The unchanged delivery oracle grades the combined ledger. Do not insert the
companion `node_state` as though its ACK were independently observed on the wire;
its custody assertion is checked separately. The existing `oracle` gate is true
only when the unchanged oracle, companion custody and containment checks all
succeed. The original ten gate names and thresholds remain unchanged.

Spool capacity is 64 MiB, but rotation occurs at 8 MiB and ACKed files can be
reclaimed. This simple adapter deliberately fails if any prefix is gone; it does
not infer missing sources. A failed source-completeness run requires a separately
registered observer revision before another trial. Complete companion wire
attempt/retry evidence is not supplied by this adapter; report that limitation.

Run within the delegated laboratory launcher (16/20 GiB, zero swap) and place
server plus all its descendants in a verified subgroup: memory high
3,000,000,000 bytes, maximum 4,000,000,000 bytes, zero swap, 512 tasks and two CPU
equivalents. Reuse the existing `completion/cgroups.py` and `enter_group.py`.
Record actual limits, peak charge, memory events, swap and group population.
Also record combined server/companion CPU accounting, I/O counters and peak tasks.
OOM, remaining descendants or failed cleanup invalidate the run. The original
2 GiB server RSS gate still measures the server alone; combined cgroup charge is
an additional containment prerequisite, not an interchangeable RSS measure.

## Prerequisites and commands

Every build, control and workload uses the resource launcher. Check mounted
storage, existing workloads and aggregate project storage before admission;
remain below the owner's 100 GB ceiling. Create a fresh frozen mini-tree on
`/run/media/kmosoti/data/FabricO11y/scratch/soak-r2-01/frozen`, with
`tools/qualification`, `bin/examples` and `target` directories. Copy qualification
Python dependencies there and copy `cgroups.py`/`enter_group.py` beside the frozen
harness. Copy the five named binaries to `bin`/`bin/examples`. Preserve exact
working-tree source identity and hashes before measurement. The copied runner
resolves its ownership root to this mini-tree's disk-backed `target`; do not
change its ownership or budget enforcement rules.

Run unchanged decision controls plus new missing-prefix, gap, identity,
ACK/retained-set and changed/missing/duplicate/fabricated-recovery controls before
smoke. Each altered outcome must be rejected. Root coordinates execution.

Before and after each smoke/full command, run
`tools/qualification/verify_soak_freeze.py` through the resource launcher with
`--manifest` pointing to the exact owned `provenance/manifest.json`, a distinct
`--label` such as `smoke-pre` or `full-post`, and a fresh absolute `--out` JSON
path on the data drive outside the owned freeze. It checks the ownership marker,
canonical member paths, absence of links, file sizes and every declared SHA-256.
Keep both receipts and require matching manifest hashes as well as successful
member checks; a changed manifest is not a new authority for the same trial.
This check verifies frozen bytes, not source-to-binary build attestation.

```sh
python3 tools/resource_group.py -- python3 -B -m unittest discover \
  -s tools/qualification -p 'test_soak*.py'
```

With `SOAK_FROZEN` set to the absolute frozen path above, run the 70-second smoke:

```sh
python3 tools/resource_group.py --delegate -- \
  python3 -B "$SOAK_FROZEN/tools/qualification/runner.py" \
  --out "$SOAK_FROZEN/target/alpha-soak-r2-smoke" --duration-s 600 \
  --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
  python3 -B "$SOAK_FROZEN/tools/qualification/soak_tier.py" \
  --seed 0xA11FA001 --bin-dir "$SOAK_FROZEN/bin" \
  --server-cpus 0-1 --sim-cpus 2-3 --companion --smoke
```

Smoke is never a measured trial. Its offered data is below the unchanged 64 MiB
sealing threshold, so `sealing_caught_up` can fail and the real command can exit
1. Admit the full trial only with a complete smoke summary, successful production
exits, companion custody/containment success, all other original gates satisfied,
the expected seal-only failure (or all gates satisfied), and confirmed group
cleanup. Record the actual runner exit and smoke gate outcomes; do not relabel
an exit 1 as a passing soak.

The continuation authorizes a finite 7,500-second outer deadline for the full
7,200-second runner:

```sh
python3 tools/resource_group.py --delegate --runtime-seconds 7500 -- \
  python3 -B "$SOAK_FROZEN/tools/qualification/runner.py" \
  --out "$SOAK_FROZEN/target/alpha-soak-r2-full" --duration-s 7200 \
  --disk-bytes 5368709120 --max-output-bytes 1048576 -- \
  python3 -B "$SOAK_FROZEN/tools/qualification/soak_tier.py" \
  --seed 0xA11FA001 --bin-dir "$SOAK_FROZEN/bin" \
  --server-cpus 0-1 --sim-cpus 2-3 --companion
```

## Preservation and interpretation

Preserve the exact command/exits, frozen hashes, simulator ledgers, independent
Spool dump, full combined transcript, oracle verdict, per-window metrics,
companion summaries, cgroup/launcher receipts and original failures. The spool
diagnostic has a 60-second/128-MiB output boundary; the runner retains its
5-GiB live-data and 1-MiB command-output/evidence limits. Account preserved
artifacts against total storage, then remove owned scratch only after verified
preservation and empty groups. Record cleanup and unresolved uncertainty in a
separate run record and the readiness continuation results. A stopped run stays
stopped. Retention at default byte scale and physical power loss remain outside
this soak; its workload does not reach the default retention ceiling.
