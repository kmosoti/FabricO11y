# C4 concrete attribute-associated attribution preparation

Status: executable draft; no builds/tests/controls/measurements run here.
Root registers this supplemental initial scope before execution. Previous CR2
null/guards, frozen binaries/harness and the independent oracle stay unchanged.
C5 remains deferred behind O6/O7 resolution and separate resource admission.

Source evidence: `completion_query_probe.fixture` constructs empty attributes
and has no attribute knob; old `borrowed_log.py` fixes H65536/limit10000, so no
existing command executes the proposed rich/small diagnostic correctly.
`rows::extract` spans OTLP decode/projection, not Parquet attribute JSON parsing.
`segment::attrs_of` uses the existing serde JSON parser in both owned/borrowed
readers. Both validate every in-window row before query predicate rejection.
Current `parquet_load_project_logs` includes decode, parse AND consumer callback;
existing spans cannot identify isolated parse CPU. Tail already borrows.

New files only: `examples/coupled_borrowed_probe.rs` and
`tools/bench/labs/catalog/coupled_borrowed.py`. Real Store/Segment/History paths;
compile borrowed mode0/1, current production semantics, no new parser/unsafe
optimization. Existing benchmark-only System allocator and CPU-clock sampling
are copied. No old driver, oracle, registered gate or production source edits.

## Fixed initial diagnostic

Root reduced initial8192-row preparation to **4096 rows** for budget/evidence
admission;8192/65536 remain conditional later registrations. Seed42, shuffled
unique timestamps,1024-byte bodies,128 logs per Batch,32 Batches/Groups in one
Segment, empty journal. No Intake bootstrap/replay assumption; committed query
frontier32 is synthetic and supplied explicitly. No service ACK/Spool claim.
Attributes are either empty or eight string values of128ASCII bytes each, with
keys attr-0..7. Each per-key value is deterministic from seed42 and identical
across rows: deliberately compressible/dictionary-friendly, not an entropy or
cardinality stress. This changes encoded and compressed bytes; record both
logical body/attribute sizes and actual encoded Batch/storage sizes, never call
these byte-matched workloads. Body/identity/timestamp data are held fixed.

Selective text needle-C4 matches one row/128 (32 hits); broad omits contains.
Walk primary and Scan reference, limit50: selective1page, broad82pages. One
untimed warm-up then3 measured first pages per plan/shape. Measurement includes
History.run and JSON Value construction; external serde byte serialization and
continuations are excluded. Exact bytes of every actual first page must equal
its independently graded canonical first page. Four complete canonical drains
per child,12 children=48 complete chains and144 measured associations; this is
not144 independently executed drains. The unchanged oracle grades raw Batches,
attributes/order/pagination; truncated and duplicate-row controls must reject.

One fresh owned/borrowed pair per attributes/regime: plain, counted phase-off,
counted phase-on (12 children total). Rich order reverses empty order. This is a
diagnostic screen, not the three-fresh-pair confirmation or a speed nomination.
Run existing rejected-row malformed-attribute/identity/column unit control
before measurements. Candidate never skips attrs validation to reduce cost.

H1a: rich attributes amplify unchanged inclusive projection cost, eroding the
ownership CPU benefit. H1b: decoding/output or heap work dominates even empty
attributes. H0: paired differences or observer effects cannot distinguish these.
Report requested/live/peak allocations, CPU/wall, encoded bytes and IO; retain
inclusive nesting, not summed nested wall/CPU. Direct-child CPU subtraction
retains an explicit unattributed residual; negative residual beyond max1us/1%
of inclusive total invalidates attribution. Per-phase record cap8192; no new
per-row spans (shared ledger cap262144 would otherwise overflow).
Observer admission requires phase-on/off CPU,wall AND requested bytes ratios
in[0.95,1.05] for each matched population. A miss invalidates causal phase
conclusions, not semantic collection. Empty/rich delta is attribute-associated
(decode+validate+callback/output), never an isolated serde-parse measurement.
At most one next hypothesis follows; no CPU improvement is inferred from bytes.

## Executable admission

Estimated screen150–450s including four builds/semantic grading; this is a
prediction, not measured runtime.800s driver inside900s coordinator. Root
reviews actual first children before expansion; no known overprojection runs.
Persistent per-run cap48MiB includes binaries/objects and16MiB failure headroom,
checked as max(unique inode file bytes,allocated blocks), periodically250ms
while children run. Pre-child projection reserves4MiB new retained objects per
attribute setting plus1MiB cell overhead and16MiB failure reserve; hard guards
can stop incomplete collection. Actual failure archives must still fit root's
prospective832MiB capacity/2GiB aggregate/wave192MiB envelope; no silent waiver.
Four frozen archives get decoded-SHA readback; only one executable is restored
per child. On failure, exact duplicated scratch binaries are removed ONLY after
matching persistent archive readback, preserving hashes and failed fixture data.
Raw rich answer dumps stay on mounted scratch; canonical objects are exact-byte
verified and gzip-deduplicated persistently. Success scratch is removed.
Keep16/20GiB RAM,zero swap,8GiB scratch,16GiB drive reserve; evidence on repository
/home and build/temp on mounted data drive. Dataset prefix is mandatory for
capacity inventory: `catalog-borrowed-attribution-*`.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-coupled-c4-01 --lab memory --stage capacity --seconds 900 -- python3 -B tools/bench/labs/catalog/coupled_admit.py --reserve-mib 48 --capacity-reserve-mib 48 -- python3 -B tools/bench/labs/catalog/coupled_borrowed.py --out docs/experiments/benchmarks/data/catalog-borrowed-attribution-run-01
```

Root handles registration, prospective aggregate accounting and serial execution.
