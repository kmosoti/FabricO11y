# Research ledger

Status: **exploratory research**, tier 6 of the [source-of-truth order](../README.md#source-of-truth). Every significant idea about the query and storage path is recorded here with a state, its ancestors, and what would falsify it. Nothing here is a decision. Failed ideas stay: they bound the search.

States: **HUNCH** (a direction, not yet a mechanism), **PRIOR_ART** (exists in the literature or in production; adopting it is engineering, not research), **REDISCOVERED** (arrived at here, then found in the literature; the finding strengthens the foundation), **DERIVED** (a mechanism written precisely enough to analyse), **CANDIDATE** (derived, with a falsification experiment designed), **EXPERIMENTING**, **FALSIFIED**, **PROMOTED** (accepted into a decision or a milestone), **UNRESOLVED** (no decisive experiment is affordable or in scope yet).

The novelty firewall of the charter applies to every entry: before "novel", the idea was translated into database, networking and information-retrieval terms and searched under each. Where that search was from memory rather than from a live literature search, the entry says "no equivalent was identified in the searched literature under these formulations" and nothing stronger.

## Index

| Id | Idea | State | Next action |
| --- | --- | --- | --- |
| L-01 | Fabric's evidence fields are query completeness under per-source completeness statements | REDISCOVERED | State it in the kernel (Q1 of the [direction review](query-engine-direction.md)) |
| L-02 | Sound Segment dispositions from manifest facts remove the per-Segment cost that grows with retention | DERIVED and confirmed sound; FALSIFIED as a material saving | [Retention-scale run 01](../experiments/benchmarks/retention-scale-run-01.md): the rule changes no answer, the two traps are real, the saving is tens of milliseconds at 319 Segments |
| L-03 | A memtable of keys makes the unsealed tail selectable | CANDIDATE, now first | Prototype against hostile tails; the tail costs 3.2 ms and 2.7 MiB per MiB per query (run 01) |
| L-04 | The threshold algorithm over row-group and entry bounds ends every `limit` scan early | CANDIDATE, now second | Measure the gain against cross-Segment overlap; the wide-window shape is the retention-scale risk (579 ms over 319 Segments of 1 MiB) |
| L-05 | Budget-bounded answers that are exact for the sources read and name the rest | CANDIDATE | Property first, then a contract change |
| L-06 | Drop the projections and answer from raw bytes with a key sidecar | FALSIFIED for text search; UNRESOLVED otherwise | None until real corpora are measured |
| L-07 | Manifest facts for two-sided skipping: per-table key bounds, node presence that covers gaps | DERIVED | Decided by L-02's result; an amendment to ADR-0020 |
| L-08 | Query-shaped sidecars that follow workload drift (charter H1) | PRIOR_ART | None until a registered gate fails |
| L-09 | Progressive evidence search (charter H2) | REDISCOVERED | It is L-02 to L-05 in sequence |
| L-10 | Information-gain planning (charter H3) | UNRESOLVED | Revisit when two access paths exist |
| L-11 | Causal pruning (charter H4) | UNRESOLVED, out of contract | None |
| L-12 | Workload-specific physical operators (charter H5) | REDISCOVERED | It is L-03 and L-04 |
| L-13 | Bounded adaptive telemetry (charter H6) | PRIOR_ART, with one measured lever | A Spindle experiment, outside this path |
| L-14 | Incremental investigation over immutable sources (charter H7) | PRIOR_ART | None until a continuous query has a consumer |
| L-15 | Query-directed instrumentation (charter H8) | HUNCH, out of contract | None |
| L-16 | External merge sort for the sealer | PROMOTED | [ADR-0022](../decisions/ADR-0022-build-segments-by-external-merge-sort.md) |
| L-17 | Clock skew between node and server is unbounded and unreported | UNRESOLVED | Measure on the target host; run 01 shows a 5 s skew breaks receive-bound pruning in 31 of 440 queries |
| L-18 | Query time and memory are proportional to the unsealed tail, unbounded by anything but `journal_bytes` | OBSERVED (run 01) | Bound it: L-03, or a budget (L-05) as the stopgap |

## Ranking of the next experiments

Ranked by what the next experiment would teach, not by the size of the expected speed-up:

Before run 01, L-02 ranked first: it tested a risk nobody had measured (the registered 2 s gate was passed on four Segments; retention allows about 320), it tested the charter's essential pruning invariant with two concrete ways to break it, and every other pruning idea depended on its answer. It was run; the [record](../experiments/benchmarks/retention-scale-run-01.md) and the L-02 entry hold the result. After it:

1. **L-03.** Run 01 found the one cost that can exceed the server's memory: a query over an unsealed tail takes about 3.2 ms and 2.7 MiB per MiB of tail, so 4.64 s and 858 MiB over 320 MiB, and `journal_bytes` allows 4 GiB. What the next experiment teaches is whether a key index bounds both under hostile tails and what it costs the commit thread on two CPUs.
2. **L-04.** The wide-window `limit` shape is the retention-scale risk (579 ms over 319 Segments of 1 MiB; tens of seconds extrapolated to 20 GiB), and its remedy's gain depends on cross-Segment overlap, which no run has measured.
3. **L-05.** A budget is the only bound on the tail cost until L-03 lands, and its property is cheap to state and test.
4. **L-07.** Run 01 showed which facts prune soundly and which do not; the manifest amendment is design work whose test already exists.

Only one experiment runs at a time; L-03's is next.

## Entries

### L-01. Evidence fields as query completeness

**State:** REDISCOVERED.

**Claim.** Fabric's `complete`, `unavailable`, retained window and `freshness` are a query-completeness verdict under completeness statements about each source: a Segment is complete for its group range when readable and intact; the journal tail is complete up to the committed group; retention bounds the retained window. An answer is complete when every source whose statements could cover a matching row was read.

**Intellectual ancestors.** Imieliński and Lipski 1984 (incomplete information); Motro 1989 (integrity = validity + completeness); Levy 1996 (obtaining complete answers from incomplete databases, local completeness statements); Razniewski and Nutt 2011 (completeness of queries over incomplete databases); Lang, Nehme, Robinson and Naughton 2014 (partial results in database systems). The earlier Fabric [coverage-receipt research](../architecture/query.md) is the same idea at block level with authenticated metadata.

**What the rediscovery buys.** A vocabulary and a decision procedure: the kernel's disposition function (read, skip, unavailable) is the entailment check "do the source's statements rule out every matching row", and the four invariants in the [direction review](query-engine-direction.md#q1-the-evidence-kernel) (skipping is silent, unavailability is sticky, a skip is sound, pages keep evidence) are the soundness and monotonicity of that check.

**Remaining uncertainty.** Whether the glossary's definition of `complete` ("every Segment and journal file that could hold matching rows was read and verified") and the code agree when a Segment that could hold no matching row is damaged: the code today reports it unavailable because it reads every Segment's gaps table; the definition says it need not be read. L-02's prototype takes the definition's side; the choice belongs in the kernel's contract.

### L-02. Sound Segment dispositions from manifest facts

**State:** DERIVED and confirmed sound; FALSIFIED as a material saving. The [run record](../experiments/benchmarks/retention-scale-run-01.md) has the data.

**Result.** Of the three ways the hypothesis could fail: (a) the narrow-window cost does grow with the Segment count, but only by about 0.08 ms per Segment (26.7 ms at 319), two thirds of it manifest parsing that the disposition still does, so there was little to save; (b) the sound rule changed no field of any of 440 answers or their pages on the hostile state; (c) the naive rules changed 73 answers, every one attributable to a trap (31 lost rows from clock-ahead nodes, 42 lost gaps from gap-only nodes). The traps are real, the rule is sound, and the saving (a third to two thirds of 20 to 40 ms) is not material against the gate. The run's larger findings were unplanned: the wide-window shape grows with the rows retained (579 ms over 319 Segments of 1 MiB), and a query's time and memory are proportional to the unsealed tail (4.64 s and 858 MiB over 320 MiB).

**Precise problem.** Every query reads every Segment's manifest, `gaps.parquet` and each table's footer, whatever its window ([frontier map §3](frontier-map.md#3-current-indexes-and-pruning)). The cost per Segment is unmeasured, and the number of Segments at the contract's retention (20 GiB, 64 MiB files) is about 320, eighty times the fixture the registered gate was measured on.

**Strongest prior art.** Zone maps (Moerkotte 1998) and partition pruning in every warehouse: a partition's min and max bound the rows it can hold. Levy 1996 for the completeness side. Nothing to add.

**Why Fabric's constraints differ.** The facts a manifest publishes were chosen for the answer's evidence fields, not for pruning: receive-time bounds and the newest time per node. Rows are filtered by the node's clock, gaps by the server's. So the usual "min and max of the filter column" is not available; what is available bounds rows from above only, and bounds gaps by a different clock.

**Proposed mechanism.** A disposition per Segment, computed from the manifest before any file is opened:
- rows (logs and metrics) can be skipped when `max(freshness) < from_ns`, because every row's time is at most its node's newest time; and when the queried node is absent from the freshness map;
- gaps can be skipped when `received_max_ns < from_ns` or `received_min_ns >= to_ns`, because gaps are filtered by receive time;
- the Segment's receive bounds and freshness still fold into the answer's evidence (skipping is silent).

**Expected advantage.** For windows that end before a Segment's data, the per-Segment cost drops from four file opens and a gaps scan to one manifest read; for "last N minutes" shapes at full retention, a cost linear in the Segment count becomes a cost linear in the Segments whose data is recent.

**Expected disadvantage.** No upper-bound skipping (nothing bounds a row's time from below), so windows that start before a Segment and end inside it still open it; a damaged Segment outside the window is no longer reported as unavailable (L-01's uncertainty).

**Novelty risk.** None claimed. This is partition pruning with Fabric's facts.

**Falsification test.** The hypothesis is false if any of these holds: (a) the stock server's cost for a narrow-window query does not grow with the Segment count, so there is nothing to save; (b) the sound disposition changes any field of any answer in a randomised differential against the stock server on a state built to defeat it (nodes whose clocks run 5 s ahead of the server's; nodes that send only gaps; windows that start within 8 s after a Segment's last receive); (c) the two "naive" dispositions (skip rows by receive bounds; skip gaps by node absence) change no answer, which would mean the traps are imaginary and the manifest's facts suffice as they are.

**Smallest prototype.** Thirty lines in a scratch copy of `query.rs`, selected by an environment variable, never in product code; states generated by the sealer-study generator at 1 MiB per file so that 320 Segments fit this container.

**Baseline.** The stock server on the same states.

**Promotion criterion.** (b) passes and (c) fails on the hostile state, and the saving at 320 Segments is material against the 2 s gate. The first two held; the third did not. What is promoted is the rule and its invariant, not the saving: the disposition becomes kernel code under Q1 with Kani harnesses for E1 to E4 because L-03 and L-04 need the same rule per tail entry and per row group, and L-07 amends the manifest when either of those lands.

### L-03. A memtable of keys for the unsealed tail

**State:** CANDIDATE.

**Precise problem.** The tail is decoded in full on every query: 126 ms for an empty window on 40 MiB, about 3.2 ms per MiB ([attribution run 01](../experiments/benchmarks/query-attribution-run-01.md)); and it is held in memory in full: 4.64 s and about 858 MiB for an empty window over a 320 MiB tail, 2.7 MiB per MiB, resident afterwards ([retention-scale run 01](../experiments/benchmarks/retention-scale-run-01.md)). The tail is bounded only by `journal_bytes`, which allows 4 GiB; a sealer that falls behind makes every query pay for the backlog.

**Strongest prior art.** The memtable of every LSM store (O'Neil et al. 1996; RocksDB; LevelDB); Prometheus's head block; Lucene's near-real-time reader. The key-only variant is the "index memtable" of systems that keep values in the log (Bitcask's in-memory keydir).

**Why Fabric's constraints differ.** Entries are opaque exact bytes that must be retained as received; rows are a projection; the index must be rebuildable from the journal and must drop entries the moment a Segment covers them; the server has two CPUs and the commit thread must not pay.

**Proposed mechanism.** Per entry: group, frame position, node, receive time, time bounds of its log rows and of its metric points, gap presence (about 80 bytes). Extended on demand by the query that needs it, from the last indexed frame to the committed group, under a lock; dropped per Segment reclaim; rebuilt on restart. Selection by the L-02 disposition applied per entry; materialisation decodes only selected entries.

**Expected advantage.** The fixed cost becomes proportional to entries committed since the last query; selective shapes on a 64 MiB tail from about 200 ms to milliseconds.

**Expected disadvantage.** Memory (about 4 MB per 64 MiB tail at the soak workload; more with small Batches); a query after a quiet period pays the catch-up; a second code path beside the full decode.

**Novelty risk.** None claimed: Bitcask-style keydir over a WAL.

**Falsification test.** False if, on hostile tails (10,000 one-row Batches per second; one node interleaved with 999; a drained backlog spanning the whole file), the index exceeds a stated byte bound, or a selective query still costs more than a small constant plus the selected entries' decode, or any answer differs from the full decode, or ACK p99 moves in the registered soak.

**Smallest prototype.** The index as a scratch module in `fabric-server`, the full decode kept as oracle, the attribution harness extended with the hostile generators.

**Baseline.** The stock tail path; the sealed-Segment path as the floor.

**Promotion criterion.** The falsification test passes on all four hostile tails, with the memory bound and the differential registered as mutants; then Q2 of the direction review.

### L-04. The threshold algorithm over source bounds

**State:** CANDIDATE.

**Precise problem.** A `limit` query reads every row group whose time range overlaps the window although the bounded heap stops changing after the first few: `limit 50` costs the same as `limit 1,000` ([attribution run 01](../experiments/benchmarks/query-attribution-run-01.md)).

**Strongest prior art.** Fagin, Lotem and Naor 2001 (threshold algorithm for sorted access); Broder et al. 2003 (WAND) and Ding and Suel 2011 (block-max WAND) for block-level upper bounds; ClickHouse's Top-N granule pruning and DataFusion's dynamic TopK filter as production forms.

**Why Fabric's constraints differ.** The sort key is total and time-leading; within a Segment row groups are disjoint in time (the sealer sorts globally); across Segments they overlap when nodes drain backlogs; a page token gives a lower bound as well. Rate queries have no limit.

**Proposed mechanism.** Order the surviving row groups and tail entries by minimum key; scan ascending; once the heap holds `limit + 1` rows with threshold `T`, skip every source whose minimum key exceeds `T` and stop. Use the page's `after` key to skip sources whose maximum key is at or below it. A pure `can_skip(bounds, lower, upper)` in the kernel, proved never to skip a source that could hold a key in the open interval.

**Expected advantage.** For `limit k` over `g` overlapping row groups, from `g` groups read to about `k / 8192` plus the overlap; one group of eight on the attribution state.

**Expected disadvantage.** None in cost; a little ordering logic; the gain shrinks with overlap.

**Novelty risk.** None claimed.

**Falsification test.** False if, on a workload where every Segment's time range overlaps every other's (the adversarial generator of the sealer study), the rows read do not fall at all; or if any answer or page differs from the full scan. The interesting measurement is the gain as a function of overlap, from disjoint to total.

**Smallest prototype.** The ordering and the stop in a scratch `query.rs`; the sealer study's steady, outage and adversarial states.

**Baseline.** The stock scan.

**Promotion criterion.** Differential clean on all three states and on pages; the gain measured against overlap; then Q3 of the direction review with a boundary mutant.

### L-05. Budget-bounded answers with sound partial completeness

**State:** CANDIDATE (a composition).

**Precise problem.** Queries carry no budget; a hostile shape (a long window, `limit 10,000`, a large tail) is bounded only by the data. The charter asks for predictable bounds and graceful degradation.

**Strongest prior art.** Lang, Nehme, Robinson and Naughton 2014 (partial results when sources fail); Zilberstein 1996 (anytime algorithms); Hellerstein, Haas and Wang 1997 (online aggregation); production resource governors (ClickHouse `max_bytes_to_read`, SQL Server Resource Governor).

**Why Fabric's constraints differ.** Fabric's answers already carry set-valued evidence (which sources were covered). A budget-exhausted query need not approximate: it can return the exact answer over the sources it read, list the unread sources as unavailable with the reason "budget", and keep `complete: false`. Nothing is silently truncated, and the retained window and freshness are still claimed only for what was read.

**Proposed mechanism.** A `Budget` (bytes read, rows materialised, deadline) in the query; the disposition gains `Unavailable(Budget)`; the scan stops when the budget is spent; the answer's `unavailable` entries name the reason.

**Expected advantage.** Bounded worst case with a truthful answer; the operator can retry with a narrower window.

**Expected disadvantage.** A contract change (a new reason in `unavailable`; a new request field); partial answers can mislead a client that ignores `complete`.

**Novelty risk.** The composition of partial-result semantics with explicit source evidence; no equivalent was identified in the searched literature under these formulations, but it is a composition, not a new algorithm.

**Falsification test.** False if a property test can find a budget and a state where the rows returned are not exactly the full answer restricted to the sources listed as read, or where `complete` is true with an unread source.

**Smallest prototype.** The property, in the verification layer, over an abstract model of sources; no server change.

**Baseline.** The unbounded path.

**Promotion criterion.** The property holds; a consumer asks for budgets; a contract change in its own commit.

### L-06. Raw bytes plus a key sidecar, no projections

**State:** FALSIFIED for text search by existing measurement; UNRESOLVED otherwise.

**Claim.** Since `batches.parquet` already holds every row's source bytes, the `logs` and `metrics` tables (45 % of a Segment) could be dropped and rebuilt or answered from the raw bytes through a key sidecar, halving Segment size and write amplification.

**Ancestors.** Loki (raw chunks plus a label index); Elasticsearch `_source`; every "WAL plus index" design.

**Why it fails.** Answering from raw bytes is the measured tail cost, 3.2 ms per MiB of protobuf decode: a text search over a 64 MiB Segment would cost about 200 ms per Segment, and over a day of retention, seconds. The projections are what make text search affordable. For node- and time-selective shapes with a key sidecar the cost would be the selected entries' decode, which may be competitive; unmeasured.

**Remaining uncertainty.** Real log corpora may compress the projection far better than the raw bytes (templates), changing the duplication cost; no corpus has been measured.

### L-07. Manifest facts for sound two-sided skipping

**State:** DERIVED from L-02's traps.

**Mechanism.** Add to the manifest, per table, the minimum and maximum of the query key's leading component (observed time for logs, point time for metrics, receive time for gaps), and a per-node presence set that covers gap rows. Then a Segment can be skipped on both sides of a window and by node for every table, each skip justified by a fact about that table. Block-max indexes (Ding and Suel 2011) are the model: bounds per block, per column that is filtered.

**Cost.** A few hundred bytes per manifest; a manifest version bump (ADR-0020 amendment) with a reader that accepts version 1 without the new facts (no skip) and version 2 with them.

**Falsification.** L-02's differential, run against version-2 facts.

### L-08. Query-shaped sidecars (charter H1)

**State:** PRIOR_ART.

**Translation.** In database terms: online physical design tuning and adaptive indexing. COLT (Schnaitter, Abiteboul, Milo and Polyzotis 2006) decides continuously which indexes to build from observed query benefit against build cost; Bruno and Chaudhuri 2007 give the online algorithm with competitive bounds; database cracking (Idreos, Kersten and Manegold 2007) and adaptive merging (Graefe and Kuno 2010) build the index as a side effect of queries; holistic indexing (Petraki, Idreos and Manegold 2015) builds during idle time.

**Fabric's angle.** Canonical evidence is immutable and every sidecar is rebuildable, so the decision per Segment is rent-or-buy: pay the scan on every query, or pay the build once; the classic ski-rental bound applies and the sidecar can be dropped under storage pressure without loss. That is an adaptation of known work, not a new algorithm, and it is gated by the contract until a registered gate fails.

**Next.** None. A registered hostile protocol (L-02's shapes at full retention, text search over a real corpus) is the way to learn whether a gate fails.

### L-09. Progressive evidence search (charter H2)

**State:** REDISCOVERED.

The progression "cheap synopsis, coarse candidate blocks, indexes, materialised columns, raw observations" is, in Fabric's terms, manifest facts (L-02), row-group statistics (present), key columns before payload columns (late materialisation, Q4), raw Batch bytes (the tail, L-03). The literature calls the whole of it multi-level pruning with late materialisation (Abadi et al. 2007). The research question that remains is L-05's: when a budget stops the progression early, what can the answer claim.

### L-10. Information-gain planning (charter H3)

**State:** UNRESOLVED.

With one access path per table (row-group statistics) there is no choice to plan. The prior art for the day there is one: adaptive query processing (Avnur and Hellerstein 2000; Deshpande, Ives and Raman 2007), learned cost models, and the observation that index use is harmful at low selectivity. Revisit when a second access path exists.

### L-11. Causal pruning (charter H4)

**State:** UNRESOLVED, out of contract.

Fabric has no traces, so no causal graph; it has a total order per node (sequence) and a total order per server (group). "What preceded this on the same host within Δ" is a node-and-time window, which L-02 to L-04 serve. Prior art if traces ever enter: interval indexes, reachability indexes (GRAIL and successors), temporal graph indexing, provenance graphs. Nothing to run.

### L-12. Workload-specific physical operators (charter H5)

**State:** REDISCOVERED.

The common shapes are the three query kinds; the operators that fit them are the threshold scan (L-04) and the tail selection (L-03). Both are known mechanisms specialised to Fabric's key.

### L-13. Bounded adaptive telemetry (charter H6)

**State:** PRIOR_ART, with one measured lever.

Prior art: congestion and admission control; adaptive sampling in Dapper and its successors; `tail -f`'s polling. The measured fact: the Spindle's fixed 1 s log poll is 85 % of the median line-to-query time ([collection-to-query run 01](../experiments/benchmarks/e2e-latency-run-01.md)). A poll interval chosen from the observed line rate under a CPU budget is a control problem with a known shape (interval halves while lines arrive, doubles while they do not, bounded both ways). It is a Spindle question with its own ADR; not this path.

### L-14. Incremental investigation (charter H7)

**State:** PRIOR_ART.

Incremental view maintenance (Gupta and Mumick 1995), differential dataflow (McSherry et al. 2013), DBSP (Budiu et al. 2023). Fabric's sources are immutable and its frontier (the committed group) is monotone, so a repeated query's work on unchanged Segments is a cache keyed by (query, Segment); only the tail and retention change. There is no continuous query in the contract to consume it.

### L-15. Query-directed instrumentation (charter H8)

**State:** HUNCH, out of contract.

Prior art: Pivot Tracing (Mace, Roelke and Fonseca 2015) for query-directed dynamic instrumentation; Canopy; active learning and experimental design for choosing what to observe. The contract excludes watchers and plugin boundaries in the Spindle; any active observation would need permission, resource and duration bounds that do not exist. Nothing to run.

### L-16. External merge sort for the sealer

**State:** PROMOTED. [ADR-0022](../decisions/ADR-0022-build-segments-by-external-merge-sort.md); evidence in the [sealer study](../experiments/benchmarks/sealer-study-run-01.md). Prior art: Knuth; nothing claimed.

### L-17. Clock skew between node and server

**State:** UNRESOLVED.

Rows carry the node's clock, gaps and the retained window the server's, freshness the node's claim. No bound on their difference is stated, measured or reported. L-02's hostile state shows that pruning rows by receive time is unsound under a 5 s skew. A per-Batch measurement (receive time minus the Batch's newest observed time) would cost nothing to record and would make skew visible in answers. That is a contract change with a measurement first.

### L-18. Query time and memory are proportional to the unsealed tail

**State:** OBSERVED in [retention-scale run 01](../experiments/benchmarks/retention-scale-run-01.md); not a hypothesis.

`History::sources` decodes every journal frame and keeps every Group in memory, and `run_once` extracts every row, before any filter. Measured: 18 MiB resident after replaying a 320 MiB journal; 876 MiB high-water mark and 714 MiB resident after one query that could match nothing, which took 4.64 s. The cost is about 3.2 ms and 2.7 MiB per MiB of tail, per query, and nothing bounds the tail but `journal_bytes` (4 GiB in the soak configuration). This is the only cost in the audit that can exhaust the server rather than slow it. L-03 removes it; L-05 would bound it meanwhile; the sealer milestone shortens the exposure by keeping the tail at most one file when sealing keeps up, and does nothing when it does not.

