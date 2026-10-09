# Matched external query locality sweep proposal

Status: proposed preparation; no executions or measured comparisons yet. The
owner registers the prospective procedure before invoking
[query_sweep.py](../../../tools/bench/labs/cross_system/query_sweep.py).
This lab characterizes ways to avoid decoding; it does not reopen the historical
[CR2 no-nomination](catalog-borrowed-log-findings.md) or qualify deployment.

The source anchors are Fabric's `segment.rs` log schema, Zstd3 Parquet writer,
8192-row groups and optional trigram fallback, and the independent Python query
oracle's half-open time window, UTF8 literal containment, deterministic ordering
and limit. The source dissections motivate conservative candidate pruning and
separate index-construction costs. Previously inspected upstream DuckDB HEAD
is **not** the installed wheel's implementation revision; this experiment binds
the exact wheel version/hash and reports its executable behavior only.

Eight configurations cross UTF8 body widths16/1024, row-group rows1024/8192 and
clustered/mixed physical timestamp locality. Every configuration has32768 rows;
two fresh child processes produce byte-identical source, Parquet and index inputs.
The seed42 shuffle changes physical order without changing the logical records.
Two records share each timestamp, with identity/sequence/index tie-breaking.
The eight-column schema is Fabric's actual log-table schema; attributes are the
same small JSON object. Body prefixes place selective/rare/common matches in
the first1/1024,1/64,1/2 of logical rows. Each body contains UTF8 lambda and a
literal bracket. Wide filler repeats a deterministic random64-character hex
string: compressible synthetic data, not a production log-corpus claim.

Four arms execute the same six shapes: absent, selective, rare, common, all,
and an all-text time window spanning1/16 of the timeline. Each returns at most64
rows in `(observed_ns,node_id,sequence,index)` order. One first call and three
process/connection reuse calls yield1536 measurements across16 children. Arm
order rotates by query/call/fresh repetition; no OS cache is flushed. UTF8 lambda,
literal bracket and literal `.*` engine controls run separately from timings.

The arms are PyArrow full Parquet decode; PyArrow conservative timestamp bounds
plus exact per-row-group byte-trigram sets followed by exact verification;
DuckDB direct Parquet; and DuckDB after persistent-table ingestion. The exact-set
index is an explanatory lab mechanism, not Fabric's Bloom-filter implementation.
Short needles conservatively skip trigram pruning. Construction records source
generation, Arrow conversion, Parquet writing, index construction and DuckDB
ingestion and its explicit checkpoint separately. Source-specific repayment rows
charge Arrow pruning for index construction plus serialization/readback and
DuckDB table queries for ingestion plus checkpoint. For every query, metric and
fresh repetition, divide that incremental cost by the observed reuse-median
saving; nonpositive savings have no finite repayment. This is an illustrative
fixed-query model, not a refill/update or steady-state amortization measurement.
Repeated query totals can expose amortization but do not imply a universal
break-even or production winner.

Every result is compared to source tuples using independent Python half-open
window/literal membership filtering, explicit ordering and slicing. Checker
controls must reject missing/duplicate/changed/out-of-order rows, an upper-window
boundary row, and a false-negative pruning defect. Parquet full readback compares
all physical tuples exactly. This new checker does not replace or alter the
existing delivery/query oracle. Native Fabric Scan/Walk are omitted because the
unchanged completion probe cannot admit these matched query/locality fixtures;
unmatched native numbers would not establish a causal comparison.

Record plain library-query wall/process CPU separately from Python result
projection, plus their sum. Arrow timing includes Python candidate selection and
Arrow decode/filter/sort;
DuckDB timing includes SQL execution through Python bindings, while fetch and
tuple materialization are separately recorded. Whole-child time/CPU/RSS includes
construction, controls and grading; per-query RSS high-water is cumulative.
Proc IO counters are OS observations, not decoded bytes. Arrow selected-group
compressed-column sums describe candidates, not measured physical reads. Native
DuckDB decoded-byte counts are unavailable. Explain plans are retained for the
first query per DuckDB arm without profiling the measured calls. No allocation
ownership, cold/warm, service-memory or stable engine-dominance claim follows.

Pinned external artifacts, verified against PyPI release metadata before
preparation, are `duckdb==1.4.3` cp314 x86_64 manylinux2_26/2_28 wheel SHA256
`1b35491db98ccd11d151165497c084a9d29d3dc42fc80abea2715a6c861ca43d`
and `pyarrow==22.0.0` cp314 x86_64 manylinux2_28 wheel SHA256
`6dda1ddac033d27421c20d7a7943eec60be44e0db4e079f33cc5af3b8280ccde`.
Download URLs and exact lengths are frozen in the script/environment receipt.
UV's actual tool version is recorded; installation uses `--target` on owned
data-drive scratch, `--no-deps --no-index --require-hashes`. No production
dependencies or ambient packages are installed. Engine/library thread counts
are1. The outer launcher supplies20GiB no-swap containment; the sweep restricts
its inherited CPU affinity to the first two available CPUs before dependency
preparation and all children, verifies it, and records inherited/effective
affinity. This is an affinity bound, not a claim about `cpu.max` quota.

Proposed command after the owner's separate registration:

```sh
python3 tools/resource_group.py -- python3 tools/bench/labs/cross_system/run_sweep_job.py --id query-sweep-01 --lab query --reserve-mib 128 --seconds 1200 -- python3 tools/bench/labs/cross_system/query_sweep.py --out docs/experiments/benchmarks/data/cross-system-sweep-01/query/sweep-01 --seconds 1100
```

The worker deadline includes dependency preparation and grading. New retained
evidence is bounded100MiB, scratch8GiB, free data-drive reserve16GiB. Evidence
includes exact compressed logical source, deduplicated Parquet and trigram
objects, source archive/diff, wheel pins, commands, metrics, deduplicated exact
compressed answers with checker verdicts, construction receipts and process
observations. Successful ingested
DuckDB files are hashed but removed: they are reconstructible from retained
Parquet, DDL and pinned wheels. A failure retains the full owned scratch including
database, intermediate fixtures, install state and partial metrics. Clean only
after exact readbacks and semantic checks; an interruption remains interrupted.

The discriminating hypothesis is that locality changes candidate group counts
and exact query costs, and repeated-query work can repay construction costs in
some configurations. The null is that pruning fails to repay construction or
survives neither mixed locality nor less-selective predicates. Compare all shapes,
both fresh repetitions and construction cost rather than selecting a favorable
median. Any later nomination needs a separate prospective confirmation protocol.
