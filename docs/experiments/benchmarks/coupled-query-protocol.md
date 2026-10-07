# Q4 descriptor reuse initial screen

Status: registered before freeze/measurement. Five
candidate controls are registered separately; their actual receipts decide
admission. [Preparation](coupled-query-preparation.md) describes the mechanism.
H1: reuse reduces total native schedule CPU; H0: repeated listings/tail work or
retention costs erase savings. One pair is a screen, never nomination.

Baseline and candidate both use Walk and shared Manifest handles. Candidate alone
uses `with_descriptor_reuse`; default constructors/Scan remain unchanged. Fresh
label checks and tail indexing remain. Byte charge is estimated conservatively,
not an allocator guarantee; hard structural/holder caps and exact overflow
fallback are separate. No file lease, durable map, background refresh or wire change.

Freeze one current-source plain and one counted example binary. Borrowed log
selector is compiled with value0. Counted feature is responsibility-alloc-probe;
phase instrumentation is absent. Both runtime arms use each same frozen binary.
Source hashes and gzip decoded/hash readbacks are retained once. Root captures
the working-tree source archive; never substitute HEAD provenance.

Fixture deviation, frozen prospectively to meet16MiB incremental evidence:
seed42/H8192,256-byte bodies,64 Segments of128 rows/one Batch each, followed by
four durable128-row appends. Tiny preflight uses H128/one Segment. Chronological
observed timestamps START1600000000000000000+i; even body pads R, odd uses
xorshift64 printable ASCII; node[7;16], generation1, empty attributes. Python
reconstructs every body independently and checks actual replayed Batch bytes.
The CQ2 correction seals seed journals before publishing any seed Segment.
Final recovered ledger must contain68/5 Groups and8704/640 rows exactly.

Native schedule alternates broad/selective `bench-0007 `, one query per ACK,
limit1000. Prime occurs outside timing. Total process CPU/native wall includes
all four ACKs, lazy acquisition, query, serialization and bookkeeping. Four
measured first pages plus two quiet final pages and two missing/corrupt optional
filter pages each drain completely outside schedule timing. Fresh Histories
execute the optional-filter controls; cached verified filters cannot mask them.
All eight actual first pages per child are associated byte-exactly with chains;
unchanged Python oracle grades their actual committed-prefix ledgers. Six
negative controls per child alter/miss/duplicate rows and archive framing/chunks.
Do not omit retained-window, freshness, gaps, continuation or completeness checks.

Three separately admitted slices: tiny plain preflight pair,64-Segment plain
pair1 lazy then reuse, and counted diagnostic pair. Six trials/48 complete chains;
no three-pair conclusion. Initial total ceiling1200s includes controls/builds,
native work/grading. Each driver child has explicit shared deadline; builds
also have a600s ceiling bounded by the admitted job deadline. Cgroup launcher
retains20GiB/no-swap and8GiB scratch. No Spool/forwarding throughput claim.

Concrete driver commands below are runnable only after root registration and
fresh candidate controls. Root serializes the equivalent coordinator jobs.
All dataset directories use catalog-boundary-reuse-*; frozen archives are counted
once, decoded binaries live only in owned FABRIC_SCRATCH_ROOT. Complete native
JSON is reconstructed byte-for-byte from shared64KiB compressed row chunks,
with native SHA/size, whole-envelope framing and incremental verdict receipts.
No semantic normalization. Slice account lists must include every closed Q4
slice plus freeze/object pool; original catalog aggregate caps also apply.

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/catalog/coupled_query_freeze.py --out docs/experiments/benchmarks/data/catalog-boundary-reuse-freeze-01 --seconds 300
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/catalog/coupled_query.py --freeze docs/experiments/benchmarks/data/catalog-boundary-reuse-freeze-01 --objects docs/experiments/benchmarks/data/catalog-boundary-reuse-objects-01 --account docs/experiments/benchmarks/data/catalog-boundary-reuse-freeze-01 --out docs/experiments/benchmarks/data/catalog-boundary-reuse-preflight-01 --rate 1 --slice preflight --cap-mib 16 --reserve-mib 2 --seconds 300
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/catalog/coupled_query.py --freeze docs/experiments/benchmarks/data/catalog-boundary-reuse-freeze-01 --objects docs/experiments/benchmarks/data/catalog-boundary-reuse-objects-01 --account docs/experiments/benchmarks/data/catalog-boundary-reuse-freeze-01 docs/experiments/benchmarks/data/catalog-boundary-reuse-preflight-01 --out docs/experiments/benchmarks/data/catalog-boundary-reuse-pair1-01 --rate 1 --slice pair1 --cap-mib 16 --reserve-mib 6 --seconds 300
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/catalog/coupled_query.py --freeze docs/experiments/benchmarks/data/catalog-boundary-reuse-freeze-01 --objects docs/experiments/benchmarks/data/catalog-boundary-reuse-objects-01 --account docs/experiments/benchmarks/data/catalog-boundary-reuse-freeze-01 docs/experiments/benchmarks/data/catalog-boundary-reuse-preflight-01 docs/experiments/benchmarks/data/catalog-boundary-reuse-pair1-01 --out docs/experiments/benchmarks/data/catalog-boundary-reuse-diagnostic-01 --rate 1 --slice diagnostic --cap-mib 16 --reserve-mib 2 --seconds 300
```

Review raw total/query/serialization/ACK timings, counted phase allocations and
outside-timing cache stats separately. Hits/builds are cumulative including prime
and quiet controls, not only timed queries. Selected index vectors add at most
256*sizeof(usize) bytes per admitted holder outside the generation charge. Record
native first-page sample count4, entire chain latency and coordinator grading
elapsed separately. Proposed candidate recommendation is revise/defer/confirm
only; common10%/three-fresh-pair rule and memory/latency guards still govern any
later nomination. Stop admission on failed exactness, controls, containment,
time/disk/evidence limits. Preserve failures before cleaning owned scratch;
success cleanup must remove only temporary files, never evidence or build caches.

## Coordinator admission and object ownership

Root runs the four listed commands as child commands of `run_job.py`, under
`resource_group.py`, using IDs `catalog-coupled-q4-freeze-01`,
`catalog-coupled-q4-preflight-01`, `catalog-coupled-q4-pair1-01`, and
`catalog-coupled-q4-diagnostic-01`, lab query/stage query, 330 seconds each
(driver deadline 300 seconds). The cumulative actual ceiling remains 1,200s;
requested deadlines do not authorize additive time beyond remaining stage/frontier.
The admission helper reserves 8/2/6/2 MiB aggregate respectively, capacity reserve
zero, and propagates child exits. No run starts while another is active.

The complete evidence auditor adds exactly three owned directory aliases:
`catalog-boundary-reuse-{preflight,pair1,diagnostic}-01/objects` point to the
real `catalog-boundary-reuse-objects-01` directory. They must pass the existing
strict target/independent-inventory/no-internal-links checks; unlisted aliases
remain rejected. The 16 MiB Q4 unique/block allowance includes freeze, all closed
slices and pool; the driver account lists are explicit. A failed reserve or
fixture/negative control stops subsequent slices. New IDs/protocol registration
are required for any retry; original result states are never rewritten.
