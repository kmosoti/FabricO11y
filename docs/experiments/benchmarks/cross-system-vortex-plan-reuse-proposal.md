# Prospective Vortex prepared-scan ablation

Register separately before executing [vortex_plan_reuse.py](../../../tools/bench/labs/cross_system/vortex_plan_reuse.py).
No result is recorded here. [Prior screen](cross-system-query-sweep-findings.md)
found selective wide predicate scan dominates selected Arrow conversion, but
did not isolate planning. H1: prepared execution saves enough repeated-call CPU
and wall time to repay preparation. H0: preparation is a small share or execution
dominates, so savings are absent, inconsistent or require many repeats.
No performance threshold is promised and no Fabric nomination follows.

Read the exact retained `query/vortex-02` files and logical-source objects;
verify SHA256 before and after each child. Do not copy inputs, write Vortex
formats or rebuild compression. Select the four repetition0 input files
(width16/1024 × clustered/mixed), and run two fresh children per configuration.
Each child runs six unchanged queries × four calls (first plus three reused) ×
two arms =48 measured answers; eight children total384, **not576**.
Alternate arm order by query/call/fresh repetition. No OS cold/warm claim.

Arms are `vf.scan(expr=expression).read_all()` and
`vf.to_repeated_scan(expr=expression).execute().read_all()`, with the prepared
object constructed **once per query**, timed separately and reused for all four
calls. Both arms share preconstructed immutable expressions; expression building
is excluded from the comparison. File open is recorded separately and shared.
The original preserved **0.87.0 wheel** `vortex/file.py` delegates
`to_repeated_scan` to `_file.prepare`; `vortex/scan.py` exposes parameterless
`execute()` returning ArrayIterator. Its `_lib/scan.pyi` documents execute with
optional start/stop. This matches the exact invocation; current online docs alone
are not evidence of old-wheel API availability. The newly installed wheel's
Python API sources and stubs are copied, hashed and read back into evidence.
An unavailable API fails the attempt without replacing the arm.

Keep literal/time/order/limit semantics from the source oracle: safe LIKE only
nonempty UTF8 literals at most254 bytes without `%`, `_` or backslash;
otherwise time-only pushdown plus exact literal fallback. Never scan-limit
before global ordering. Both arms perform the same schema conversion, exact
Arrow filtering/sorting/limit64 and final Python projection. Preserve actual
answers before grading, source-derived expected rows and all rejected controls.
Reuse the retained two-row wildcard Vortex file to demonstrate the actual
prepared unsafe LIKE rejects the literal oracle; safe fallback recovers it.
Expect eight actual wildcard rejections,48 generic answer-checker rejections
and112 engine literal checks (seven needles × two arms × eight children).
These controls are separate from timed measurements and preserve vectors.

Record per-call scan, Arrow conversion, exact operation and Python projection
wall/process CPU/RSS-HWM/proc IO, plus combined totals. Charge each query's
preparation independently; report both fresh repetitions and first/reuse
boundaries without pooling. Same-query repayment is
ceil(preparation_cost / (new_scan_reuse_median - prepared_reuse_median));
nonpositive savings have no finite repayment. No update/refill or OS cache
model, physical decoded-byte claim or FSST dispatch assertion follows.
Six retained prepared objects may affect shared process memory/cache, so this
is a paired API-path screen, not isolated per-arm RSS or planning-only proof.

Use the exact seven pinned wheels from the Vortex proposal, including
vortex-data0.87.0 and PyArrow22.0.0, with length/SHA verification. Install only
in launcher-owned scratch using uv --no-cache and copy linking, no dependency
resolution, no production dependency or system install. CPython3.14 x86_64,
two inherited CPUs by explicit affinity, identical thread settings, unchanged
20GiB/no-swap launcher limits. Maximum512MiB new scratch,16MiB retained evidence,
16GiB free reserve,120 seconds inner and180 outer. Full failure scratch stays
preserved; successful owned scratch is removed only after exact readbacks.
Record historical source snapshot, revision, actual API sources, input hashes,
versions, all commands/exits, resource observations and cleanup receipt.

```sh
python3 tools/resource_group.py -- python3 tools/bench/labs/cross_system/run_sweep_job.py --id vortex-plan-01 --lab query --reserve-mib 16 --seconds 180 -- python3 tools/bench/labs/cross_system/vortex_plan_reuse.py --inputs docs/experiments/benchmarks/data/cross-system-sweep-01/query/vortex-02 --out docs/experiments/benchmarks/data/cross-system-sweep-01/query/vortex-plan-01 --seconds 120
```

The baseline and candidate are upstream Vortex paths on matched retained data,
not native Fabric paths. No production format migration or CR2 reconsideration.
