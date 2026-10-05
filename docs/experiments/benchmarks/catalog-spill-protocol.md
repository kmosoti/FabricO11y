# CR1: reusable private spill workspace

Status: **registered for execution; no outcome claimed**.
Parent: [capacity packet](catalog-labs-capacity.md). Scope: an isolated codec
roundtrip screen, not full sealing, deployment qualification or service capacity.
No production code, durable format, core dependency, oracle or C2 gate changes.

H1: bounded reusable byte workspaces reduce allocation traffic and observer-off
CPU/wall while preserving exact bytes and bounded retained capacity. H0: owned
row/string/map allocation dominates, or workspace retention/overhead erases gains.
Baseline calls the actual private `spill::read_row/write_row`, included by path.
Three independent comparisons vary encode reuse, decode reuse, or both. Candidate
uses safe owned `Vec<u8>` only; decoder rows own all strings/maps before reuse.
It preserves the existing 16 MiB row ceiling. Maximum reusable capacity is
256 KiB per reader/writer; larger rows use discarded temporary buffers. Actual
capacity is checked, not assumed from requested lengths. This models one stream;
sixteen merge readers would require sixteen charged workspaces.

## Frozen fixture and controls

Seed 2703163393, exactly one generated input targeting at least 64 MiB of framed
log rows. Bodies are 1,024 ASCII bytes alternating repetition and xorshift13/7/17
entropy. Every row includes Unicode/null attribute keys and escaped values;
node identity, sequence/index and time are deterministic. Hash actual input bytes
and retain row count/byte count; use the same file for every variant and binary.
Input is generated into disk-backed owned scratch and remains warm. No cache
eviction or filesystem filling. Output files are fresh and removed after grading.

Both binaries first run controls: maximum legal 16 MiB serialized String row,
then small/empty rows; exact encoding equality and retained capacity <=256 KiB;
oversized encode and framed decode rejection; malformed lengths/string fields,
every nonempty truncation of a small valid frame, EOF; explicit independent
little-endian signed-integer and NaN-payload bytes. All must reject/preserve as
specified. Full runs decode every log row and reencode it; full-file SHA-256 must
equal the original for all variants. This checks every represented field and
order, not only a sample. Any control failure stops collection and is retained.
Runner gate controls require accepting an unchanged digest pair and rejecting
changed input/output digests and a false exactness flag before measured children.

## Build and run

Root registers this protocol before execution and captures exact commands,
revision/diff, build settings and binary/source hashes. Existing
`responsibility-alloc-probe` selects benchmark-only allocator instrumentation;
its unsafe `GlobalAlloc` implementation delegates pointer/layout validity to
`System`, as prior probes do. Candidate algorithms contain no unsafe code.
No counted result is substituted for observer-off performance.

Run all commands through `python3 -B tools/resource_group.py -- ...`.
Prepare a fresh owned directory under the mounted data drive for frozen binaries.
Build then copy the plain example there before building the counted variant:

```sh
cargo build --offline --locked --release -p fabric-server --example catalog_spill_probe
cargo build --offline --locked --release -p fabric-server --example catalog_spill_probe --features responsibility-alloc-probe
python3 -B tools/bench/labs/catalog/spill.py --plain /run/media/kmosoti/data/FabricO11y/scratch/CR1-BINARIES/plain --counted /run/media/kmosoti/data/FabricO11y/scratch/CR1-BINARIES/counted --out docs/experiments/benchmarks/data/catalog-spill-run-01
```

The two build lines are separate contained commands; between them copy the first
binary from `$CARGO_TARGET_DIR/release/examples/catalog_spill_probe` using a
contained command, then copy the counted binary likewise. Root replaces
`CR1-BINARIES` with its fresh owned binary directory and records copies/hashes.
Never overwrite frozen baseline binaries from an earlier campaign.

## Measurements, decision and accounting

Three alternating fresh-process pairs for each candidate, separately plain and
counted: 36 measured children total. Codec phase wall includes owned decode,
encode, writes and final buffered flush; excludes generation, startup, hashing
and sync. External child CPU/wall includes startup and exact file readback;
report that boundary explicitly. Successful allocator calls count alloc,
alloc_zeroed and realloc; cumulative bytes count full successful requested sizes
(including realloc), peak/live use requested layouts, not RSS or allocator pages.
Reset counters after IO buffers exist; snapshot before readback, report retained
encode/decode capacity and live heap before/after workspace drop. No allocator
RSS reclamation claim follows from dropping vectors. Child block IO is retained;
logical final output bytes are constant. Warm-page-cache IO is not device cost.

Nominate a candidate only if all byte/control gates pass and either allocator
calls or cumulative requested bytes decrease >=15% in all three pairs, while
plain median whole-child CPU and phase wall each regress no more than 5% against
their paired baselines. Three pairs are a nomination screen, not statistical
qualification. Report each pair and contrary evidence, not just averages.
Workspace capacities may never exceed256KiB each; compare incremental peak and
retained heap separately, without redefining the historical builder heap gate.
Counterexamples include large-then-small retention, stale trailing bytes, changed
EOF/error behavior, and owned decode costs remaining unchanged despite reuse.

Resource budget: <=45 minutes total including builds, controls, validation and
cleanup. Every child has60s timeout; generation/controls120s. Default outer
20GiB/no-swap/deadline enforcement remains mandatory. Scratch <=1GiB, free drive
reserve >=16GiB checked before admission. Root records whole-job cgroup/CPU and
cleanup receipts; runner retains failures, raw stdout/stderr, exit codes and
provenance, removes successful outputs and final owned fixture directory.
Root removes owned frozen binaries after archival. No uncapped fallback.
The concrete first-run command builds and freezes both variants within one
contained job, invoking the two build commands above in order and preserving
compressed binaries in the result directory:

```sh
python3 -B tools/resource_group.py -- python3 -B tools/bench/labs/completion/run_job.py --id catalog-spill-01 --lab memory --stage capacity --seconds 1500 -- python3 -B tools/bench/labs/catalog/launch.py spill --out docs/experiments/benchmarks/data/catalog-spill-run-01
```

Spool/ACK/query/backlog metrics are absent by design: any nominated lever needs
the capacity packet's native fixed-demand composite before a service claim.
