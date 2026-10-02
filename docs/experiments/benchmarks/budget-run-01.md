# Budget-bounded answers, run 01

Status: **Exploratory.** This is the fourth experiment of the [research ledger](../../research/ledger.md) (entry L-05). No protocol was registered before it ran, so it is not **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense and decides no gate. It ran on 2026-10-02 with the stock server built from the code at `58b694d` and a research prototype of the same server, selected by environment variables and never part of the product. The prototype changes the shape of an answer; nothing here changes the product contract, which needs its own commit ([specification protection](../../../AGENTS.md#specification-protection)).

The question: can a query that stops after a fixed amount of work still answer truthfully, so that a client that follows pages reaches exactly the full answer and a client that reads one page knows what it covers?

## Method

**The semantics.** The threshold walk of [run L-04](topk-run-01.md) visits sources (tail entries and Parquet row groups) in order of their minimum key. If the walk stops at a source whose minimum is `m` because a budget is spent, every source with a minimum below `m` has been read, so every row with a key below `m` is known. The prototype ([diff](data/budget/query-budget-prototype.diff.txt)) therefore answers with exactly the rows below `m`, marks the answer `complete: false`, lists in `unavailable` one entry `{"budget": {"rows_examined": n, "boundary_ns": m}}`, and issues a page token that resumes at `m`. A client that follows pages receives the full answer in the same order as the stock server, cut into more pages; a client that stops knows the answer is exact below `m` and says nothing about what lies beyond it. A rate query stops the same way and is computed over `[from, m)`; it has no page, so its answer is the exact rate over the shorter window it names.

**Progress.** A boundary is only taken at a source whose minimum is strictly above the source last read and at least two past the page's start key, so the next token is strictly greater than the current one and a drain terminates. The first version of the rule allowed a boundary one past the page start, which can produce a page with no rows and an unchanged token; the harness's first run found it on the steady Segment state (a metric drain made three rows of progress per page and hit the page cap), the rule was corrected, and the whole run was repeated with the corrected binary ([first attempt's data](data/budget/budget-attempt1-before-progress-fix.json), [page-by-page diagnostic](data/budget/budget_diag.py.txt)). The harness now also refuses a page whose token did not advance.

**The budget** is rows examined: rows extracted from tail entries and rows read from row groups before the filters, checked before each source is read. It is the unit the attribution run measured cost in (about 0.63 µs per log row materialised); bytes or a deadline would be alternatives with the same semantics. The prototype takes it from `FABRIC_PROTO_BUDGET`; its absence is the unbounded walk of run L-04.

**Oracle.** The stock server's answer with every page followed. A budgeted drain is correct when the concatenation of its pages' rows equals the stock concatenation; every page that names a boundary is `complete: false`, holds no row at or beyond it and carries a next page; every page without a boundary is `complete: true`; and a budgeted rate equals the stock rate over `[from, boundary)`. With no budget every answer must equal the stock answer field for field.

**States, modes, measurements.** The four states of run L-04 (steady and outage tails of 64 MiB; 64 disjoint and 64 totally overlapping Segments of 1 MiB). Budgets: none, 50,000, 10,000 and 2,000 rows. Shapes over the whole window: logs `limit 10,000` and `limit 50`, a rare text search, one metric `limit 10,000`, a rate; each drained to its last page or to 400 pages, recording the first page's time, the slowest page's time, the total and the number of pages. Then 120 random queries drained up to 60 pages and checked by the oracle; drains that reached the 60-page cap on both servers are re-run without a cap and compared whole ([re-check](data/budget/budget_recheck.py.txt)). Server on CPUs 0 and 1 ([harness](data/budget/budget.py.txt), [results](data/budget/budget.json), [log](data/budget/run.log.txt)).

## Results

Milliseconds for the first page, the slowest page and the whole drain, and the number of pages (bounded pages in brackets). Stock, then the prototype at each budget.

**Steady tail**

| Shape | stock | no budget | 50,000 | 10,000 | 2,000 |
| --- | --- | --- | --- | --- | --- |
| logs `limit 10,000` | 353 / 455 / 4,277; 12 | 193 / 452 / 2,459; 12 | 182 / 550 / 2,653; 12 | 74 / 136 / 2,120; 23 (22) | **19 / 65** / 3,196; 113 (112) |
| logs `limit 50`, 400 pages | 284 / 398 / 110,056 | 17 / 22 / 4,848 | 17 / 48 / 4,427 | 22 / 392 / 5,996 | 18 / 56 / 5,438 |
| rare text, `limit 100` | 243 / 252 / 495; 2 | 516 / 516 / 573; 2 | 101 / 153 / 542; 5 (4) | 35 / 37 / 718; 23 (22) | **14 / 27** / 1,975; 113 (112) |
| one metric, `limit 10,000` | 307; 1 | 124; 1 | 74 / 77 / 194; 3 (2) | 16 / 21 / 207; 13 (12) | **14 / 14** / 500; 62 (61) |
| rate | 242 | 109 | 67 (1) | 13 (1) | **7** (1) |

**Outage tail**: the same pattern; the rare text search 242 ms stock, 458 ms unbounded, 20 / 29 ms per page at 2,000; the rate 229 ms stock, 9 ms at 2,000.

**64 disjoint Segments**

| Shape | stock | no budget | 50,000 | 10,000 | 2,000 |
| --- | --- | --- | --- | --- | --- |
| logs `limit 10,000` | 545 / 545 / 2,719; 12 | 163 / 174 / 1,657; 12 | 187 / 187 / 1,642; 12 | 165 / 165 / 1,326; 13 (10) | **57 / 65** / 1,666; 60 (59) |
| rare text | 130 / 130 / 241; 2 | 97 / 97 / 123; 2 | 47 / 48 / 113; 3 (2) | 16 / 22 / 206; 13 (12) | **14 / 18** / 715; 60 (59) |
| one metric, `limit 10,000` | 91; 1 | 137; 1 | 40 / 41 / 102; 3 (2) | 17 / 24 / 172; 10 (9) | **14 / 23** / 449; 40 (39) |
| rate | 55 | 64 | 29 (1) | 14 (1) | **9** (1) |

**64 overlapping Segments** (every Segment's range covers every other's)

| Shape | stock | no budget | 50,000 | 10,000 | 2,000 |
| --- | --- | --- | --- | --- | --- |
| logs `limit 10,000` | 330 / 330 / 2,809; 12 | 250 / 439 / 2,926; 12 | 89 / 598 / 6,459; 46 (34) | 36 / 266 / 7,042; 70 (58) | 13 / **505** / 7,585; 74 (62) |
| rare text | 132 / 132 / 231; 2 | 140 / 140 / 251; 2 | 51 / 108 / 2,856; 36 (34) | 16 / 125 / 3,665; 60 (58) | 10 / **133** / 4,341; 64 (62) |
| one metric, `limit 10,000` | 95; 1 | 95; 1 | 33 / 130 / 1,807; 29 (28) | 14 / 186 / 2,265; 42 (41) | 13 / **94** / 2,236; 45 (44) |
| rate | 59 | 93 | 45 (1) | 13 (1) | **9** (1) |

**Correctness.** With no budget, every shape and all 480 random drains matched the stock server (0 mismatches on four states). With budgets, every page that named a boundary was `complete: false`, held no row at or past its boundary and carried a next page; every drain that reached its last page concatenated to exactly the stock rows; no token failed to advance. The harness flagged 90 random drains and three shape drains on which both servers hit the page cap with the budgeted pages holding fewer rows; of these, 16 were re-run without a cap ([recheck.json](data/budget/recheck.json)): the 7 on the tails and the disjoint Segments and 9 on the overlapping Segments; 13 drained to the end and were equal to stock (up to 1,731 budgeted pages against 1,705), and 3 (`limit 1` and `limit 10` over the whole window on the overlapping Segments) reached even a 5,000-page cap on both servers with the budgeted drain 14 to 140 rows shorter, which the capped comparison can neither confirm nor refute; the remaining 74 flagged drains on the overlapping Segments were not re-run, each taking minutes at a thousand pages. No re-run drain that reached its end differed from stock. Memory: the server's high-water mark stayed at 29 to 69 MiB under every budget (stock: 414 MiB on the steady tail, 226 on the outage tail, 51 to 55 on the Segment states).

**What the budget could not do.** The rate half of the oracle is vacuous on these states: the generator emits only gauges, so every rate answer, stock or budgeted, is empty; the budgeted rate's boundary and `complete: false` were exercised, its rows were not. On the overlapping Segments, every row group starts at nearly the same time, so no source qualifies as a boundary until the walk has read them all; the page is then exact and complete, and it costs what the stock scan costs (505 ms for the slowest page at a 2,000-row budget). A budget bounds work to the granularity of distinct source minimums, not below it.

## Findings

- **The semantics hold.** Exact rows below a named boundary, `complete: false`, a token that resumes at the boundary: a client that drains receives the stock answer, cut into more pages, and a client that stops knows what it has. 480 unbudgeted and 1,440 budgeted random drains across four states, with every page checked, found no page that broke the rule once the progress defect was fixed.
- **It bounds the shapes the threshold walk could not.** The rare text search, which never fills the heap, fell from 243 to 516 ms a page to 14 to 35 ms at budgets of 2,000 to 10,000 rows, on the tails and on disjoint Segments; a rate over the whole window from 229 to 242 ms to 7 to 13 ms. `limit 10,000` over the tail, 353 ms stock, answers its first page in 19 ms at 2,000 rows.
- **The price is pages.** A 2,000-row budget turns a 12-page drain into 113 pages on the tail and the total time of a full drain is unchanged or higher (3.2 against 2.5 s unbounded on the steady tail); the budget trades latency and memory per request for round trips. Per-page time also grows with the number of sources already passed, because the prototype re-sorts every source's bounds on each page and walks past the ones below the page start; a binary search on the sorted bounds would remove that term.
- **Overlap sets the floor, as for L-04.** When every source starts at the same key no boundary exists before the end, and a page costs the full scan. The remedy is finer sources (row groups of 8,192 rows in a 64 MiB Segment are disjoint within the Segment) or a budget that may stop inside a source and name a key rather than a source minimum; the latter is the same rule with the heap's own threshold as the boundary.
- **Two defects were found by the harness and are kept**: the zero-progress boundary (fixed in the rule, the first attempt's data kept), and a harness bug that sent page fields to rate queries and crashed the stock reference on an empty window. The vacuous rate check is a gap in the states, not in the method, and a counter-bearing workload is the next thing to add.

## Limits

- One host, warm page cache, synthetic workloads; eight-repetition medians for the L-04 shapes, single drains here.
- Rates were not exercised beyond their boundary and completeness fields (gauges only).
- The 400-page cap bounds the `limit 50` drains on every state; their totals compare 400 pages against 400 pages.
- The prototype holds the index lock for the whole page, as in runs L-03 and L-04.
- A budget in rows examined is not a budget in time: a source's rows cost 0.6 µs each to materialise, but a cold Parquet row group or a frame decode costs more than its rows, and the overlapping state shows a page of 2,000 examined rows taking 500 ms because the budget could not take effect.

## Reproduce

[budget.py](data/budget/budget.py.txt) takes an output root, the CPUs for the server and optional state names; it expects the stock and prototype servers under `bin/stock` and `bin/budget`, the L-03 tails and the L-04 Segment states. [budget_recheck.py](data/budget/budget_recheck.py.txt) re-runs the flagged drains without a cap. Results: [budget.json](data/budget/budget.json), [run.log.txt](data/budget/run.log.txt).
