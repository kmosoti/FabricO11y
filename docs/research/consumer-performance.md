# Consumer experience and performance across workload profiles

Status: source-grounded consumer interpretation and consolidation of existing native measurements, **not a new benchmark or deployment qualification**. The four names are [workload tiers](workload-sizing.md); the product currently has one [Linux packaging/deployment contract](../architecture/deployment.md), not four independently qualified service profiles. All measured trials used the same constrained cloud host and experimental runtime binaries. [Machine-readable overview](../experiments/benchmarks/data/consumer-profile-overview.json) preserves source summary paths/hashes and exact values.

## What each consumer gets

| Consumer | Current interaction | What useful success looks like | What overload or uncertainty looks like |
| --- | --- | --- | --- |
| Human operator | `fabricctl` enrollment/configuration/inventory, inspection, and log/metric/rate/span queries; no built-in graphical UI | Find retained evidence, see which nodes are reporting, verify desired/applied configuration, understand source coverage | Recent logs arrive late; backlog and freshness need inspection; configuration is desired until applied; gaps/incomplete answers must stay visible |
| Agent | Integrates through the same CLI or structured HTTP responses; no separate agent runtime/API | Bounded time/window/node searches, trace findings to node/sequence/time, inspect coverage before deciding, follow snapshot pagination | Treat stale or incomplete evidence as uncertainty; an empty result is not evidence that an incident never happened; `410` requires restarting the paginated search with a new snapshot |
| Programmatic client | Authenticated HTTPS JSON query/control APIs, including `POST /v1/admin/query`; OTLP traces enter through the local Spindle endpoint | Parse typed rows and metadata, maintain snapshot-bound pages, budget response time and data age separately | Handle non-success responses and expiring snapshots explicitly; distinguish new input delay from query latency; current query route requires admin authorization, not a dedicated read-only role |

An agent here means a consumer that uses existing interfaces. No MCP server, dashboard, autonomous remediation service or agent-specific access-control feature is inferred. Central configuration is pulled on a five-second interval when healthy; desired/applied revisions and errors distinguish intent from activation. Source: [control view](../architecture/control-plane.md), [CLI](../../src/bin/fabricctl.rs), [HTTP routes](../../crates/fabric-server/src/http.rs).

The [answer contract](../architecture/retained-history.md#answer-envelope) gives all consumers rows plus `complete`, `unavailable`, `gaps`, `freshness`, retained time bounds, `snapshot` and `next_page`. These dimensions mean different things:

- **Complete** means the relevant retained server sources were readable; it does not mean every application line has been collected or every historical event was retained.
- **Freshness** is the newest retained observation/metric/span time per node. Fresh host metrics can coexist with unread application logs; freshness alone does not establish that a log source is caught up.
- **Gaps** report observed collection problems. Unread input waiting in app files need not yet produce a gap. Spindle backlog metrics are needed to see that wait.
- **Retention** defines what history is still available. An empty answer outside retained coverage is not a negative finding about the original source.
- **Snapshot pagination** freezes the view. New data arrives in a new snapshot; a page bound to deleted data can return `410 Gone`.

For an operator interface or agent integration, present data age, source backlog, coverage, retained window and query response time together. That is a recommended presentation, not an implemented UI. A defensible agent statement is “no matching rows in this retained snapshot, with these coverage limitations”; “the event did not happen” would require evidence this contract does not provide.

## Performance from all four tiers

All paths below use real native Spindles, selected app files, durable Spools, TLS Sender, server journal and fresh durable recovery. Rates are aggregate scheduled ordinary / 3× burst. Latencies are record-weighted p50 / p99, not request-weighted API percentiles.

| Workload tier | Scheduled logs/s ordinary / burst | Actual native Spindles | Collection → ingestion p50 / p99, ms | File write → ingestion p50 / p99, s | Exact recovered logs | Finite criteria |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Development | 10 / 30 | 1 | 5.06 / 8.89 | 0.414 / 0.910 | 3,000 | Met |
| Small | 1,000 / 3,000 | 20 | 6.19 / 50.65 | 0.508 / 0.998 | 300,000 | Met |
| Moderate | 10,000 / 30,000 | 20 | 15.58 / 102.99 | 0.528 / 1.014 | 350,000 | Met |
| Enterprise volume | 100,000 / 300,000 | 20 | 66.56 / 1,854.64 | 2.242 / 21.315 | 3,500,000 | Timing and producer lag failed |

[Development/small results and protocol](../experiments/benchmarks/dev-small-local-run-01.md); [moderate/enterprise results and protocols](../experiments/benchmarks/native-scaled-spindle-run-01.md). Moderate is named medium in the latter evidence. All completed trials recovered exact source bodies without gaps or duplicate/missing/altered logs, with matching ACK/recovered Batch hashes. The interrupted first enterprise attempt is separately preserved and excluded.

| Workload tier | Collection → ACK observation p50 / p99, ms | Peak server RSS, MiB | Largest Spindle RSS, MiB | Whole-phase server / all-Spindle CPU equivalents | Published Segments |
| --- | ---: | ---: | ---: | ---: | ---: |
| Development | 7.46 / 12.67 | 7.91 | 4.47 | 0.0012 / 0.0015 | 0 |
| Small | 9.10 / 57.46 | 440.41 | 5.38 | 0.0350 / 0.0335 | 5 |
| Moderate | 22.20 / 126.70 | 470.39 | 9.24 | 0.1274 / 0.0903 | 5 |
| Enterprise volume | 79.63 / 1,865.86 | 521.79 | 9.13 | 0.9801 / 0.5882 | 58 |

Development's small RAM result predates its first seal and cannot predict lifetime memory. Whole-phase CPU averages include settling/drain/quiet; they are not burst CPU requirements or reservations. Moderate/enterprise scheduled-window server CPU averages were 0.259 / 1.261 equivalents. RAM is process RSS, not all cgroup/cache memory: enterprise shared cgroup memory reached 16 GiB. Memory limits remain configurable deployment dimensions, not inferred minimums or architectural ceilings.

These rows are useful observed operating points, not a controlled scaling curve. Development/small phases lasted 60/60/60 seconds; moderate/enterprise phases 10/5/10 seconds. Native fleet cardinality is 1/20/20/20, whereas the planning model assumes 1/20/200/2,000. Enterprise generation fell behind during the burst: its nominal five-second burst actually finished writing at 17.02 seconds instead of about 15 seconds. Aggregate volume was preserved; planned burst timing was not maintained. Cross-host/WAN latency, realistic fleet-state cost, cold disks, long retention and sustained stability remain unmeasured here.

## What those timings mean to consumers

At development/small/moderate loads, file polling accounts for much of the approximately one-second p99 arrival delay. A human watching a log search might see new evidence arrive a little later than the application wrote it. An agent that checks immediately may miss newly written rows in its snapshot. A programmatic poller needs a measured ingestion/freshness allowance before interpreting an empty result.

Enterprise overload stretched that file-to-server tail to about 21 seconds while the collection-to-server tail was only 1.85 seconds. About 1.02 GiB of unread source data and 15 server journal files waiting for sealing accumulated; Spindles stayed small because waiting data remained in files. The run drained about 21 seconds after generation ended. That is eventual recovery of the finite workload, not evidence of a sustainable 100k/s service or timely answers under continued input.

Queries read **both Segments and the unsealed journal tail**; consumers do not intrinsically need to wait for sealing to finish. However, persisted receive time is before sync, and these trials did not time when a polling query first returned each record. A send ACK demonstrates durable server custody under the storage assumptions; it is neither a query-response measurement nor a promise about query latency.

## The consumer performance gap

For each tier, four separate measurements are needed:

1. Source-write → first query response containing the exact record, p50/p99: useful evidence freshness.
2. Query request → complete response, p50/p99: responsiveness, separated by query shape and result size.
3. Correctness/coverage/retention outcomes: exact answers, gaps, incomplete sources and page expiry.
4. Concurrent query/control cost during intake: CPU, process/cache memory, journal/source backlog and throughput.

**None of those consumer query-performance measurements were run in these four native pipeline trials.** [Historical query measurements](../experiments/benchmarks/history-run-01.md) and query optimization studies exist, but their revisions/workloads do not supply missing per-tier numbers. A single combined “performance” score would hide this distinction.

A follow-up protocol should freeze the same input shape, query plan, retention volume, poll cadence, query concurrency and exact oracle before measuring. Include log search by node/window, match/no-match text searches, metric history, and long snapshot pagination; rate/span cases require their own signal populations. Tag newly written records, keep full source/collection/receive/ACK/first-query clocks, and include query errors or missing records in the decision rule rather than trimming them. Same-host clocks work here; cross-host runs require measured synchronization uncertainty.

The current host can pilot those finite consumer workloads and compare static worker counts. The PC can isolate producer/Spindles from server, exercise real disks and cache states, run longer retention/soak and configurable systemd budgets. Neither host qualifies enterprise hardware it does not possess. No additional experiment was executed as part of this consolidation.

## Planning configurations are separate

Existing [sizing assumptions](workload-sizing.md#planning-hardware-and-storage) propose starting server experiments at 2 vCPU / 4 GiB for development and small, 4 / 8 GiB for moderate, and 16–24 / 32 GiB for enterprise. Those machines were **not benchmarked by the table above**. They are assumptions to test, not published minimums. Retention, record size/cardinality, queries, outage buffers, cache and concurrent builders all change the budget; the cheapest acceptable profile is the one meeting explicit freshness, responsiveness, fidelity and retention requirements in measured operation.

## Verification

The overview was generated directly from four existing summary files. Summary SHA-256s are retained. For each row, primary/source clock populations match the expected log count, and ordinary/burst/recovery arithmetic matches that count. No percentiles were recomputed, pooled across tiers or extrapolated to new hardware. Documentation checks and `git diff --check` exited 0. No runtime changed; the prior native-run fast checks remain 19 passed / 1 existing Clippy failure ([receipt](../experiments/benchmarks/data/native-scaled-spindle-run-01/receipts/clippy.json)); they were not rerun for this documentation/data consolidation.

The owner-directed [development-through-moderate study plan](development-moderate-plan.md) narrows the next practical testing/use target and adds cap/threading hypotheses plus proposed query responsibilities. No new defaults or query performance results are implied.
