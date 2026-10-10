# Catalog query evidence: lossless archive and accounting amendment

Status: draft; root registers this representation-only amendment before invocation.
This changes no workload, measurement, oracle, expected outcome or trial count.
Coordinator accounting allowed completed CQ1 diagnostic evidence plus the active
first pair to exceed the campaign's 256 MiB aggregate allowance. Preserve and
report that overshoot; no next workload is admitted until a bounded inventory is
restored. Per-slice guards were insufficient. Earlier completion-query evidence
is a separate historical campaign and remains visible in the disk inventory.

The [offline utility](../../../tools/bench/labs/catalog/compact_evidence.py) transforms
only explicitly named closed `catalog-boundary-*` or `catalog-maintenance-*` data
directories, with successful cleanup status. Root waits for the active first pair
to finish. Symlink inputs/contents, overlapping inputs, preexisting output paths
and foreign pool markers fail closed. Scratch controls use launcher-owned mounted
`FABRIC_SCRATCH_ROOT`; no system temporary directory is substituted.

Every JSON file of at least 1 MiB is gzip-compressed without parsing or semantic
normalization. Stream the exact bytes, then decompress/read back and compare every
byte, original SHA256 and size. Record original logical path/hash/size and new
compressed path/hash/size before removing the raw representation. JSON readers
use the original path first, then `original.json.gz`; the original logical artifact
is recoverable exactly. The [CQ1 summary](../../../tools/bench/labs/catalog/boundary_summary.py)
implements that fallback. The reusable `compress_json(path, retain_receipt)` API
allows a driver to compact a fully verified, closed trial before its next trial;
the callback persists intent before raw-file removal and completion afterward.

All gzip artifacts in the named closed slices, including objects and frozen
binaries, are decoded with a 1 GiB per-object ceiling. Pool keys are decoded SHA256;
on every match, streaming decoded-byte equality is mandatory. Hash equality alone
does not establish equivalence. Preserve every existing gzip path by atomically
replacing it with a hardlink to the owned canonical gzip. Record old/new compression
hashes and sizes, canonical path, decoded hash/size and exact comparison. Compression
headers may change; decoded evidence bytes cannot. Always create and verify the
canonical and replacement before replacing the original. No sole evidence copy is
deleted. A crash retains the incremental transformation receipt and remaining files.

Before any transformation, controls must reject changed decoded bytes, truncated
gzip and missing objects through the same exact-byte checker. Successful same-byte
readback is the positive control. Full path inventories contain compressed-file
hashes, logical lengths, allocated blocks, devices and inode identities. Report the
logical sum of all paths separately from unique-inode file bytes and allocated
blocks; pool aliases count once physically. Directory/filesystem metadata overhead
is not a payload measurement. Receipt bytes/blocks are charged separately with a
stable final accounting check; no receipt contains its own hash.

Initial concrete invocation after the current pair's successful cleanup:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-evidence-compaction-01 --lab query --stage query --seconds 600 -- python3 -B tools/bench/labs/catalog/compact_evidence.py --inputs docs/experiments/benchmarks/data/catalog-boundary-diagnostic-01 docs/experiments/benchmarks/data/catalog-boundary-pair1-01 --account docs/experiments/benchmarks/data/catalog-metadata-run-01 docs/experiments/benchmarks/data/catalog-preboundary-build-01 docs/experiments/benchmarks/data/catalog-preboundary-build-02 --prior docs/experiments/benchmarks/data/lab-completion-run-01/query --pool docs/experiments/benchmarks/data/catalog-evidence-pool-01 --out docs/experiments/benchmarks/data/catalog-evidence-compaction-01
```

Root substitutes the actual closed diagnostic directory if its registered name
differs; no active directory may be added. The aggregate catalog inventory includes
all transformed slice directories, immutable metadata precursor evidence, both
frozen baseline archives and the canonical pool. Prior campaign paths get separate
before/after inventory and no mutation. Record both campaign sizes and their sum;
the 256 MiB cap applies to the explicitly named catalog query dataset, not a hidden
claim that all historical repository evidence fits that cap. Subsequent amendment
invocations include every new catalog query dataset in `--account` or `--inputs`.

Exit zero requires both unique-inode lengths and allocated file blocks, including
the new compaction receipt, to fit 256 MiB. Otherwise exit 2 with status
`cap_exceeded_preserved`, preserve all evidence and stop admissions for a separately
reviewed budget disposition. The representation change is not experimental speedup.
After compaction root reruns the contained summary using gzip fallback and records
its exact command/exit. No compaction, checker or validator ran during preparation.
