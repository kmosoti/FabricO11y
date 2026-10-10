# Fresh cadence boundary confirmation proposal

Status: prospective preparation; root must register this supplement before the
first execution. This is a separate copy of the completed
[cadence sweep](cross-system-cadence-sweep-proposal.md); its source, receipts and
decision rules remain frozen. No production configuration or source changes.

The original event-arm ledger records four append groups for each finite burst,
165–170 at2ms targets, and506–508 at5ms targets. Existing append-intent hooks
identify actual successful append calls; corresponding data/marker sync counts
are twice the append count. These observations establish grouping, not the cause
of group closure. The original after-sealing event-arm offer intervals had medians
about2.072–2.074ms, and about27% were at or below2ms. Target cadence therefore
does not specify actual commit-thread arrival timing. The50ms window,2ms quiet
rule, IO, producer timers and thread scheduling are competing explanations.

The registered null is that adjacent cadences do not reproduce a distinct
grouping/latency tradeoff beyond observer or scheduling variation: the broad2/5ms
association could follow offered load and IO instead of a reproducible boundary.
The candidate prediction is fewer append groups and higher ACK latency at1ms
than3ms, with2ms as an anchor, under unchanged `CommitMode::GROUPED`. This is
an arrival-cadence association, not a causal ablation of the quiet parameter.

Run12 fresh cells: worker1 dense-middle only, target cadence1/2/3ms, quiet/events,
and new payload markers2703204363/2703204364. Reverse observer order in the second
replication. Markers are body identities, not independent scheduler seeds.
[cadence_boundary_probe.rs](../../../crates/fabric-server/examples/cadence_boundary_probe.rs)
copies the previous native fixture; [cadence_boundary.py](../../../tools/bench/labs/cross_system/cadence_boundary.py)
copies its driver. Seed three128-Batch journals with128 log rows per dense Batch
and two otherwise. Extend legitimate OTLP bodies to exactly8192 encoded bytes
per Batch. Offer512 ordered Batches over128 strands in sequence rounds4–7 using
absolute targets and asynchronous ACK observers. Only one actual production
sealer pass runs; there is no historical-scheduler arm or filesystem poller.

Quiet leaves the existing phase observer uninstalled; events charges its setup,
timestamps, ledger and allocation. Record actual offer/reply/target timestamps,
lateness and after-sealing interarrival distributions. Compare after-sealing ACK
p50/p95/p99, process CPU, actual append counts and accepted Batches/group. Count
groups from existing event hooks only; do not infer quiet-arm groups from ACK
clustering. Builder-return hooks lack journal labels; checkpoint service time
and rename times remain unmeasured. Linux CPU ticks have10ms resolution. HWM
includes startup; after-sealing offers exclude timed sealing but do not reset IO
or cache history.512 ACKs share groups and are not independent durability samples.

Before interpreting performance, all12 cells must accept all512 Batches, retain
exact replay and reopened retry/conflict checks, and pass eight unchanged
pre/post Scan/Walk oracle chains. Missing/duplicate query rows and both telemetry
defects must be rejected per cell. Expected totals are6144 offers/ACKs,96 positive
chains,24 rejected query controls and24 rejected telemetry controls; count actual
outcomes rather than assuming these totals. All actual initial file sizes must
match across cadence/observer arms within each marker.

A screening association requires1ms event groups ≤75% of3ms event groups and
after-sealing ACK p50 ≥110% of3ms, in both fresh markers. Observer neutrality is
a separate check: events versus quiet after-sealing p50 within±10% in every
cadence/marker pair. If either screen fails, report the failed/null/mixed outcome;
do not select favorable tails. Repayment or a production nomination is outside
scope. Even successful screens cannot establish timer causality; a later quiet
parameter ablation would need separate scope and prospective registration.

Allow500 seconds including coordinator overhead,450 internally,20 per cell.
State≤64MiB and raw≤128MiB/cell. Total new retained evidence≤64MiB (maximum of file logical/allocated bytes): successes
≤16MiB including one exact-readback representative archive≤8MiB, plus a reserved
whole-failure archive≤48MiB. Full ACK/hook vectors, verdicts, file hashes and
generators accompany summarized successes. A streaming compressed-size limiter
must reject an injected oversized write before fixture admission. Preserve the
whole failing raw tree before cleanup; if complete archival/readback or bounds
fail, retain the original and stop. Root admission reserves64MiB in operations.

Root build, in its separately admitted build job:

```sh
cargo build --offline --release -p fabric-server --example cadence_boundary_probe --features phase-probe
```

Registered execution command, after root grants the serialized slot:

```sh
python3 tools/resource_group.py -- python3 tools/bench/labs/cross_system/run_sweep_job.py --id cadence-boundary-01 --lab operations --seconds 500 --reserve-mib 64 -- python3 -B tools/bench/labs/cross_system/cadence_boundary.py --binary /run/media/kmosoti/data/FabricO11y/cargo/release/examples/cadence_boundary_probe --out docs/experiments/benchmarks/data/cross-system-sweep-01/operations/cadence-boundary-01 --protocol docs/experiments/benchmarks/cross-system-sweep-protocol.md --proposal docs/experiments/benchmarks/cross-system-cadence-boundary-proposal.md
```
