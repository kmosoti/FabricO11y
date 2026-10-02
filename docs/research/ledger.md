# Research ledger

Status: **exploratory research**, tier 6 of the [source-of-truth order](../README.md#source-of-truth). Every significant idea about the query and storage path is recorded here with a state, its ancestors, and what would falsify it. Nothing here is a decision. Failed ideas stay: they bound the search.

States: **HUNCH** (a direction, not yet a mechanism), **PRIOR_ART** (exists in the literature or in production; adopting it is engineering, not research), **REDISCOVERED** (arrived at here, then found in the literature; the finding strengthens the foundation), **DERIVED** (a mechanism written precisely enough to analyse), **CANDIDATE** (derived, with a falsification experiment designed), **EXPERIMENTING**, **FALSIFIED**, **PROMOTED** (accepted into a decision or a milestone), **UNRESOLVED** (no decisive experiment is affordable or in scope yet).

The novelty firewall of the charter applies to every entry: before "novel", the idea was translated into database, networking and information-retrieval terms and searched under each. Where that search was from memory rather than from a live literature search, the entry says "no equivalent was identified in the searched literature under these formulations" and nothing stronger.

## Index

| Id | Idea | State | Next action |
| --- | --- | --- | --- |
| L-01 | Fabric's evidence fields are query completeness under per-source completeness statements | REDISCOVERED | State it in the kernel (Q1 of the [direction review](query-engine-direction.md)) |
| L-02 | Sound Segment dispositions from manifest facts remove the per-Segment cost that grows with retention | DERIVED and confirmed sound; FALSIFIED as a material saving | [Retention-scale run 01](../experiments/benchmarks/retention-scale-run-01.md): the rule changes no answer, the two traps are real, the saving is tens of milliseconds at 319 Segments |
| L-03 | A memtable of keys makes the unsealed tail selectable | CANDIDATE, confirmed on the prototype | [Tail-index run 01](../experiments/benchmarks/tail-index-run-01.md): selective shapes from about 220 ms to 4 to 13 ms, 0 mismatches in 1,500 queries, 95 B per entry; a whole-window query still materialises everything. Promotion needs the soak and the Q2 design |
| L-04 | The threshold algorithm over row-group and entry bounds ends every `limit` scan early | CANDIDATE, confirmed on the prototype | [Threshold run 01](../experiments/benchmarks/topk-run-01.md): whole-window `limit 50` from 252 to 320 ms to 13 to 20 ms on 64 MiB tails, reading 26 entries of 55,000; one row group of 64 on disjoint Segments; no gain at total overlap; the server stays at 56 to 59 MiB where stock reached 242 to 257; 0 mismatches in 1,200 queries and pages. Shapes that cannot stop paid 20 to 80 % for the walk until a 64-frame cache removed the re-decoding ([optimality run 01](../experiments/benchmarks/optimality-run-01.md), A4): the no-match search is now under the stock scan. Promotion with L-03 under Q2 |
| L-05 | Budget-bounded answers that are exact for the sources read and name the rest | CANDIDATE, confirmed on the prototype | [Budget run 01](../experiments/benchmarks/budget-run-01.md): a page ends at the first unread source's minimum key, returns exactly the rows below it, names the boundary and resumes there; drains equal the stock answer; rare text search from 243 to 516 ms a page to 14 to 35 ms, a whole-window rate to 7 to 13 ms, server memory 29 to 69 MiB against 414; no effect when every source starts at the same key; the price is pages. Promotion needs a counter-bearing workload for rates and a contract change in its own commit |
| L-06 | Drop the projections and answer from raw bytes with a key sidecar | FALSIFIED for text search; UNRESOLVED for selective shapes | Measured on real text ([storage layout run 01](../experiments/benchmarks/storage-layout-run-01.md)): the duplicate payload is 63 % of a real-text Segment and the raw copy is the larger half. The remedy ranked first is now the other way round (L-19, L-20: shrink the custody copy, keep the projection) |
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
| L-18 | Query time and memory are proportional to the unsealed tail, unbounded by anything but `journal_bytes` | OBSERVED (run 01); bounded by L-03 and L-04 on the prototype | On real text the tail also holds 2.5 times the bytes of its Segment (uncompressed protobuf against Zstd Parquet). The bounded sealer keeps it short; L-03 and L-04 make queries over it cost their answers |
| L-19 | Hash the custody table per row group instead of per Batch | CANDIDATE | 32 B per Batch is 3.6 % of a synthetic Segment and 17.6 % of a real-text one; a scratch sealer and a replay differential decide whether custody survives without the per-row check |
| L-20 | A custody encoding that stores OTLP framing and repeated attributes once per Batch | HUNCH | On real text the raw copy costs twice the body column; the framing share is unmeasured. Measure it first |
| L-21 | Real-corpus query run | FALSIFIED on the tail, held per entry; held on Segments within 2× except metrics | [Real-corpus query run 01](../experiments/benchmarks/real-corpus-query-run-01.md): the same 64 MiB of real text holds 2.7 times the entries, and the stock tail cost is 4.7 µs per entry on both; Segment shapes within 1.0 to 1.5×; the walk reads the same sources; text selectivity decides its gain (1 %: 706 to 82 ms); the walk's 8.3 µs per entry when nothing stops it is the cost to fix; 0 mismatches in 800 queries |
| L-22 | Delta encodings for the metrics projection | DERIVED | Measured 16 to 29 % of a table that is 3 to 13 % of a Segment; a writer property and an ADR note when a consumer's metric volume makes it matter |
| L-24 | One canonical copy in place of the custody table and both projections (suite D3) | CANDIDATE, text search confirmed | [Optimality run 01](../experiments/benchmarks/optimality-run-01.md): a full text search over FOB1 blocks within 1.2 to 1.4× of the Parquet projection, identical hits, a third of the bytes; the other shapes need per-block key bounds |
| L-25 | A text filter per row group reaches the block floor for rare tokens on real streams | CANDIDATE, measured in [optimality run 01](../experiments/benchmarks/optimality-run-01.md) | A trigram bloom per row group (1 to 2 bytes per row) takes the no-match search from 114 to 126 ms to 7 to 10 ms on either order and skips 17 to 62 % of groups for present tokens in stream order, none on the random draw; 0 mismatches in 400 queries; a contract question (an index beyond row-group statistics) |
| L-26 | Real streams compress 1.1 to 1.7× better than the random draw (suite B3) | OBSERVED | Every random-draw storage figure is pessimistic; the fixture should be stream order |
| L-23 | Traces as a third projection of the same record spine | OUT OF CONTRACT (design held) | The [storage direction](storage-direction.md) says where spans, locators and a `trace_id` bloom would go; nothing runs until the contract changes |

## Ranking of the next experiments

Ranked by what the next experiment would teach, not by the size of the expected speed-up:

Before run 01, L-02 ranked first: it tested a risk nobody had measured (the registered 2 s gate was passed on four Segments; retention allows about 320), it tested the charter's essential pruning invariant with two concrete ways to break it, and every other pruning idea depended on its answer. It was run; the [record](../experiments/benchmarks/retention-scale-run-01.md) and the L-02 entry hold the result. After it:

L-03 ran next ([tail-index run 01](../experiments/benchmarks/tail-index-run-01.md)): the index removes the tail's cost for every selective shape, changes no answer, and costs about 95 bytes per entry, but a query whose window covers the tail still decodes everything, so the index bounds selection and not materialisation. After it:

1. **L-04.** It is now both the retention-scale remedy (the wide-window `limit` shape, 579 ms over 319 Segments of 1 MiB) and the missing half of the tail bound: with the index's entries sorted by their lower time bound, the heap's threshold can stop materialisation after a few frames. What the experiment teaches is the gain against cross-Segment and cross-entry overlap, which no run has measured, on the outage and adversarial tails where overlap is worst.
2. **L-05.** The only bound for shapes that neither index nor threshold can shrink (a rate over a long window; a text search), and the semantics that lets a bounded query still answer truthfully.
3. **L-07.** Design work whose test exists.
4. **L-03's promotion** waits on the registered soak (ACK latency while the index extends on two CPUs) and the Q2 design in the direction review.

L-04 ran next ([threshold run 01](../experiments/benchmarks/topk-run-01.md)): ordering sources by their minimum key and stopping at the heap's threshold makes a `limit` query cost its answer on disjoint history (26 entries read of 55,000; one row group of 64), keeps the server's memory flat where the index alone did not, changes no answer in 1,200 queries, and gains nothing at total overlap or for shapes that cannot fill the heap, which pay for the walk. After it:

1. **L-05** ran ([budget run 01](../experiments/benchmarks/budget-run-01.md)): the boundary semantics hold and bound the shapes L-04 cannot, at a price in pages; it waits on a counter-bearing workload and a contract decision.
2. **L-21** ran ([real-corpus query run 01](../experiments/benchmarks/real-corpus-query-run-01.md)): real text holds 2.7 times the entries per byte, so every per-entry cost weighs more; the walk's per-entry overhead when it cannot stop is now the query-side item to fix.
3. **L-19**, then **L-20**: the custody copy, measured as the largest removable cost of a Segment and heavier per byte on real text.
4. **L-07**, design work whose test exists; **L-03 and L-04's promotion** together under Q2 of the direction review, after the registered soak.

The [hypothesis suite](hypotheses.md) states the next experiments with their nulls and decision rules and ranks them; B3 (a real stream's compression against the random draw) is next, then A1 and A4, then L-19 as B1.

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

**State:** CANDIDATE, confirmed on the prototype; promotion pending the soak. Result in [tail-index run 01](../experiments/benchmarks/tail-index-run-01.md).

**Result.** On five 64 MiB tails (steady, 303,500 tiny entries, 1,000 identities, drained backlog, skewed clocks with gap-only nodes): every selective shape fell from 216 to 238 ms to 4 to 13 ms, the 300-query random set ran twelve times faster, 0 of 1,500 queries and their pages differed from the stock server, a rotation and seal under 1,908 live queries produced no error and the index dropped the sealed entries. Index cost: 92 to 97 bytes per entry (138 on the tiny tail), built in about one stock query's time. What failed: a query whose window covers the tail selects everything and reaches the same high-water mark as stock, and on the tiny tail the wide shapes are slower than stock. The index bounds selection; materialisation needs L-04 or L-05. The ACK falsifier was not run.

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

**Result (run 01, 2026-10-02).** [Record](../experiments/benchmarks/topk-run-01.md). On two 64 MiB tails, whole-window `limit 50` fell from 252 to 320 ms to 13 to 20 ms, reading 26 of 55,000 entries; `limit 1,000` to about 30 ms; the second page costs the same as the first; the server's high-water mark stayed at 56 to 59 MiB through the shapes and 300 random queries where stock reached 242 to 257 MiB and the index alone 398 to 414. On 64 disjoint Segments the walk read one row group of 64 (96 to 8.6 ms). On 64 totally overlapping Segments it read everything and cost 10 to 20 % more than stock, the predicted floor. Shapes that cannot fill the heap (a rare text search; `limit 10,000` over 3,700 points) were 20 to 80 % slower, because the walk decodes entries in key order rather than frames in file order. 0 mismatches in 1,200 random queries and their pages. The differential and the overlap measurement are done; the promotion waits on Q2 with L-03, with a file-order fallback when the first pass over the bounds shows the heap cannot fill.

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

**Result (run 01, 2026-10-02).** [Record](../experiments/benchmarks/budget-run-01.md). The prototype bounds a request by rows examined over the L-04 walk and answers exactly the rows below the first unread source's minimum key, `complete: false`, with the boundary named and a token that resumes there. On four states and three budgets, every bounded page obeyed the rule and every drain concatenated to the stock answer; the shapes the threshold cannot stop (a rare text search, a whole-window rate, `limit 10,000`) answered their first page in 7 to 35 ms where stock took 229 to 545 ms, at a cost in pages (12 become 113 at 2,000 rows). The harness found and the run kept a progress defect in the first boundary rule (a boundary one past the page start yields an empty page and no progress). What the budget cannot do: cut between sources that start at the same key (the overlapping state pays the full scan per page) or bound a rate's rows on a gauge-only workload, which left the rate oracle vacuous. Next: a counter-bearing workload; a boundary inside a source using the heap's threshold; then the contract change.

### L-06. Raw bytes plus a key sidecar, no projections

**State:** FALSIFIED for text search by existing measurement; UNRESOLVED otherwise.

**Claim.** Since `batches.parquet` already holds every row's source bytes, the `logs` and `metrics` tables (45 % of a Segment) could be dropped and rebuilt or answered from the raw bytes through a key sidecar, halving Segment size and write amplification.

**Ancestors.** Loki (raw chunks plus a label index); Elasticsearch `_source`; every "WAL plus index" design.

**Why it fails.** Answering from raw bytes is the measured tail cost, 3.2 ms per MiB of protobuf decode: a text search over a 64 MiB Segment would cost about 200 ms per Segment, and over a day of retention, seconds. The projections are what make text search affordable. For node- and time-selective shapes with a key sidecar the cost would be the selected entries' decode, which may be competitive; unmeasured.

**Remaining uncertainty.** Real log corpora may compress the projection far better than the raw bytes (templates), changing the duplication cost; no corpus has been measured.

**Measured since (2026-10-02).** [Storage layout run 01](../experiments/benchmarks/storage-layout-run-01.md) sealed 64 MiB of real log lines: the raw copy is 43 % of the Segment at 5.4:1 and the body column 20 % at 6.2:1, so the raw copy costs twice the projection (the OTLP framing and five file attributes travel with every line). The duplicate is the Segment's largest cost on real text as on synthetic, but the cheaper half to keep is the projection. The question inverts into L-19 and L-20: shrink the custody copy without losing custody. Option C of the [storage direction](storage-direction.md) (projection without body, decode on demand) stays open only for selective shapes under L-03 and L-04, and L-21 measures it.

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

### L-19. Hash the custody table per row group

**State:** CANDIDATE.

**Precise problem.** `batches.parquet` stores a 32-byte SHA-256 per Batch so that replay can check each row ([store.rs](../../crates/fabric-server/src/store.rs) `scan_batches`). The column is uncompressible: 3.6 % of a synthetic Segment and 17.6 % of a real-text one, where Batches are two short lines ([storage layout run 01](../experiments/benchmarks/storage-layout-run-01.md)).

**Strongest prior art.** Prometheus chunks carry one CRC32C per chunk, not per sample; Parquet carries page-level CRCs; the manifest already hashes each file.

**Why Fabric's constraints differ.** Replay must detect a corrupted or substituted Batch before it re-enters stream state; a per-row-group hash detects it per 8,192 rows and names the group, not the row. The node's own bytes carry no hash of their own; the server's hash is the custody claim.

**Proposed mechanism.** Hash per row group in the manifest (`files.batches.row_groups[i].sha256`), computed over the row group's `batch` values in order; replay hashes a row group as it reads it and refuses the Segment on mismatch. Manifest version 2; version-1 readers unchanged.

**Expected advantage.** 6 to 28 % of a Segment; nothing on the query path.

**Expected disadvantage.** A corrupted row is located to a group, not a row; a persisted-format change (ADR-0020 amendment, own commit).

**Falsification test.** False if replay from a Segment with one flipped byte in `batch` is not refused, or if HIST-1/HIST-2 differentials change, or if replay time grows materially.

**Smallest prototype.** A scratch sealer and reader; the real-text and synthetic states; a flipped-byte negative control.

**Baseline.** The per-Batch hash.

**Promotion criterion.** The negative control refused; HIST-1/2 clean; a written ADR amendment.

### L-20. A custody encoding that stores framing once

**State:** HUNCH.

**Precise problem.** On real text the raw Batch costs 77 bytes for two lines whose body column costs 37; the difference is OTLP framing and five file attributes repeated per record, compressed in blocks but still paid.

**Strongest prior art.** Tempo's nested schema (resource attributes once per resource); OTel Arrow's attribute tables keyed by parent id; CLP's separation of static text from variables.

**Why Fabric's constraints differ.** The custody copy must reproduce the node's exact bytes, byte for byte, because the hash and replay depend on them. A re-encoding that is not bijective breaks custody; one that is bijective is a codec, pure by nature, adapter-support by ownership.

**Proposed mechanism.** Measure first: the share of a real Batch that is framing and repeated attributes. If it is most of the difference, a custody format of (framing template once, per-record variable fields, lines) with a proven exact round trip.

**Falsification test.** False if any Batch fails to round-trip byte for byte under a fuzzer over the OTLP encoder's freedom (field order, varint lengths), or if the share measured is small.

**Smallest prototype.** The measurement, with `pyarrow` over the real-text state's Batches; no server change.

### L-21. Real-corpus query run

**State:** CANDIDATE, ranked first among the storage questions.

**Precise problem.** Every query figure in the ledger comes from synthetic lines (half repeated bytes, half random) that contain no templates; text-search selectivity, body decode and per-row cost may differ on real text, and the real-text state ([storage layout run 01](../experiments/benchmarks/storage-layout-run-01.md)) now exists.

**Proposed experiment.** The attribution shapes and the L-04 shapes on the real-text state, stock server and the L-04 prototype; per-shape time and rows read; then the same with the tail unsealed. No code change.

**Falsification test.** The hypothesis is that the synthetic figures transfer within a factor of two. False if a shape differs by more.

**Promotion criterion.** None; it calibrates L-06, L-19 and L-20.

**Result (run 01, 2026-10-02).** [Record](../experiments/benchmarks/real-corpus-query-run-01.md). On the unsealed tail every stock shape cost 2.2 to 3.1 times its synthetic figure, which falsifies the hypothesis as stated; the cause is entries per byte (149,585 against 55,000 in 64 MiB), and per entry the cost is the same 4.7 µs. The right unit for the tail's cost is the entry, not the megabyte. On Segments the log shapes transferred within 1.0 to 1.5 times; metric and rate shapes ran 1.9 to 2.5 times because real text packs 2.7 times the points into the same bytes. The walk prototype read the same sources on real as on synthetic text and its gain on a text search is set by selectivity (124 entries read at 35 %, 4,412 at 1 %, 50,556 at 0.1 %); when nothing stops it, it costs 8.3 µs per entry against the stock path's 4.8, so the file-order fallback and a cheaper per-entry extract are worth more than the synthetic figures suggested. 0 mismatches in 800 random queries with pages. What it decides for the storage entries: real text raises records per byte, so every per-record cost (the per-Batch hash of L-19, the custody framing of L-20, the walk's per-entry work) weighs more on real workloads than on the soak generator.

### L-22. Delta encodings for the metrics projection

**State:** DERIVED.

**Mechanism.** `DELTA_BINARY_PACKED` on `time_ns`, `start_ns`, `group`, `sequence` and `value_int`; reader-transparent in Parquet. Measured 16 to 29 % off the metrics table, which is 3 to 13 % of a Segment ([storage layout run 01](../experiments/benchmarks/storage-layout-run-01.md)); the value column's share is a floor, since the generator's values are random.

**Cost.** A writer property; a note in ADR-0020's consequences since the bytes of a Segment change; HIST-1/2 unchanged.

**Falsification.** None needed; the question is whether a consumer's metric volume makes the saving matter.

### L-23. Traces as a third projection

**State:** OUT OF CONTRACT; design held in the [storage direction](storage-direction.md).

**Claim.** A span is an observation with the same key (start time, node_id, sequence, index), locators (`trace_id`, `span_id`, `parent_span_id`) that logs and metric exemplars share as nullable columns, and a per-row-group bloom filter on `trace_id` for the "fetch one trace" shape. The wire `Batch` needs a traces payload slot (a wire change); the product contract names traces a non-goal; the contract allows no index beyond row-group statistics unless a gate fails. Nothing runs.

### L-24. One canonical copy in place of the custody table and both projections

**State:** CANDIDATE; the text-search shape confirmed in [optimality run 01](../experiments/benchmarks/optimality-run-01.md).

**Claim.** With the record canonical (ADR-0023), a Segment can hold FOB1 blocks once and answer every registered shape from them through `decode_view`, with per-block key bounds in the manifest in place of Parquet row-group statistics, at a third of today's bytes. The text search, the shape that falsified L-06 against raw OTLP, runs within 1.2 to 1.4× of the projection: the per-byte cost of the view decode (0.8 ns) is what L-06 lacked. **Open:** the thirteen other shapes against block bounds (hypothesis D3 in full); the manifest version; replay from blocks.

### L-25. A text filter per row group

**State:** CANDIDATE; measured in [optimality run 01](../experiments/benchmarks/optimality-run-01.md) (hypothesis C5). A 2^16-bit bloom over the byte trigrams of each row group's bodies (8 KiB per group, two positions per trigram, built at 0.8 µs per row) lets the walk skip every group that cannot hold the needle. Measured on two 64-Segment real-text states: the no-match search falls from 114 to 126 ms to 7 to 10 ms on both stream and random order (every group rejected); for present tokens the filter skips 17 to 62 % of the 4,690-line groups in stream order and none on the random draw, so its gain for present tokens is the locality's and the row group's grain (D4), as the derivation said; the walk's early stop already makes those shapes cost 1 to 3 groups. 0 mismatches in 400 random queries. It is an index beyond row-group statistics, which the product contract allows only when a registered gate fails; the floor it reaches is stated in the [optimality bounds](optimality-bounds.md). **Open:** the contract decision; the filter written at seal time into the manifest rather than built per process; its size at 8,192-row groups (1 byte per row).

### L-26. Stream order

**State:** OBSERVED. Real lines in their own order compress 1.08 to 1.68× better than the same lines shuffled (1.34× lines-weighted); the random-draw corpus understates every storage figure by that much. The fixture decision is in the [consolidation](design-consolidation.md) (D8).
