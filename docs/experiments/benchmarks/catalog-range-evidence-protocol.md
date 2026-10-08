# Timestamp reduction without query-row construction

Registered before execution. This is a finite, local performance ablation and
correctness investigation under the existing catalog campaign. It targets the
snapshot boundary fallback introduced by the [ownership transition
investigation](catalog-transition-ownership-protocol.md). No custody, wire,
retention, oracle, qualification or persistent-format change is proposed.

## Model and alternatives

For a boundary Segment, let B be all raw Batch bytes, N its records, Bi the
included bytes, Ri the included observations, F their node cardinality, and P
the number of query pages. The current approximate cost is:

`P * [open + parquet(B,N) + hash(B) + copy(B) + decode(Bi) + project(Bi,Ri) + reduce(Ri,F)]`.

The candidate retains the same raw reader, digest checks, Batch/node-ID
validation, and fully typed Prost decoding. It computes supported timestamps
directly from decoded trees, removing only query-row construction: row vectors,
attribute maps, row identity/name copies, and span hexadecimal formatting.
The null is that this removal offers no material whole-boundary improvement,
because reading, hashing and typed decoding dominate. The alternative is a
measurable reduction in time and allocation without changing exact evidence.

Reading projection columns alone and caching prior integrity results are
rejected for this experiment: neither preserves detection of a corrupt excluded
raw record. Min/max evidence is mergeable but not invertible; a whole-Segment
maximum cannot be subtracted to obtain an arbitrary snapshot range. A skinny
protobuf parser is deferred because skipping nested validation can change
malformed-input behavior. No novel-algorithm or universal-speedup claim follows.

## Frozen screen and semantic controls

Seed 42, four deterministic cells. Each Entry contains four logs with eight
string attributes, two Gauge and two Sum points, and two spans. Body generation
and compressibility remain identical between arms; fixtures are constructed
outside measurement. An unsupported Histogram carries a dominant timestamp.

| Cell | Entries N | Bytes/log body | Included groups | Nodes F |
| --- | ---: | ---: | --- | ---: |
| A | 16 | 128 | 1 through N/2 | 1 |
| B | 16 | 4096 | 1 through N/2 | 1 |
| C | 64 | 4096 | 1 through N/2 | 1 |
| D | 64 | 4096 | N/8+1 through N | 8 |

Three pairs per cell run baseline/candidate, candidate/baseline,
baseline/candidate, with sixteen repetitions per arm. Measure separately the
in-memory decode/reduction kernel and the whole boundary scan through the same
public `segment::scan_batches` API. Both arms use its default Arrow batch size;
these fixtures have at most 64 Entries. This controls reader differences but is
not a memory comparison of the production one-row reader, a cold-cache test,
or an HTTP/end-to-end query throughput claim. Report normal warm-cache results;
do not drop host caches. A second build with the existing allocation probe
feature measures requested bytes, allocation calls and incremental live peak;
its timings are diagnostic, not promotion evidence. Record per-arm wall and
process CPU nanoseconds, included counts, raw bytes hashed, exact output and
source/binary hashes. Requested heap bytes are not RSS.

Each arm must equal fixture arithmetic for receive bounds and per-node
freshness, independently of arm equality. Controls cover no included records,
a middle interval with both bounds, gap-only/empty and timestamp-zero records,
unsupported metric exclusion, malformed envelope/node/logs/metrics/traces and
decode-error order. Missing or corrupt projected files must not change raw
evidence. Excluded malformed OTLP is not decoded, but an excluded raw digest
mismatch must reject the scan. Digest controls must alter the stored digest
column; arbitrary footer damage alone does not establish this property.

The operations lab also supplies independent known-value Rust regressions,
including invalid nested strings in ignored fields. The existing full-Segment
borrowed-manifest control and unchanged snapshot-transition integration oracle
remain required if the candidate enters production. Query-chain checks cover
exact rows, receive bounds, freshness, completeness, publication/restart and Gone.

## Decision and execution

Nominate only after every exactness/control check succeeds, requested allocation
falls in every cell in both phases, median paired whole-scan wall time improves
by at least 10% in at least two cells, and no plain phase/cell regresses more
than 5%. Report all three paired ratios and CPU results, including disagreeing
or noisy cells. A rejected screen stays rejected; no post-hoc cell removal.
These thresholds justify only this narrow implementation choice. On nomination,
move the same reducer into `rows::latest_observation_ns`, make boundary evidence
call it, and rerun the probe using that production function plus the independent
Rust regressions and existing complete query-chain oracle. Other query paths and
fully included manifest evidence remain unchanged.

The query lab authors the isolated probe, the capacity lab checks the numerical
summary, and operations supplies adversarial controls. Root serializes execution,
reviews source and integrates only supported changes. No more than three helpers.

| Job | Stage | Deadline | Reserve after coordinator snapshot | Driver cap |
| --- | --- | ---: | ---: | ---: |
| `catalog-range-evidence-screen-01` | preparation | 300 s | 3 MiB | 3 MiB |
| `catalog-range-evidence-fixed-01` | preparation | 240 s | 3 MiB | 3 MiB |
| `catalog-range-evidence-checks-01` | verification | 320 s | 1 MiB | 512 KiB |

Only run the fixed job after nomination. Each command runs through
`tools/resource_group.py`, `completion/run_job.py`, and `catalog/coupled_admit.py`.
The final job runs `cargo xtask checks --profile fast` and manual documentation
checks with their real receipts. Existing frontier/stage/evidence budgets are
not reset: approximately 1030 seconds frontier and 322 seconds verification
remain at registration. A timeout remains interrupted, never passed.

Use only the mounted data drive, memory high 16 GiB/max 20 GiB, no swap, and the
existing outer deadline. Preserve all small fixture files, sources, stdout,
stderr, command exits, raw samples and failure traces; verify archive coverage,
lengths and exact decoded file bytes before removing owned scratch. The decoded
fixture-file cap is 2 MiB per job (compressed Parquet counts its actual file
length, not logical OTLP payload size). Existing altered/missing/duplicate
archive controls must reject. The numerical checker rejects malformed sample
populations and output mismatches; a plausible slow timing must reject
nomination, not be misrepresented as detected evidence tampering. Bind raw
output hashes separately. Keep originals on an unexpected preservation failure.

No remote workload, release, qualification, privileged installation or
destructive fault run is included. Findings must distinguish eliminated row
construction from the still-present raw IO, hashing and typed decode costs.
