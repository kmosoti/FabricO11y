# Readiness continuation protocol

Owner scope: close the known invariant and readiness gaps, continuing through
prerequisites and corrections. Preserve the existing product contract, independent
oracles, bounded-sealer acceptance and soak thresholds. This registration adds
checks and one opt-in writer experiment; it does not reinterpret prior failures.

## Work queue and decision rules

| ID | Work | Deciding check |
| --- | --- | --- |
| RC-MERGE | Independent stable-sort equivalence across byte limits, ties and multiple merge levels | 64 seeds, five byte limits and thirteen fan-in boundary populations; exact ordered payloads and no leftover runs; changed-key/payload/tie-order controls rejected. |
| RC-RETENTION | Positive byte-ceiling boundary, long-lived readers, interrupted deletion and restart | Exactly the independently declared retained suffix at B and B−1; old pages Gone; all Scan/Walk query chains match the unchanged Python oracle; missing-row control rejected. |
| RC-FAULT | Ordinary failure cleanup before recovery, plus existing process-kill recovery | All seventeen existing syscall cases must hit the intended injection, preserve input and recover the exact clean manifest. Ordinary errors leave no build/run artifacts before retry; killed processes may leave artifacts which restart must remove. Leaked-before-retry and other altered outcomes must be rejected. |
| RC-FORMAL | Restore pinned TLC and execute existing delivery/trace models | Existing model scripts and negative controls, with unchanged expected states and traces. Restore further registered verification tools on the data drive as needed. |
| RC-GROUP | Decouple bounded input chunks from physical sorted row groups | Original sealer equivalence, strict pruning equality, 80 MiB heap ceiling, 10% 64→256 MiB scaling, cleanup and repeatability gates; no relaxed threshold. |
| RC-SOAK | Make the soak account for the mandatory companion, then run it | Separately registered compatibility revision retains the original ten gates and adds complete independently observed companion custody and containment. Smoke must complete before admitting the full duration. |
| RC-INSTALL | Resolve installed-service enforcement evidence | Inspect an isolated supported host/VM, register exact placement and run the existing acceptance checks without changing base infrastructure. |

## Writer experiment

H1: retaining Parquet's reference 8,192-row groups while feeding byte-bounded Arrow
chunks can recover identical pruning without restoring the old payload heap.
H0: encoded writer state, dictionary accumulation or chunk-sensitive encoding
violates memory or exactness constraints. Keep raw Batch/gap group flushing bounded.
Build each arm from the same source and toolchain; the only selector is the
compile-time `FABRIC_ROW_GROUP_CHUNKS_EXPERIMENT=1`. The default arm remains unchanged.
Incremental filters must encode exactly the trigrams of each physical group and
must reject every known false-negative control.

Use the frozen acceptance generator seed `0xA11FA001`, original steady, outage,
adversarial and 16 KiB-body shapes. First screen steady64, bigrows64 and steady256
(one fresh process per arm/cell); stop the candidate on any semantic or memory
failure and retain the counterexample. A successful screen admits the original
eight-workload, three-run acceptance suite; screening alone is not acceptance.
Measure ordered row digests, manifests and applicable file hashes, row-group
pruning at 10/60 seconds, exact filter alignment, counting-allocator incremental
peak, RSS, wall/CPU, I/O and spill bytes. Preserve neutral or adverse CPU/time
results. Full acceptance remains the original registered decision, including soak.

## Execution and resources

### RC-GROUP second candidate registration

Screen02 retained exact logical rows, pruning and filter alignment but failed
the large-row file-byte gate; its candidate heap was51,384,696bytes. The pinned
Parquet writer restarts its1,024-row encoding mini-batches for each Arrow input.
Byte-limited chunks split these boundaries, changing page encoding.

Before the next screen, admit an opt-in logs-only variant with1,024-row chunks
and a17MiB owned-payload cap. Registered16KiB bodies fit an encoding mini-batch
within that cap. Other sorted tables retain their current chunk policy; the
default writer remains unchanged. An oversized row can exceed the cap by one
row, as in the existing builder; no general whole-process memory bound is
claimed. Earlier byte-cap flushes for arbitrary larger rows must preserve exact
logical data and sound filters, but are not assumed byte-identical. Reuse every
original screen gate, including80MiB heap and exact applicable file bytes. The
first candidate failure remains failed. A passing screen still requires the
full registered repetitions and service checks.

Use `python3 -B tools/resource_group.py -- COMMAND` for every build, workload and
validator, serializing resource-heavy jobs. The data drive at
`/run/media/kmosoti/data/FabricO11y` owns caches, tool installations and scratch.
Laboratory maximum20 GiB, zero swap; native server/companion maximum4 GB;
project storage maximum100 GB. Default job deadline30 minutes. The already
registered 7,200-second soak runner requires a finite outer deadline of7,500
seconds, with smoke and cleanup checked first. No workstation power cuts, base
infrastructure changes or release publication are part of this continuation.

Record commands, tool/source hashes, exits, resource observations and cleanup in
[the execution record](readiness-continuation-results.md). Preserve failures before
cleaning owned scratch. Only unavailable external capabilities may suspend their
own queue item; continue other admitted work.
