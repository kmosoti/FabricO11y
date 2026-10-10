# O7 additive native pruning soundness diagnostic

Status: registered before building/measuring, under completion dispatch. No historical C2 criterion, oracle or result changes.
The archived bigrows `pruning_equal=false` remains failed even if this diagnostic
passes. This tests conservative selection and exact answers, not identical
physical admitted-row counts or actual read IO.

H1: actual bounded Parquet footer ranges conservatively cover every independently
decoded row identity, and selected groups cover every matching identity in every
registered window. H0: bounds/selection omit a matching row, producer identities
or content drift, or a complete query chain disagrees with the existing oracle.

The new `coupled_pruning_probe` creates 1100 logs with 16KiB bodies in 18 manually
framed Groups (64 logs/Batch except the last), node ID07 repeated16bytes, monotonic
Batch sequences, unique body prefixes. The fixed fixture has first arrival time
T+10, final late arrival T+1, and all remaining rows T+5, where
T=1800000000000000000. This forces equal-time rows across real byte-capped group
boundaries independently of their precise cardinality. It contains logs only;
metrics/spans and service contention are outside this correctness diagnostic.

Build uses the real `segment::build_sealed` at default16MiB, 8MiB output chunks,
unchanged ZSTD3, FAN_IN16 and spill codec. Decode each actual Parquet group and
record group index, footer min/max, node identity, sequence/index, time and body
SHA. Read footer ranges through the real `segment::row_group_bounds`. Preserve
producer Batch bytes before IO; Python wire-decodes them independently. Require
at least two groups, exact globally sorted identity/content partition, conservative
bounds, and at least one actual tied boundary. Bounds may be wider than actual
extrema; narrower bounds fail. No zero-match suppression of admitted-group counts.

Windows are deduplicated/sorted: [T-1,T), [T,T+11), [T+11,T+12), plus
[min,min+1) and [max,max+1) for every actual group. Require empty and1100-row
broad windows. For each window, record all footer-admitted groups using
max>=from and min<to, actual matching rows and hypothetical admitted cardinality.
Run complete Walk AND Scan chains at limit50 and actual committed Group18;
maximum24 pages/chain. Grade producer records/query/pages using the unchanged
Python query oracle. Preserve all chains and producer records with exact gzip
SHA readbacks. Native verifies the manifest/table hashes before readback.

Inject four independent-checker defects: omit an admitted group, narrow a bound
past a real row, remove a physical group, change row body hash. Every control must
be rejected. For each broad chain also require the existing oracle to reject a
duplicated row and a truncated chain. Extra reads remain visible and do not fail
this prospective soundness diagnostic; they do not clear the original gate.

## Commands and resource admission

Root chooses fresh job IDs and source freeze; no builds/validators outside the
launcher. Compile selector is explicit16; all other experimental selectors unset.
Existing target/build paths come from the coordinator and owned data drive.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-coupled-o7-native-build-01 --lab recovery --stage recovery --seconds 300 -- env -u FABRIC_SPILL_WORKSPACE_EXPERIMENT -u FABRIC_BORROWED_LOG_EXPERIMENT FABRIC_RUN_MIB_EXPERIMENT=16 cargo build --offline --locked --release -p fabric-server --example coupled_pruning_probe
```

Freeze/copy the resulting `$CARGO_TARGET_DIR/release/examples/coupled_pruning_probe`
to an owned data-drive location and record SHA. That path is `$O7_FROZEN_BIN`
below; this variable is a root-selected path, not a new native CLI option.
Root must prospectively resolve evidence attribution/admission before running:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-coupled-o7-native-01 --lab recovery --stage recovery --seconds 240 -- python3 -B tools/bench/labs/catalog/coupled_pruning_native.py --bin "$O7_FROZEN_BIN" --out docs/experiments/benchmarks/data/catalog-coupled-o7-native-01
```

Driver checks real containment first; fixed20MiB evidence cap includes8MiB failure
headroom, live250ms checks, scratch8GiB and free16GiB. Outer20GiB/no swap remains.
Expected fixture scratch below256MiB; gzip evidence expected below12MiB including
binary. These are forecasts, not measurements. Native deadline180s; expected
small diagnostic runtime tens of seconds, build<=300s. Persistent evidence stays
on repository disk; build/temp fixtures stay on mounted data drive. A new allocation
or approved mandatory inventory assignment is needed; do not silently reuse
capacity's near-full832MiB allowance. Failure scratch stays for root's verified
archive/cleanup. Success deletes owned fixture scratch and records cleanup.

Result includes exact verdicts/counts, negative controls, bound/window identities,
source/binary hashes, command and archive readbacks. Summary success means only
these finite checks ran. No O6 availability waiver, C2 rehabilitation, deployment
qualification, sustainable-rate claim or C5 performance nomination follows.

## Separate C5 selector controls

`FABRIC_RUN_MIB_EXPERIMENT` is now an experimental compile-time selector accepting
only8/16/32, unset16; invalid strings fail constant evaluation. Root should run
`cargo test --offline --locked -p fabric-server --lib experimental_run_limit`
inside a separately registered contained job at each value and capture selector
build provenance. The test covers all valid values/invalid parsing, byte-driven
spill and oversized-row exception; existing bounded merge/order/custody controls
remain required. C5 harness and performance registration are separate and pending.

Root admission assigns this diagnostic to operations, under its20MiB retained
reservation (including8MiB failure headroom); it is not a capacity dataset. Build
uses a4MiB global overhead reservation. Execution uses a20MiB global reservation
via `coupled_admit.py`, with zero capacity reserve. The native executable will
be read directly from the data-drive build cache after its captured build; no
other build may overwrite it until execution and its exact gzip archive finish.
This is a source/binary freeze, not a mutable name as identity: the driver records
the actual binary hash and retains decoded/hash-verified bytes.
