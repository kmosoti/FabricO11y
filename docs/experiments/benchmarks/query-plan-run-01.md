# Query-plan comparison and allocation run 01

Status: native comparison completed under the [registered protocol](query-plan-protocol.md); allocation study inconclusive.
This is a local finite investigation, not deployment qualification or a default-plan decision.

## Provenance and lab ownership

Protocol registered in `6c126dc`; independent supplemental auditor in `0144bc0`.
The [dispatch record](data/query-plan-run-01/dispatch.json) records three lab
responsibilities, estimated delegated allowances and unavailable actual token telemetry.
The inline coordinator serializes workloads; capacity PI prepares profiling and
Luna assists the query/recovery review. Each job archives its exact source tree,
working-tree diff, protocol, command, exit and cgroup observations under
[data/query-plan-run-01/coordinator](data/query-plan-run-01/coordinator).
Native binary identities are frozen for the campaign. Native trial sources differ
only in profiling-harness corrections between the first development trial and
subsequent trials; that profiling helper is not executed by native trials.

## Allocation study: inconclusive

The first tiny preflight rejected an incorrect harness assumption that generated
fixture records and recovered records are byte-identical. The second attempt
retained per-Batch bytes and framing losslessly but incorrectly required tail and
Segment receipt timestamps to match. Source inspection established that the tail
uses Intake (actual receive time), while Segment construction uses fixture Groups
(fixed receive time). Both plans read the same recovered ledger within each layout.
Corrections are isolated in `1fd0651` and `37af74e`; all original failures remain.

Both corrected tiny variants completed 64 answer checks with independent negative
controls each. The full plain probe ran, but its grader failed: the probe records
only the first page (limit 10,000), whereas `grade_query` supplies that page to an
oracle requiring complete pagination. The broad fixture has 65,536 rows and a
non-null continuation token. This demonstrates a harness-interface mismatch,
not a product loss of 55,536 rows. Full counted profiling was not launched, and
full allocation/timing comparisons are ineligible.

The [counterexample](data/query-plan-run-01/memory/retry-02/full-failure/counterexample.json)
and [lossless evidence map](data/query-plan-run-01/memory/retry-02/full-failure/manifest.json)
retain all 64 actual full-plain answer wrappers, original ledger framing, Batch
bytes and timings. One preservation job exceeded the 50 MiB lab cap because it
compressed repeated answers separately with their wrappers. A subsequent job
factored the identical answer bytes from wrappers, verified every reconstruction,
and retained 51,570,760 bytes across the memory lab, below 50 MiB. The initial
preservation failure remains recorded. No oracle or registered guard was weakened.
A future protocol must either capture pagination outside measured spans or use
an explicitly registered first-page checker before repeating allocation profiling.
Read-only source inspection also found both `query_scan_source_loading` and
`query_walk_source_loading` spans nested inside `sources_walk`; the Scan source
loader has no corresponding label. These existing optional labels need repair
and verification before interpreting source-loading attribution. They are disabled
in native comparison binaries; no instrumentation repair was made in this run.

## Native results

All eight native commands and independent audits exited 0. Each cell completed
200 ordinary and 36 visibility requests at the registered cadence; all custody,
exact recovered logs, quiescent query answers, negative controls, clock/rate and
demand guards passed. Development recovered 3,000 logs per trial; small recovered
300,000 per trial. No retries or final pending/file backlog remained.

| Cell | Balanced server CPU (cores) | Sampled phase peak RSS (MiB) | Process HWM (MiB) | Collection-to-ACK-observation p99 (ms) |
| --- | ---: | ---: | ---: | ---: |
| Development Scan | 0.00688 | 30.30 | 30.63 | 28.61 |
| Development Walk | 0.00479 | 18.04 | 20.15 | 29.66 |
| Small Scan 1 | 0.23438 | 460.80 | 467.93 | 43.38 |
| Small Walk 1 | 0.13589 | 163.93 | 173.91 | 45.45 |
| Small Walk 2 | 0.13578 | 171.19 | 178.00 | 46.58 |
| Small Scan 2 | 0.23484 | 477.25 | 485.94 | 61.57 |
| Small Scan 3 | 0.23498 | 472.26 | 481.80 | 44.06 |
| Small Walk 3 | 0.13552 | 171.58 | 175.26 | 43.97 |

The registered decision is favorable on this workload: median paired CPU ratio
0.5782 (**42.2% less**), every pair below 1; median paired sampled phase-peak RSS
ratio 0.3587 (**64.1% less**). Median ACK p99 was 44.06 ms Scan versus 45.45 ms Walk
(+3.15%, within the declared 5% tolerance). Median peak pending batches was seven
for both plans, with zero sampled file backlog. All nine ordinary shape/phase
p99 comparisons passed. Development remains diagnostic, excluded from these aggregates.

Median-of-three burst p99 fell from 187.81 to 52.83 ms for recent logs,
541.46 to 362.26 ms for absent text, and 188.63 to 48.70 ms for CPU metrics.
Absent-text search remains the slowest tested shape; the source suggests tail
index extension, filtering and decoding as candidates for a later discriminating
ablation, not a demonstrated allocation attribution.

Small Spool acceptance was approximately 20 successful batches/s in every offered
phase, carrying about 1,000/3,000/1,000 logs/s. Payload throughput rose from about
1.18 to 3.52 MB/s in the burst. All recorded sends were ACKed; a complete Spool
append-attempt denominator is unavailable, so no append acceptance percentage is
claimed. Per-cell CPU user/system time, IO, process/cgroup memory, rates, backlog,
query/visibility latencies and transition samples are in the
[consolidated evidence](data/query-plan-run-01/consolidated.json) and raw cell archives.
All selected visibility targets returned by 3.58 seconds after source write;
requests were deliberately held until +3 seconds, so this is an availability
bound under that schedule, not a measured minimum queryability delay.

The result supports Walk as the next candidate for tighter deployment/lifecycle
testing. It does not change the production default, establish statistical
significance, or remeasure the earlier builder-heap reduction.

## Verification, resources and cleanup

Every command ran through `python3 tools/resource_group.py -- python3
tools/bench/labs/query_compare/run_job.py ... -- COMMAND`, with data-drive scratch,
16 GiB memory high, 20 GiB maximum, no swap and the outer 30-minute deadline.
The registered campaign serialized every build, workload, grader and validator.
Exact commands, source snapshots and exits are retained per coordinator job.

| Job(s) | Executed command inside coordinator | Exit/result |
| --- | --- | --- |
| `controls` | `python3 tools/bench/labs/query_compare/run.py --controls` | 0; native, measurement, archive and demand controls |
| `native-build` | `cargo build --offline --locked --release -p fabric_o11y -p fabric-server --bin fabric-node --bin fabric-server --example server_dump --example lab_history_seed` | 0 |
| Eight named native cells | `python3 tools/bench/labs/query_compare/run.py --cell NAME` | All 0; individual and independent guards passed |
| `allocation`, `allocation-retry-01`, `allocation-retry-02` | `python3 tools/bench/labs/query_compare/profile.py --out PATH` | All 1; two ledger assumptions, then pagination mismatch |
| `preserve-profile` | `python3 tools/bench/labs/preserve_query_profile_failure.py` | 1; retained evidence exceeded cap before lossless factoring |
| `compact-profile` | `python3 tools/bench/labs/compact_query_profile_failure.py` | 0; 64 exact wrapper round-trips, cap restored |
| `consolidate` | `python3 tools/bench/labs/summarize_query_plan.py` | 0; all registered paired guards satisfied |
| `verify-profile-archive` | `python3 tools/bench/labs/verify_query_profile_archive.py` | 0; independently compared 64 wrappers and three ledgers against original files |
| `fast-checks` | `cargo xtask checks --profile fast` | 0; all 17 checks passed, including workspace tests, Clippy and independent oracles |
| `audit-controls` | Python `runpy` invocation of `tools/bench/labs/dev_small/audit.py:controls` | 0; raw accounting/custody counterexamples rejected |
| `cleanup` | `python3 tools/bench/labs/cleanup_query_plan.py` | 0; stopped units checked, receipts copied, owned failure scratch removed |
| `documentation` | Compile ten Python helpers, then invoke `bun tools/docs/check.mjs` | 1; Python syntax succeeded, Bun absent from PATH |
| `documentation-retry` | Copy existing Bun runtime into launcher scratch; run `tools/docs/check.mjs` and `git diff --check` | 0; documentation and whitespace checks passed, temporary runtime removed |
| `cleanup-final` | `python3 tools/bench/labs/cleanup_query_plan.py --report cleanup-final.json` | 0; budget/evidence caps, zero OOM/swap and stopped services checked; final preparation failure scratch removed |

Native execution used 2,105.04 seconds of its 2,400-second allowance; profiling
used 29.23 seconds of 900 before its unresolved harness failure stopped that branch.
Checks/cleanup through the cleanup job used 165.23 seconds of 300. Final documentation
and receipt checks are recorded separately in the same budget ledger.
Whole-job cgroup peak, including grading, was **2.332 GiB**; this is distinct from
server RSS. All completed cgroup receipts recorded zero OOM events and zero swap.
Successful fixtures were removed after each job. The final cleanup removed
1,096,306,893 bytes of owned failed scratch after compact preservation/verification;
shared build caches were preserved. Recovery evidence retains the tiny failed
fixtures, while the full failure is retained through its lossless map. No remote
host or persistent service was modified.

The memory lab retained 51,570,760 bytes (49.18 MiB), query 37,961,318 bytes
(36.20 MiB), with coordinator/recovery separately below 50 MiB. Subsequent final
receipts remain subject to those caps. See the
[cleanup review](data/query-plan-run-01/cleanup-review.json) and
[archive verification](data/query-plan-run-01/recovery/profile-archive-verification.json).
The [final cleanup ledger](data/query-plan-run-01/cleanup-final.json) records all
completed services through documentation validation. Its own coordinator and
outer launcher receipts record successful exit and removal afterward. Final
inspection found no Fabric server/node processes and no remaining launcher scratch.

## Limits and remaining work

Fresh history, finite offers, native host metrics plus synthetic logs, one local
machine and two server CPUs do not establish long-run capacity. Visibility probes
at source-write +3 seconds test availability by that observation, not earliest
queryability. Each ordinary shape has twenty requests per offered phase; the
latency comparison reports p99 conservatively. No spans, retention, fault runs,
remote workloads, installation, releases, tags or runtime/default changes occurred.
Mixed-signal integration, tight deployment budgets, longer lifecycle/recovery,
full bounded-builder acceptance and supported installation remain prerequisites
for a defensible development/small release claim.
