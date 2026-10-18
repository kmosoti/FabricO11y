# Two-runner verification pilot

Register before implementing or running the pilot. The owner requested research,
benchmarks and parallel verification. Existing required workflows and all ten
extended checks remain unchanged. The pilot cannot replace their merge evidence.

## Hypothesis and comparison

The exact-head extended run 39083441964 completed in 3,291 seconds. Semantic
controls took 1,493 seconds; the other nine checks totaled 1,716 seconds. A
two-runner split may reduce critical-path time despite repeated setup and lost
cache/target reuse. This arithmetic is a hypothesis, not a measured speedup.

Use one new clean source revision for the original extended workflow (serial
baseline) and an additional experimental workflow with two isolated CI runners.
Each runner uses the original pinned environment, actual host inspection,
disk-backed storage and resource launcher. Keep heavy checks serial per runner;
the local alpha queue remains serialized and continues independently.

One shard runs `semantic-mutants`. The other runs every remaining extended ID
once, in original order. Keep the same commands, tools, seeds, fault durations,
proof bounds and expected outcomes. Matrix fail-fast must be false. Preserve all
partial and failed receipts/logs. A failed check must not hide later independent
checks. Do not reuse successful results from a different source revision.

## Evidence and failure controls

Upload each shard's complete receipt directory separately. A bounded Rust
collector must require both named shard directories, disjoint exact registered
ID sets, no running markers, ordinary receipt/log files and exclusive output
creation. It copies bytes into one new DATA-backed directory without overwrites.
The original independent Python `verify_receipts.py --profile extended` then
checks the combined evidence at the actual clean source revision.

The aggregate job must run after failed/canceled shards and explicitly fail if
either shard failed, was skipped, was canceled or is absent. It must not turn a
skipped aggregate into success. Before publication, exercise missing/duplicate
IDs, incomplete markers, symlinks, reused output, stale source and changed logs;
preserve the existing independent validator's negative controls. This is an
experimental collector, not a new product oracle or acceptance gate.

## Measurement and decision

Start with one pilot at the same commit and runner offering as the canonical
serial workflow. Record actual CPU/RAM/disk and enforced limits, cache outcomes,
setup time, check durations, artifact/aggregation time, queue delay, completion
statuses and complete source/command inventories. Report walltime and total
runner time separately; more runners are not free work. Compare equal complete
check sets only after both final validators succeed. A failed or incomplete pilot
is not a speedup.

The first pilot is a discriminating observation, not a universal ranking. If
useful, repeat before changing production scheduling. Reject the candidate on
any evidence/control mismatch or if extra setup/runner cost erases useful
latency gain. No GPU dependency, new verification tool, weaker gate, release/tag
or concurrent heavy workload on a shared host is authorized by this pilot.
