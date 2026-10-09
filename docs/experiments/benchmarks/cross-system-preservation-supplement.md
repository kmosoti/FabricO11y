# Owned sweep failure preservation supplement

Status: prospective preparation; the owner must register this supplement before
running [sweep_preserve.py](../../../tools/bench/labs/cross_system/sweep_preserve.py).
It does not change the frozen storage/source/query workload protocols or any
existing receipt. The query's first attempt remains failed before measured cells.

Origin: the query dependency preparation created one relative symlink inside UV's
owned cache. The sweep's regular-file-only footprint guard rejected that cache
entry. This is a scratch-preservation counterexample, not evidence of a Fabric
semantic defect. Read-only inventory found156 regular files totaling208251789
logical bytes and one internal symlink under
`/run/media/kmosoti/data/FabricO11y/evidence/fabric-work-9a1f74581a6748ea92ebd55ca74fa2d7`.
Its target remains within that exact outer owned root. No workload or cleanup
has been executed by the helper author. The retry will use no-cache/copy behavior
under separate root preparation; it does not overwrite this failure.

The helper admits only an inactive owner-confirmed original directory directly
under the mounted drive's `evidence/fabric-work-<32 lowercase hex>` namespace.
It runs under the unchanged resource launcher and the serialized sweep coordinator.
It uses240 seconds internally, raw512MiB and compressed128MiB limits. Before
admission, the existing query256MiB category must have room for the complete
128MiB archive plus1MiB metadata. An empty-only source cleanup uses the source
category allowance and rejects any entry rather than widening cleanup scope.

Inventory uses lstat and records every regular file, directory and symbolic-link
entry. Regular-file names, sizes and SHA256 hashes cover the complete owned tree.
Directories retain permission/time metadata, including empty directories;
symlinks retain their exact target strings and resolved target identity. All
targets must resolve inside the original owned root. Special entries, path
traversal and duplicates are rejected. Symlinks are archived as metadata, never
followed for file contents and never extracted. Regular tar members are emitted
explicitly even when original files share an inode, avoiding hardlink deduplication.

Before real preservation, a tiny valid internal-link fixture must verify; mutated
payload, missing-file and changed-link archives must be rejected, and an explicit
outside-root target must fail the ownership boundary. Preserve all original files
on any failure. The helper then streams the entire original into a fresh archive,
verifies all regular names/types/sizes/hashes and link targets twice, and inventories
the original again. It removes the original only if both readbacks and the final
unchanged-tree check succeed. Hash comparisons rely on SHA256 collision resistance;
the procedure does not recreate or execute downloaded artifacts. Keep the manifest,
origin/source hashes, archive hash and a new cleanup receipt beside the failed query
evidence. Do not mutate the old failure or coordinator receipts.

Proposed query preservation command, after registration:

```sh
python3 tools/resource_group.py -- python3 tools/bench/labs/cross_system/run_sweep_job.py --id query-failure-preserve-01 --lab query --seconds 300 --reserve-mib 129 -- python3 -B tools/bench/labs/cross_system/sweep_preserve.py --root /run/media/kmosoti/data/FabricO11y/evidence/fabric-work-9a1f74581a6748ea92ebd55ca74fa2d7 --out docs/experiments/benchmarks/data/cross-system-sweep-01/query/sweep-01/preserved-scratch-01 --category query --origin docs/experiments/benchmarks/data/cross-system-sweep-01/coordinator/query-sweep-01/receipt.json --origin docs/experiments/benchmarks/data/cross-system-sweep-01/query/sweep-01/failure.json --proposal docs/experiments/benchmarks/cross-system-preservation-supplement.md
```

For the separate empty source-failure outer directory, root may use this same
command shape with its exact authorized root, fresh source-category output,
original source receipt and `--empty-only`. The helper must fail if that directory
contains entries. No arbitrary original evidence-directory cleanup is admitted.
