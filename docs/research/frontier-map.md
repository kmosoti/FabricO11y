# Frontier map: the query and storage path

Status: **exploratory research**, tier 6 of the [source-of-truth order](../README.md#source-of-truth). This page is an audit of the retained-history path at `58b694d` (verification tooling merged) with the sealer design on top ([ADR-0022](../decisions/ADR-0022-build-segments-by-external-merge-sort.md), not implemented). It records what exists, what it costs, what checks it, what has not been measured, and where the same problems were solved elsewhere. It decides nothing. The hypotheses it leads to are in the [research ledger](ledger.md).

Every component below carries one of the charter's states: **implemented** (code on `main` with a test), **partial**, **specified** (a contract or ADR says so; the code does not yet), **proposed** (a blueprint or review mentions it), **experimental** (research code outside the product), **obsolete**. A thing that only the [blueprint](../architecture.md) mentions is proposed, however detailed the prose.

## 1. Current representations

| Representation | State | What it is | Where |
| --- | --- | --- | --- |
| `FAB1` frame log | implemented | Append-only frames: 16-byte header (magic, length, header CRC32, payload CRC32), payload, 16-byte `FAC1` commit marker. Two-sync commit. Active file plus sealed files named by first group. One format for the Spindle's Spool and the server's journal | [frame.rs](../../crates/fabric-frame/src/frame.rs) |
| Batch envelope | implemented | Protobuf: version, 16-byte Spindle id, generation, sequence, OTLP logs bytes, OTLP metrics bytes, cursors, collection-gap texts. The exact bytes are the unit of identity (SHA-256) and of retention | [envelope.rs](../../crates/fabric-frame/src/envelope.rs) |
| Journal Group | implemented | One frame per commit group: group sequence and entries, each entry the credential label, the exact Batch bytes and the server receive time | [store.rs](../../crates/fabric-server/src/store.rs) |
| Segment | implemented | One per sealed journal file: four Zstd-3 Parquet tables (`batches` with the exact bytes and their SHA-256; `logs` and `metrics` projections sorted by the query key; `gaps`), 8,192 rows per row group, and a JSON manifest written last: group range, record count, receive-time bounds, newest time per node label, each file's size, rows and SHA-256. Directory rename is the commit | [segment.rs](../../crates/fabric-server/src/segment.rs), [ADR-0020](../decisions/ADR-0020-store-sealed-history-as-parquet-segments.md) |
| Stream checkpoint | implemented | `streams.json`: next group, per-Strand last sequence and digest, label and node bindings. Written before a journal file is deleted | `State` in store.rs |
| Control state | implemented | `control.json`: enrollments, desired and applied revisions | [control.rs](../../crates/fabric-server/src/control.rs) |
| Hot representation | **none** | The unsealed tail is searchable only by decoding every frame on every query. There is no memtable, no in-memory index, no Arrow batch kept between queries. The charter's "Arrow-compatible hot representation" is **proposed**, not present | [query.rs](../../crates/fabric-server/src/query.rs) `History::sources` |
| Side indexes | **none** | By contract until a registered gate fails: no Bloom, postings, dictionary or zone map beyond Parquet's own row-group statistics | [contract non-goals](../PRODUCT-CONTRACT.md#non-goals-of-the-first-profile) |

Semantics the representations carry, in the charter's vocabulary: **identity** (Strand = Spindle id and generation; sequence; byte digest), **time** as two claims kept apart (the node's `observed_ns` or `time_ns`, and the server's `received_ns`), **partial order** (total per Strand by sequence, total per server by group), **provenance** (every row traceable to one entry's exact bytes), **custody** (ACK only after the two-sync commit), **completeness, freshness, gaps** (fields of every answer), **explicit loss** (collection gaps are rows, never silence), **bounded use** (journal and Spool caps, `limit` at most 10,000), **replayability** (journal replay rebuilds stream state; Segments rebuildable from journal files until reclaimed). Not represented: **correction or retraction** (no mechanism; appends only), **uncertainty of time** (skew between node and server clocks is unstated and unmeasured).

## 2. Current query pipeline

Three query kinds ([retained-history contract](../architecture/retained-history.md)): log search, metric history, counter rate. One JSON shape over HTTPS; `fabricctl` is the one client. The pipeline, as the code runs it:

1. `axum` handler, `spawn_blocking`, `History::run` with the committed group as the snapshot's newest bound; three retries on "journal moved" or "segment removed".
2. `sources()`: list every Segment manifest; read **every** journal file (sealed and active) frame by frame and decode every Group into memory, keeping those not covered by a Segment; compute the oldest retained group; apply the page token's floor.
3. For every tail entry, `extract` every OTLP row (logs, metrics, gaps) into `Rows`; fold freshness and receive bounds.
4. For every Segment: fold receive bounds and per-node freshness from the manifest; read `gaps.parquet`.
5. Per kind: a bounded heap of `limit + 1` smallest keys; offer every tail row that passes the filters; for every Segment, `scan_*` with row groups pruned by the time column's statistics, every surviving row fully materialised (`LogRow` with three allocations and an attribute-map JSON parse), then filtered, then offered.
6. Pages: a token binds the snapshot (oldest, newest) and the last key; `after_page` continues strictly after it; a floor below the oldest retained group answers Gone.
7. Answer: rows, `complete` (no unavailable source), `unavailable`, retained window (receive bounds over all sources), per-node `freshness`, filtered `gaps`, `snapshot`, `next_page`.

Domain decisions in the pure kernel ([fabric_core::query](../../crates/fabric-core/src/query.rs)): window validity, limit range, snapshot containment, Gone rule, page continuation, completeness, counter step. Everything about cost lives in the adapter.

## 3. Current indexes and pruning

| Mechanism | State | Used for | Not used for |
| --- | --- | --- | --- |
| Parquet row-group statistics on the time column | implemented | `segment::prune`: skip row groups whose min/max cannot overlap the window | nothing else; node, name and page bounds are applied after materialisation |
| Manifest receive bounds | implemented, **not used for pruning** | the answer's retained window | skipping a Segment whose gaps cannot be in the window |
| Manifest per-node freshness | implemented, **not used for pruning** | the answer's `freshness` | skipping a Segment that holds no row of the queried node, or none at or after `from_ns` |
| Row order inside `logs` and `metrics` | implemented | nothing at query time | stopping a scan once the bounded heap cannot change |
| Manifest file size and row count | implemented | detecting a damaged Segment per query | — |
| Whole-file SHA-256 | implemented | `segment::verify`, on demand | per-query integrity (deliberately) |
| The tail | none | — | the tail has no selection: every entry is decoded for every query |

Found during this audit, and tested in [retention-scale run 01](../experiments/benchmarks/retention-scale-run-01.md): the manifest's facts allow **sound lower-bound skipping only**. The newest time per node bounds every row's time from above, so `max(freshness) < from_ns` proves no row is in the window; receive bounds bound gaps, which are filtered by receive time. Nothing bounds a row's time from below (no minimum observed time is recorded), so a Segment cannot be skipped for a window that ends before it. Two traps: receive bounds do **not** bound row times, because a node's clock may run ahead of the server's; and a node absent from the freshness map may still have gap rows, which carry the node and are filtered by it.

## 4. Current cost centres

Measured, on this 4-CPU container, all exploratory unless marked:

| Cost | Measurement | Record |
| --- | --- | --- |
| Unsealed tail per query | 126 ms for a query that can match nothing, on a 40 MiB tail; 125 to 152 ms for every shape; about 3.2 ms per MiB; 1.1 ms once the same bytes are a Segment | [query attribution run 01](../experiments/benchmarks/query-attribution-run-01.md) |
| Row materialisation | about 0.63 µs per log row, 0.24 µs per metric point, paid for every row in a surviving row group whether or not it is returned | the same |
| No early termination | `limit 50` over 8 row groups costs the same as `limit 1,000` | the same |
| Per-Segment fixed cost | opening the manifest, `gaps.parquet` and each table's footer, for every Segment, every query | [retention-scale run 01](../experiments/benchmarks/retention-scale-run-01.md) |
| Query floor at 1,000,000 records | about 280 ms whatever the query, with a 48 MiB tail and `fabricctl`; p99 at most 481 ms (registered gate 2 s) | [history run 01](../experiments/benchmarks/history-run-01.md) (registered) |
| Journal-only mode | 1.3 to 1.5 s per query at the same fixture | the same |
| Client | `fabricctl` adds 25 to 47 ms (process start, TLS handshake) | attribution run 01 |
| Sealing | 356 MiB peak heap per 64 MiB file today; 43 MiB in the accepted design; about 1.1 s per file | [sealer study run 01](../experiments/benchmarks/sealer-study-run-01.md), [soak run 01](../experiments/benchmarks/soak-run-01.md) (registered, failed) |
| Startup replay | 1.6 s for a 40 MiB journal (first read from disk); 1.7 s for 320 MiB with a warm page cache | attribution run 01; [retention-scale run 01](../experiments/benchmarks/retention-scale-run-01.md) |
| **Query memory over an unsealed tail** | one query that can match nothing, over a 320 MiB unsealed tail, took 4.64 s and raised the server's high-water mark from 18 MiB to 876 MiB; 714 MiB stayed resident afterwards. Query time and memory are both proportional to the tail, which `journal_bytes` allows to reach 4 GiB | retention-scale run 01; the key-index prototype in [tail-index run 01](../experiments/benchmarks/tail-index-run-01.md) removes the time for selective shapes and not the memory for whole-window ones |
| Line to query | 517 ms at the median, 85 % of it the Spindle's 1 s log poll; 50 ms of it the server's group window | [collection-to-query run 01](../experiments/benchmarks/e2e-latency-run-01.md) |
| **Wide-window `limit` queries** | with sources ordered by minimum key and the heap's threshold as the stop: `limit 50` over a 64 MiB tail 252 to 320 ms stock, 13 to 20 ms on the prototype, reading 26 entries of 55,000; one row group of 64 on disjoint Segments; no gain at total overlap; server memory flat at 56 to 59 MiB against 242 to 257 | [threshold run 01](../experiments/benchmarks/topk-run-01.md) |
| **Budgeted answers** | a request bounded by rows examined over the L-04 walk answers exactly the rows below a named boundary key and resumes there; a rare text search 243 to 516 ms a page stock and unbounded, 14 to 35 ms at 2,000 to 10,000 rows; a whole-window rate 229 to 242 ms to 7 to 13 ms; no effect when every source starts at the same key | [budget run 01](../experiments/benchmarks/budget-run-01.md) |
| **Segment bytes** | the payload stored twice (raw Batch bytes and the body column) is 92 % of a synthetic Segment and 63 % of a real-text one; the per-Batch hash 3.6 % and 17.6 %; all key and time columns about 3 B per row; the unsealed tail holds 2.5 times its Segment's bytes on real text | [storage layout run 01](../experiments/benchmarks/storage-layout-run-01.md) |
| Group commit | 50 ms window held open even for a lone Batch | the same; [store.rs](../../crates/fabric-server/src/store.rs) `CommitMode::GROUPED` |

## 5. Current correctness invariants

From the [verification matrix](../formal/verification-matrix.md), with their checkers: DEL-1 to DEL-5 (commit precedes ACK, no duplicate, no replacement, gaps detected, bindings fixed: kernel truth table, differential, delivery oracle, mutants, TLA+ with trace validation, turmoil, Kani), SPOOL-1 to 3 (custody, reclaim, known failure: tests, mutants, process-death stages), CTRL-1 and 2 (invalid configuration, terminal revocation: end-to-end tests, Kani), HIST-1 to 6 (journal and Segment answer identically; sealing changes no answer; a damaged Segment never yields `complete`; pages bound to a snapshot; retention semantics; resets not rates: query and rate oracles, mutants, crash-state tests, Kani, proptest), ARCH-1 and 2 (layer and purity gates). Query-page invariants stated in the contract: query execution never mutates; row order is total; every row traces to one entry's bytes.

Stated for the sealer design, not yet checked: S-1 to S-6 (same rows, same order, bounded heap, no leftovers, delete after rename, determinism). Not stated anywhere, and this audit's central finding: **no invariant says what a source may be skipped on.** Row-group pruning is sound by Parquet's statistics; nothing in the kernel or the matrix names the property, and nothing would catch an unsound skip added at Segment or tail level except the query oracle, after the fact.

## 6. Current benchmark evidence

Registered and run: history (revision 2, four-CPU host, passing), outage and stress (revision 2, passing), soak (failed on sealer memory), installation acceptance (inconclusive). Exploratory: the sealer study, collection-to-query latency, query attribution, retention scale, tail index, threshold walk, storage layout. None on the target profile. Fixtures: synthetic (the fleet simulator's two 512-byte lines per identity per second, half repeated bytes and half seeded entropy; 32 gauge points every 15 s). No real log corpus has been used; compression, text-search selectivity and template structure of real logs are unmeasured.

## 7. Missing measurements

- Query cost against the **number of retained Segments** at the real Segment size: [retention-scale run 01](../experiments/benchmarks/retention-scale-run-01.md) measured 319 Segments of 1 MiB (about 0.08 ms per Segment for a narrow window; the full-window shape grows with the rows retained); the same at 64 MiB Segments, 20 GiB, is not measured.
- **Recovery time at full retention**: journal replay measured 1.7 s for 320 MiB warm; cold, and at the 4 GiB `journal_bytes` allows, unmeasured. The first query after such a restart is the larger cost (4.64 s and 858 MiB over a 320 MiB tail).
- **Per-row cost split** between Parquet decode, Zstd, allocation and the attribute JSON parse; it decides whether late materialisation or a leaner row type is the lever.
- **Clock skew** between nodes and server in any real deployment; the freshness field assumes the node's clock.
- **Cross-Segment time overlap** under backlog drains: measured at its two extremes in [threshold run 01](../experiments/benchmarks/topk-run-01.md) (disjoint: one source read; total: everything read); the distribution under a real backlog drain at 64 MiB Segments is not.
- **Real log corpora**: bytes measured in [storage layout run 01](../experiments/benchmarks/storage-layout-run-01.md) on 15,994 real lines drawn at random (6:1 body column, 5.4:1 raw copy, duplication 63 % of the Segment); query cost on real text (text-search selectivity, body decode) is not measured (ledger L-21).
- **Cycles, instructions, cache misses**: no `perf` in this container; only wall and CPU time have been recorded.
- **Cold-cache query cost**: every run so far had the page cache warm.
- **ACK p99 while sealing** and while a query runs, on the two-CPU profile.
- **Write amplification of the Segment layout**: raw Batches are stored once in the journal and again in `batches.parquet` (52 % of a Segment in attribution run 01), plus the projections (45 %); the end-to-end bytes written per byte ingested are not recorded as one number.

## 8. The same problems in major systems

| Fabric mechanism | Nearest production counterparts | What they do differently, and why |
| --- | --- | --- |
| Sealed Segment per journal file, manifest written last, rename as commit | LSM SST files and `MANIFEST` (RocksDB, LevelDB); Lucene segments; ClickHouse parts; Prometheus TSDB blocks; Loki chunks; Druid and Pinot segments | All immutable; most are **merged** (compaction) into larger units to bound the number of files a query opens and to drop deleted rows. Fabric never merges: one file, one Segment, deleted whole by retention. Simpler custody, but the per-query cost grows with the Segment count |
| Row groups of 8,192 rows with min/max statistics | ClickHouse granules (default `index_granularity` 8,192) with a sparse primary-key index and skip indexes (minmax, set, Bloom); Parquet row groups and page statistics in Druid, Pinot, Tempo, DataFusion; PostgreSQL BRIN; Netezza zone maps | They add a **sparse index over the sort key** so a lookup finds the granule without reading every granule's statistics, and they allow secondary skip indexes. Fabric reads every row group's statistics from each file's footer |
| Unsealed tail decoded per query | RocksDB and LevelDB memtables (skip lists, searchable); Prometheus head block (in-memory series, WAL for recovery); Lucene's near-real-time reader; Loki ingesters holding chunks in memory | Every one keeps the unflushed data in a **queryable in-memory structure** and uses the log only for recovery. Fabric's tail is a WAL without a memtable: durable, but searchable only by replay |
| Bounded heap of `limit + 1` keys | Top-k in every engine; ClickHouse's Top-N granule pruning that feeds the current threshold back into reading; DataFusion's dynamic filter pushdown for TopK | They **stop reading** sources that cannot beat the threshold. Fabric reads every surviving row group |
| Full materialisation of candidate rows | C-Store and Vertica late materialisation; DuckDB's late materialisation; ClickHouse's lazy materialisation; Parquet row selection in arrow-rs | They decode filter columns first and fetch the rest for survivors |
| Page token bound to a snapshot and a last key | Elasticsearch point-in-time plus `search_after`; keyset pagination over immutable parts | The same idea; Fabric's snapshot is a group range because its sources are immutable |
| `complete`, `unavailable`, `freshness`, `gaps` in every answer | Prometheus staleness markers and the `up` series; Loki and ClickHouse: none; Monarch and Dremel: result-level completeness for sharded reads (described in their papers, not specified here) | Few systems carry a completeness verdict; none that this audit knows carry collection gaps as data. This is Fabric's distinctive surface |
| Exact bytes retained beside projections | Loki (chunks of raw lines plus a label index); Tempo (raw traces as Parquet); Elasticsearch `_source` beside the inverted index | The same duplication, accepted for provenance and reindexing. ClickHouse and Prometheus keep no raw form |
| Group commit with a 50 ms window | DeWitt et al. (1984) group commit; Kafka `acks=all` with `flush.ms`; every WAL-based database | Most close a group as soon as the previous sync returns rather than holding a fixed window; a fixed window is simpler and costs latency when idle |
| One-second log poll, no inotify | `tail -f` (polling by default), promtail and Vector (fsnotify) | They notify; Fabric polls by contract to keep the Spindle free of watchers |
| Retention by whole Segments, oldest first, by age and bytes | ClickHouse TTL by partition; Druid retention rules; Prometheus block deletion; Loki table manager | The same |

## 9. The same problems in the literature

| Front | Foundational | Later and current | Fabric's relation |
| --- | --- | --- | --- |
| Completeness of answers over incomplete data | Imieliński and Lipski 1984 (incomplete information); Motro 1989 (integrity = validity + completeness); Levy 1996 (complete answers from incomplete databases, local completeness statements) | Razniewski and Nutt 2011 (completeness of queries over incomplete databases: table-completeness statements entail query completeness); Lang, Nehme, Robinson and Naughton 2014 (partial results in database systems: semantics when sources fail) | Fabric's `complete` is a query-completeness verdict under per-source completeness statements: a Segment or journal file is complete for its group range if readable; unavailable otherwise. The kernel does not say so in those terms. **Rediscovery**, recorded in the ledger |
| Pruning without false negatives | Moerkotte 1998 (small materialised aggregates, the zone-map idea); Bloom 1970 | PostgreSQL BRIN; ClickHouse skip indexes; learned indexes (Kraska et al. 2018) keep the same contract: a superset of the answer | The charter's pruning invariant is the soundness of a synopsis; Fabric has it only for row groups and only by construction, not by statement |
| Top-k with early termination | Fagin, Lotem and Naor 2001 (threshold algorithm); Broder et al. 2003 (WAND) | Ding and Suel 2011 (block-max WAND); ClickHouse and DataFusion dynamic Top-N pruning | Fabric's answers are the `k` smallest keys over time-ordered sources: the threshold algorithm over row-group bounds applies directly |
| Materialisation strategies | Abadi, Myers, DeWitt and Madden 2007 | Parquet row selection; DuckDB and ClickHouse late materialisation | Fabric materialises early |
| Write-optimised logs and read-optimised structures | O'Neil et al. 1996 (LSM-tree); DeWitt et al. 1984 (group commit) | RocksDB; Prometheus TSDB | Fabric is a one-level LSM without a memtable and without compaction |
| Adaptive and self-tuning physical design | Chaudhuri and Narasayya (AutoAdmin, 1997 on); Schnaitter et al. 2006 (COLT, continuous online tuning); Bruno and Chaudhuri 2007 (online physical design); Idreos, Kersten and Manegold 2007 (database cracking); Graefe and Kuno 2010 (adaptive merging) | Petraki, Idreos and Manegold 2015 (holistic indexing); learned indexes | The charter's H1 is this body of work. Fabric's difference is that canonical evidence is immutable and sidecars are rebuildable, which turns the decision into a per-Segment rent-or-buy problem. Contract-gated until a gate fails |
| Approximate and progressive answers | Hellerstein, Haas and Wang 1997 (online aggregation); Zilberstein 1996 (anytime algorithms) | BlinkDB 2013; progressive and adaptive query processing (Avnur and Hellerstein 2000, Eddies; Deshpande, Ives and Raman 2007, survey) | Fabric's evidence is set-valued (which sources were read), not statistical. A budget-bounded answer can be exact for the sources read and name the rest as unavailable, which is Lang et al.'s partial-result semantics with Fabric's fields |
| Incremental maintenance | Gupta and Mumick 1995 (IVM survey); McSherry et al. 2013 (differential dataflow); Budiu et al. 2023 (DBSP) | Materialize; Feldera | Immutable Segments make partition-level incremental maintenance trivial; no consumer (no continuous query) exists in the contract |
| Log compression and template search | Drain (He et al. 2017) | CLP (Rodrigues, Luo and Stumm 2021); LogGrep (2023) | Fabric uses generic Zstd columns; real corpora are unmeasured |
| Time-series compression | Gorilla (Pelkonen et al. 2015) | Prometheus TSDB chunks; VictoriaMetrics | Fabric stores metric points as Parquet columns; no delta or XOR encoding |
| Provenance | Green, Karvounarakis and Tannen 2007 (provenance semirings) | — | Fabric keeps where-provenance (row to exact bytes) and nothing finer |

## 10. Unresolved tensions

1. **A WAL without a memtable.** Freshness requires the tail to be queryable; the contract's ACK rule requires it to be a synced log; today the two are reconciled by decoding the log on every query, which costs about 3.2 ms and 2.7 MiB of memory per MiB of tail, per query. Every other system pays a bounded memory for a memtable instead. This is the only cost in the audit that can exceed the server's memory: a sealer that falls behind, or a `journal_bytes` of 4 GiB, puts gigabytes behind one query.
2. **Provenance costs bytes twice.** The exact Batch bytes are retained beside the projections that answer queries (52 % plus 45 % of a Segment). Dropping the projections makes every query decode protobuf (the measured tail cost, 3.2 ms per MiB); dropping the raw bytes breaks the contract.
3. **No merging.** One file, one Segment keeps custody simple and retention exact, and makes every query's fixed cost proportional to the Segment count.
4. **The manifest publishes facts that are not the facts pruning needs.** Receive bounds and newest-time-per-node allow lower-bound skipping only, and the gap table escapes the node facts.
5. **Two clocks, one answer.** Rows are filtered by the node's clock, gaps by the server's; freshness is the node's claim. Skew is neither bounded nor reported.
6. **A two-CPU profile.** The commit thread, the sealer, any future tail indexer and every query share two CPUs; the 50 ms group window hides contention today.
7. **A contract clause that gates research.** Indexes beyond row-group statistics need a failed registered gate; the registered gates are loose (2 s) and were measured on four Segments. The honest path is a registered protocol with hostile shapes that could fail.
8. **Early termination needs cross-Segment order.** Within a Segment row groups are disjoint in time; across Segments they overlap when nodes drain backlogs. The threshold algorithm still works, but its gain is bounded by the overlap, which is unmeasured.

## 11. Promising cross-domain transfers

- **Query completeness reasoning** (Levy; Razniewski and Nutt) as the specification of Fabric's evidence fields, so that every skip is a theorem over source statements rather than a code path.
- **The threshold algorithm** (Fagin et al.) over row-group and tail-entry bounds, for every `limit` query.
- **Block-max style bounds** (Ding and Suel) as the model for what a manifest should publish: per-table minimum and maximum of the query key, so skipping is sound on both sides.
- **Memtable discipline** (every LSM): keys in memory, bytes in the log, rebuilt on restart.
- **Partial-result semantics** (Lang et al.) for budgeted execution: bounded cost with a truthful answer, never a silent truncation.
- **Rent-or-buy analysis** for rebuildable sidecars, if the contract ever admits them: build a Segment's sidecar after its queries have cost as much as the build.

What this audit did not find: a reason to build a general planner, a cost model, a vectorised executor, a query language, or any correlation operator before the four tensions numbered 1, 3, 4 and 8 are resolved by measurement. The [direction review](query-engine-direction.md) says the same from the proposal's side.
