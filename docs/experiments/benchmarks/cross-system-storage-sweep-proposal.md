# External-sort run-size storage sweep

Prospective supplement to the [matched workload protocol](cross-system-sweep-protocol.md).
No result is claimed here. The owner authorizes this finite exploration under
the unchanged 20 GiB resource envelope; production defaults and the previous
memory census outcomes remain unchanged.

## Mechanism and falsifiable prediction

The existing `FABRIC_RUN_MIB_EXPERIMENT` compile selector in
[bounded.rs](../../../crates/fabric-server/src/segment/bounded.rs) accepts 8, 16
and 32 MiB. A normal build retains 16 MiB. `Runs::push` spills a nonempty run
when its row count reaches 32768 **or** its estimated resident bytes plus the
next row exceed the selected byte cap. An oversized first row is permitted.
For constant row cost `r`, the simplified fit model is
`min(32768, floor(cap_bytes / r))`; actual estimates include row struct size,
String capacities and map allowances. Output chunk bytes and merge fan-in remain
unchanged. A selector change is not evidence that the byte cap bound first.

H1: at 64 MiB journal payload, changing run size produces repeatable changes
in requested-live build peak, spill/merge observations and write work, with a
measurable throughput tradeoff on identical logical input. H0: the row cap or
another stage dominates, there is no repeatable benefit, or a saving moves cost
into IO, CPU or another memory domain. The 16 MiB targets are finite small-input
counterexamples; bigrows isolates byte pressure, while adversarial timestamps
challenge an explanation based only on already ordered data.

The existing probe does not install/take the optional phase observer. Exact
spill counts and exclusive phase CPU are therefore unsupported. The runner
records 100 ms snapshots of `.run-<level>-<index>` paths, sizes and table/level,
and the highest observed initial-run index plus one as a **lower bound** on
spill count. Intermediate levels establish observed merge rewriting; absence
does not prove none occurred. Both byte-cap and row-cap hypotheses stay open
where these observations cannot discriminate. For 512-byte log bodies alone,
8 MiB fits at most 16384 bodies; 16 KiB bigrows bodies fit at most 512/1024/2048
at the three caps, before additional resident overhead. Those are source-derived
upper bounds, not measured Rust capacities or exact run counts.

## Finite cells and execution

Freeze three release/offline/locked binaries before measurement, one per cap.
Build the existing `readiness_memory_lab` example with two Cargo jobs, unset
the borrowed-log and spill-workspace experiment selectors, and record compile
settings, source/protocol identity, binary copies and SHA-256. The binary copies
live in a distinct data-drive cache directory and must be removed by the
coordinator after all four jobs, with a cleanup receipt. No upstream code runs.

The complete grid has 36 fresh pairs / 72 native cells:

| Dimension | Fixed values |
|---|---|
| Compile-time run cap | 8, 16, 32 MiB |
| Encoded Group payload target | 16, 64 MiB |
| Shape | steady, adversarial, bigrows |
| Fresh repeats/seeds | 0: 2703204353; 1: 2703204354 |
| Builders | reference and bounded, using the same frozen cap binary |

Actual framed journal bytes and encoded payload bytes are reported separately;
the generator finishes a Group, so a target is not an exact file-size claim.
Each `(target, repeat)` job contains nine pairs / eighteen native children.
Reference-first order is reversed when `(cap_index + shape_index + repeat)` is
odd, balancing each treatment across the repeats. References are restricted to
64 MiB maximum; no 128/256 MiB reference is admitted. Fixed-seed reference
repetition provides a matching-control distribution across cap compilations.

Four jobs receive at most 900 seconds each including pinned decoder installation
and cleanup. The inner campaign is 840 seconds, each native child at most 120
seconds, binary-wheel installation at most 45 seconds, wrapper child at most
850 seconds. Build is a separate serialized job with three 360-second command
ceilings, inside its outer 1200-second allocation. Actual elapsed time counts
against the registered round, rather than allocating all maxima additively.

The implementation is [storage_sweep.py](../../../tools/bench/labs/cross_system/storage_sweep.py)
with [storage_with_decoder.py](../../../tools/bench/labs/cross_system/storage_with_decoder.py).
After this supplement is registered, the coordinator runs this build command:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/cross_system/run_sweep_job.py --id storage-build --lab memory --seconds 1200 --reserve-mib 128 -- python3 -B tools/bench/labs/cross_system/storage_sweep.py build --destination docs/experiments/benchmarks/data/cross-system-sweep-01/memory/builds --protocol docs/experiments/benchmarks/cross-system-sweep-protocol.md --binaries-root /run/media/kmosoti/data/FabricO11y/cross-system-storage-sweep-01-binaries
```

For each `TARGET=16,64` and `REPEAT=0,1`, invoke the following with those numeric
arguments and a matching fresh job/directory name; there are exactly four jobs:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/cross_system/run_sweep_job.py --id storage-16-0 --lab memory --seconds 900 --reserve-mib 128 -- python3 -B tools/bench/labs/cross_system/storage_with_decoder.py --destination docs/experiments/benchmarks/data/cross-system-sweep-01/memory/target-16-repeat-0 --protocol docs/experiments/benchmarks/cross-system-sweep-protocol.md --build-manifest docs/experiments/benchmarks/data/cross-system-sweep-01/memory/builds/build-manifest.json --target-mib 16 --repeat 0
```

## Measurements, semantic gates and decision rule

Native boundaries separately report requested-live/incremental build peak,
retained heap after build, instrumented build seconds, actual encoded/framed
MiB per build second, build `/proc/self/io` deltas including `wchar`,
`write_bytes` and cancelled writes, and whole-native wall/CPU/wait4 RSS.
Sample post-exec VmRSS/VmHWM, process IO, cgroup memory/CPU/pressure and logical
journal/segment/temporary bytes every 100 ms. Record parent RSS immediately
before spawn and require that it never imported PyArrow. OS write bytes are
accounting observations, not an assertion of device amplification or completed
durable IO. Inclusive build timing contains decoding, spilling, sorting, merging
and publication; exclusive phase timings are unsupported.

PyArrow 22.0.0 exists only in owned data-drive scratch and a reaped subprocess;
availability/version and each physical-filter check occur outside native cost
measurement. Reuse the existing
[memory checker](../../../tools/bench/labs/cross_system/memory_census.py) without
changing its oracle logic: exact fixture equality, five ordered row/custody
ledgers, manifest semantic equality, applicable existing file-byte equality,
independent FTF1 reconstruction against each file's actual Parquet row groups,
and no build/run leftovers. The row ledgers are a Rust scanner differential,
**not** an independent full-row oracle. No service reservation metric exists.
Run existing missing/misaligned/corrupted-filter and row/custody controls plus
IO counter reset rejection before native admission.

Any semantic/control failure rejects the entire screen, irrespective of timing.
For a shape-specific research candidate versus bounded 16 MiB at the same
target/seed, both repeats must show at least 10% less requested-live peak or
write work, no greater than 10% loss in build throughput, and no greater than
10% increase in native CPU or RSS. Zero-baseline IO cannot establish a percentage
IO benefit. A reproducible difference in sampled spill/merge behavior or heap
must support that the treatment changed useful work; timing alone cannot nominate
an indistinguishable mechanism. Report each cost separately, matched reference
variation, all adverse cells and the null. This is a screening vector requiring
a separately registered fresh confirmation, not production nomination.

## Evidence and cleanup

Category memory has 256 MiB. Prospectively reserve 128 MiB for a complete failed
pair archive and allow up to 128 MiB successful receipts; aim for less than
100 MiB passing evidence. Binary copies stay in the explicitly tracked data-drive
cache, rather than consuming receipt space. Each active whole pair has a 512 MiB
logical/allocated scratch cap; all active scratch stays within the round's
8 GiB allowance and the data drive keeps 16 GiB free. Both arms remain until
paired gates complete. Passing states retain raw native JSON, configurations,
five ordered ledgers, complete regular-file path/size/SHA-256 maps, exact output
manifest/file identities, decoder reconstruction summaries and resource timelines.
Only after native verification, independent filter/file authentication and paired
gates may owned state be removed; no fresh successful fixture tar is retained.

Stop on the first failure/deadline/overage. Save the complete remaining pair
state as gzip tar within 128 MiB, read back every regular payload and compare
path/size/SHA-256 with source before removing scratch. If the archive cannot fit,
record exact partial bytes/hash, retain complete owned scratch and stop. Never
truncate or delete failed state to admit another cell. Interrupted jobs remain
interrupted. The same protocol and source identities must match frozen builds;
an edited harness or decoder checker requires a new registration/build identity.
