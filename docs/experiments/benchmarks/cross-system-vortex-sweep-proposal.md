# Vortex comparison on retained query-sweep rows

Status: proposed extension; no package downloads, builds or workloads have run
for this lab during preparation. The owner registers this supplement before
executing [vortex_sweep.py](../../../tools/bench/labs/cross_system/vortex_sweep.py).
This is a compressed-representation experiment, not a production format change
or a replacement for the historical CR2 no-nomination.

Prospective retry supplement: the first `vortex-01` attempt failed in the first
worker on Arrow materialization of a zero-length chunked struct array, before
completing its measured cases. Preserve that original attempt and scratch.
For a new `vortex-02` attempt, the adapter constructs an empty Arrow table from
the returned struct's actual field schema, then performs the same checked cast
to the ingestion schema used for nonempty results. Non-struct outputs remain
errors. This is schema handling, not a changed predicate, order, limit or oracle.
The registered absent query and independent exact-source check still require
an empty answer. All original cells, version pins, controls and bounds remain.

Use exactly the successful [query-sweep](cross-system-query-sweep-proposal.md)
source and Parquet objects for rowgroups8192 at widths16/1024 and
clustered/mixed physical locality. Bind every object to its original hash and
copy/read back those bytes into the new evidence directory. No new log corpus
or changed answer contract is generated. Two fresh processes per configuration
compare three arms: Arrow full Parquet decode, Vortex full decode, and Vortex
predicate scan followed by exact verification. Six queries with first+three
reuse calls yield576 measured answers in eight children; order rotates by
query, call and fresh repetition.

Use the documented Vortex Arrow-table writer and file scanner. Convert
FixedSizeBinary16 identity to variable binary explicitly before ingestion,
record that cost, and verify the bytes/identity and all physical rows exactly.
The Vortex writer selects its layout; this is not a claim of equivalent row-group
boundaries. Charge initial Parquet decode, identity conversion, combined Vortex
compression/write and file open as incremental construction. Record full
representation readback separately as verification. Preserve the Vortex files,
their encoding trees, conversion costs and every measured phase.

Vortex's [Python LIKE API](https://docs.vortex.dev/api/python/expr) exposes SQL
wildcards without a documented escape parameter. Only nonempty needles with no
percent/underscore/backslash and at most254 UTF8 bytes enter LIKE pushdown.
Other needles use time-only pushdown and decoded exact literal matching.
The `C%z` common shape must use this fallback. A real two-row Vortex control
(`C%z`,`Cxxz`) demonstrates that raw `%C%z%` LIKE produces a false positive
which the independent literal oracle rejects; exact fallback must pass. Extra
engine controls cover UTF8 lambda, bracket, `.*`, underscore, backslash and
254/255-byte needles. The254-byte route boundary is a conservative adapter
choice inspired by inspected source, not a claim about every wheel kernel.

Every arm still applies exact half-open time filtering and literal membership,
then globally orders all matches before selecting64 rows. Never apply the
scanner's limit before ordering mixed physical rows. Results are compared to
independent Python source-tuple filtering/order/slicing from the retained source;
the existing query oracle is unchanged. Missing/duplicate/content/order/window
checker controls remain required. Preserve actual bad outputs before raising.

The [documented scanner](https://docs.vortex.dev/api/python/io) returns Vortex
arrays before conversion to Arrow. Record representation scan, selected Arrow
decode, exact filter/order/limit and Python result projection separately, and
their total. Returned Arrow string/binary views cast to the ingestion schema
inside selected Arrow decode, so that materialization cost is included.
Matcher/expression construction is included in predicate scan.
RSS high-water is whole-process cumulative; proc IO is not decoder bytes.
Full readback and controls precede measured calls, so no cold/warm cache claim
is made. CPU affinity is restricted to two inherited CPUs; Arrow threads1,
Rayon threads1 and Tokio worker environment2 are recorded, without claiming
all Vortex internal threads honor those environment values.

Encoding trees record what the wheel returned; an FSST name alone does not
prove compressed LIKE dispatch or zero decompression. Retain
`kernel_dispatch_verified:false` unless separate executable evidence supports
that claim. The previously inspected Vortex source pin
`881b867d736e8955a0381655f80d6d2d630fc590` differs from an installed release;
this experiment makes no unsupported attribution between them.

The pinned wheel closure is Vortex-data0.87.0 (cp311-abi3 Linux x86_64,
SHA256 `9525588a85777613b84b058e850fb34bded51d5da094eebfa392cdf693a7f2ea`),
PyArrow22.0.0 (same cp314 wheel as the query sweep), typing-extensions4.15.0,
Substrait0.28.0, substrait-protobuf0.79.0, substrait-extensions0.79.0 and
protobuf6.33.5 (cp39-abi3 Linux x86_64). Every URL, exact length and SHA256 is
frozen in the runner from official PyPI release metadata. Actual installed
distribution versions must match all seven pins. UV records its actual version
and installs only into owned data-drive scratch using no-cache/copy link mode,
no-deps/no-index/required hashes, with UV_CACHE_DIR unset and launcher data-drive
TMPDIR recorded. Link rejection remains strict; accounting failure is recorded
without replacing the original failure. No ambient or production dependency
changes are made.

For each query, fresh repetition and wall/CPU metric, report incremental
conversion cost divided by reuse-median savings against Arrow Parquet. A
nonpositive saving means no finite repayment. This fixed-query model excludes
updates/refills and does not establish universal break-even or engine dominance.

Proposed command after separate owner registration:

```sh
python3 tools/resource_group.py -- python3 tools/bench/labs/cross_system/run_sweep_job.py --id vortex-sweep-02 --lab query --reserve-mib 128 --seconds 1000 -- python3 tools/bench/labs/cross_system/vortex_sweep.py --inputs docs/experiments/benchmarks/data/cross-system-sweep-01/query/sweep-02 --out docs/experiments/benchmarks/data/cross-system-sweep-01/query/vortex-02 --seconds 900
```

The900-second deadline includes dependency preparation and grading. Bound
scratch8GiB, new retained evidence128MiB and free data-drive reserve16GiB, under
the required20GiB/no-swap resource launcher. Retain exact source snapshots,
working-tree diff, wheel verification receipts, commands, inputs, Vortex files,
counterexample vectors, encoding trees, raw metrics, exact compressed answers
and cleanup receipts. Successful cleanup follows exact readbacks and checks;
failure or interruption preserves the complete owned scratch state.
