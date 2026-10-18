# Bounded verification concurrency experiment

Registered before execution or changes to the production scheduler. The objective
is shorter verification latency with identical checks, outcomes and evidence.
The alpha acceptance queue continues; heavy jobs remain serialized. No GPU code,
oracle change, reduced workload, shortened exposure or acceptance-policy change
is included.

## Evidence and alternatives

The local host has six physical CPU cores/twelve threads and an NVIDIA RTX 3060
Laptop GPU with 6 GiB VRAM. Inspection found sequential registry dispatch and
sequential extended-CI steps; semantic mutants share a mutable tree and target.
GPU arithmetic does not implement these existing commands or shorten registered
wall-clock trials. Cargo already parallelizes compilation and libtest runs tests
within each binary concurrently; adding nested workers can instead oversubscribe
the CPU or contend on build locks.

Investigate bounded CPU concurrency first. Independent CI runners are a separate
alternative for expensive proof/mutation groups. Do not generalize a short
source-check result to the full CI or acceptance critical path.

## Fixed workload and hypothesis

Use these unchanged registered commands at one clean source revision:
`delivery-oracle`, `query-oracle`, `workload-rate-oracle`, `soak-decision-rule`,
`telemetry-contract`, `telemetry-cli`, and `ui-palette`. Inspect their filesystem
effects before admission. Give each invocation its own DATA scratch, temporary
directory and receipt directory. Do not schedule source-mutating checks, Cargo,
VMs, browsers, pressure trials, outages or the soak concurrently in this sample.

An experimental Rust example invokes the existing registry runner in child
processes, each selecting exactly one registered ID. Compare a FIFO with one
worker against the same FIFO with two workers. This isolates scheduling without
changing the runner, commands, fixtures, oracles or receipt format. One worker
must not overwrite another's current marker, logs or receipts. Keep all results;
a failed job must not hide independent jobs or become success.

Before timing, use an isolated fake-check registry to inject exit 7 and confirm
the experiment rejects a claimed all-pass result while retaining every job's
actual outcome. Also reject missing, stale, modified-log and running-marker
evidence. These controls concern the experimental collector, not a new release
gate. Then run one warmup per mode and seven alternating paired repetitions.
Compare the exact ID/argv/source/result/exit inventory, require all selected
original checks to pass, verify each log digest, and retain all receipts.

Record monotonic whole-batch wall time, per-check duration, completed/failed job
counts, peak active workers and whole-command cgroup resource observations.
Report median paired latency and each pair. Charge process/receipt overhead to
both modes; compilation precedes timing. This is warm-cache execution, not a
cold-build or cross-hardware comparison.

## Decision and containment

Reject the candidate if any semantic/evidence/control check fails. Adopt no
production scheduler change from this example alone. A median improvement below
5%, or higher contention/resource cost without useful latency gain, is grounds
to retain sequential local execution and prioritize a separately measured CI
shard design. Preserve regressions and bound conclusions to this seven-check
workload.

Pin exact source, toolchain, commands and helper bytes before running. Use the
required launcher, DATA-only scratch, strict full census/95 GB stop, explicit
reservation and the coordinator's exclusive workload lease. Two lightweight
workers share one existing bounded outer group; this does not authorize parallel
heavy acceptance jobs or different limits. Research/results belong in the wiki;
keep raw observations and failures on DATA. GPU and remote CI experiments require
their own measured workload and containment plan before execution.
