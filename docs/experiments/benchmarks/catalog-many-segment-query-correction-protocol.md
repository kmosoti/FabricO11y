# CR3 canonical broad-query fixture and adapter correction

Status: draft; root registers separately before new freeze or measurements.
Original CR3 protocol, freeze01 and the failed first pair remain unchanged.
The unchanged independent oracle rejected first64Segment/1reader/plain clone
broad query with `MALFORMED: query: field 'contains' must be str`.
Origin: coordinator `catalog-many-s-64-r-1-plain-p-1`; preserved run
[data/catalog-many-segment-run-01/S-64-R-1-plain-p-1/failure.json](data/catalog-many-segment-run-01/S-64-R-1-plain-p-1/failure.json)
and original clone/timings.json. The failed pair is not successful evidence.

[Minimal deterministic fixture](fixtures/catalog-many-segment-contains-null.json)
records seed42,65536rows,64Segments,1reader and the exact malformed/corrected
queries. Native broad JSON previously included `contains:null`. Rust's Option
accepts both null and omission as None, but the independent oracle's canonical
query contract requires an optional predicate to be omitted or a string.
This is fixture/adapter serialization correction, not a query semantic change.

Native broad JSON now omits `contains`; absent still has the exact string
`__CR3_ABSENT__`. Driver semantic grading and retained-artifact association
expectations follow the same omission. The oracle, its MALFORMED rule and
expected rejection remain unchanged. Aggregate's pure association controls
accept corrected broad omission and must reject reintroduced contains:null.
Root's explicit zero expected/answered-row check for absent is preserved.
Counterexample origin and this correction protocol are included in new freeze
source provenance. No other fixtures, seeds, sizes, limits, layouts, demands,
metrics, pairs, primary thresholds or guards change.

Freeze02 under a fresh runroot, after registration/format and root controls:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-many-segment-freeze-02 --lab memory --stage capacity --seconds 600 -- python3 -B tools/bench/labs/catalog/many_segment.py --stage freeze --out docs/experiments/benchmarks/data/catalog-many-segment-run-02/freeze
```

Rerun every registered cell against freeze02. Admit first64/1/plain pair1:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-many-s-64-r-1-plain-p-1-r02 --lab memory --stage capacity --seconds 600 -- python3 -B tools/bench/labs/catalog/many_segment.py --stage pair --freeze docs/experiments/benchmarks/data/catalog-many-segment-run-02/freeze --segments 64 --readers 1 --build plain --pair 1 --out docs/experiments/benchmarks/data/catalog-many-segment-run-02/S-64-R-1-plain-p-1
```

Remaining23 exact cells retain the original1/64 x1/4 x plain/count x3pair
matrix and alternating order, with unique coordinator IDs ending`-r02` and
sibling directories under run02. No old timings or grader receipts are reused.
Aggregate only run02's24 cells with run02/freeze and a new summary/coordinator
ID. All96 canonical chains and3072 actual first-page associations remain
required. Root admits the matrix against measured corrected first-pair runtime,
remaining capacity-stage/shared budgets and reviewed persistent evidence caps;
768MiB capacity and2GiB total catalog allocation include failedrun01 evidence.
RAM/no-swap/scratch/free-reserve/cleanup requirements stay unchanged.

No builds, controls, tests, validators or workloads ran during correction prep.
