# Threshold algorithm over source bounds, run 01

Status: **Exploratory.** This is the third experiment of the [research ledger](../../research/ledger.md) (entry L-04). No protocol was registered before it ran, so it is not **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense and decides no gate. It ran on 2026-10-02 with the stock server built from the code at `58b694d` and a research prototype of the same server, selected by environment variables and never part of the product.

The question: when every source (a tail entry or a Parquet row group) is visited in order of its lowest possible key, and the scan stops once the heap of `limit + 1` rows cannot change, how much of the work of a wide-window `limit` query disappears, how much does cross-source overlap take back, and does any answer or page change?

Origin: [query attribution run 01](query-attribution-run-01.md) found `limit 50` costs the same as `limit 1,000`; [retention-scale run 01](retention-scale-run-01.md) found the whole-window `limit` shape grows with the rows retained; [tail-index run 01](tail-index-run-01.md) found the tail key index bounds selection and not materialisation.

## Method

**The prototype** ([diff](data/topk/query-topk-prototype.diff.txt), on top of the tail key index of run L-03; the diff also carries the budget of [run L-05](budget-run-01.md), which was off here). With `FABRIC_PROTO_TOPK=on`, for a logs or metrics query the server builds one list of items: every selected tail entry that carries rows of that table, with the entry's minimum and maximum row time from the index, and every row group of every surviving Segment's table whose time statistics overlap the window, with the group's minimum and maximum from the Parquet footer. It sorts the list by minimum, then walks it. An item whose maximum is below the page's `after` key is skipped. Once the heap holds `limit + 1` rows and the item's minimum exceeds the heap's largest key (the threshold), the walk stops: no later item can hold a smaller key, because the list is ordered by minimum. A tail item decodes its frame (one frame is cached between consecutive entries of the same frame) and extracts its one entry; a row-group item reads that one row group through the stock Parquet path. Gap-bearing tail entries stay on the stock path, since gaps are keyed by receive time. Rate queries have no limit; the prototype decodes their selected tail entries lazily but cannot stop early. Everything after the heap is stock code, so the stock server is the oracle. The prototype prints per query how many items it had, how many it processed, how many it skipped below the page bound and whether it stopped.

**States.** Two unsealed tails of 64 MiB from run L-03 (steady, and outage: 20 nodes offline for 240 s then draining a backlog; 55,000 entries each), and two Segment states of 64 Segments of 1 MiB each, sealed by the stock server, so each Segment's logs table is one row group of 1,630 rows: steady (the first 64 files of retention-scale run 01) and adversarial (the sealer study's workload in which every node's clock and backlog differ, so every Segment's time range overlaps every other's).

**Modes.** On the tails: stock; index (L-03); index with the threshold walk. On the Segment states: stock; the threshold walk (there is no tail). Server on CPUs 0 and 1.

**Measurements.** Per mode: ten shapes, eight repetitions each over one keep-alive connection, the median reported; the second page of the `limit 50` shape; memory after the shapes; 300 random queries with every page followed up to three hops, compared field by field with the stock server's answers; memory after. Shapes over the whole retained window unless named: logs `limit 50`, `limit 1,000`, `limit 10,000`; one node's logs `limit 50`; one metric `limit 100` and `limit 10,000`; a rare text search `limit 100`; a rate; logs of the last 10 s; an empty window in the past.

## Results

Median milliseconds per query; in brackets, the sources the walk read out of the sources it had.

**Unsealed tails** (stock / index / index with the walk):

| Shape | steady | outage |
| --- | ---: | ---: |
| logs `limit 50`, whole window | 320 / 300 / **20** [26 of 55,000] | 252 / 295 / **13** [26 of 55,000] |
| the same, second page | 298 / 325 / **20** | 360 / 304 / **13** |
| logs `limit 1,000`, whole window | 293 / 297 / **35** [502] | 300 / 399 / **27** [502] |
| logs `limit 10,000`, whole window | 399 / 410 / **178** [5,002] | 376 / 441 / **216** [5,002] |
| one node, `limit 50`, whole window | 306 / 11 / **6.5** [26 of 550] | 233 / 13 / **8.3** |
| one metric, `limit 100`, whole window | 323 / 104 / **11** [102 of 3,700] | 254 / 98 / **14** |
| one metric, `limit 10,000`, whole window | 275 / 180 / 163 [3,700 of 3,700] | 326 / 162 / 174 |
| rare text, `limit 100`, whole window | 260 / 309 / 468 [50,346] | 281 / 392 / 480 [51,454] |
| rate, whole window | 262 / 108 / 103 | 273 / 115 / 126 |
| logs, last 10 s | 267 / 10 / 8.9 | 264 / 8.5 / 6.3 |
| empty window in the past | 241 / 5.1 / 6.8 | 281 / 6.3 / 4.7 |

**Segment states** (stock / the walk):

| Shape | steady, 64 disjoint Segments | adversarial, 64 overlapping Segments |
| --- | ---: | ---: |
| logs `limit 50`, whole window | 96 / **8.6** [1 of 64] | 120 / 71 [37 of 64] |
| the same, second page | 120 / **7.8** | 117 / 105 |
| logs `limit 1,000`, whole window | 128 / **19** [1] | 117 / 140 [64] |
| logs `limit 10,000`, whole window | 223 / **131** [6] | 251 / 291 [64] |
| one node, `limit 50`, whole window | 97 / **17** [4] | 106 / 128 [64] |
| one metric, `limit 100`, whole window | 84 / **11** [2 of 45] | 70 / 75 [45] |
| one metric, `limit 10,000`, whole window | 102 / 117 [45 of 45] | 96 / 118 [45] |
| rare text, `limit 100`, whole window | 103 / 117 [59 of 64] | 105 / 120 [64] |
| rate, whole window | 61 / 83 | 67 / 88 |
| logs, last 10 s | 8.9 / 8.6 | 6.5 / 8.0 |
| empty window in the past | 6.3 / 6.9 | 6.4 / 8.8 |

The random set of 300 queries with their pages, seconds; then the server's high-water mark in MiB after the shapes and after the random set:

| State | stock | index | index with the walk |
| --- | ---: | ---: | ---: |
| steady tail | 169 s; 219, 242 MiB | 46 s; 215, 398 MiB | **14 s; 56, 59 MiB** |
| outage tail | 168 s; 244, 257 MiB | 45 s; 225, 414 MiB | **15 s; 56, 57 MiB** |
| steady Segments | 26 s; 49, 67 MiB | | 15 s; 49, 72 MiB |
| adversarial Segments | 52 s; 49, 55 MiB | | 55 s; 60, 74 MiB |

**Mismatches: 0 of 1,200 random queries and their pages across the four states.** Every shape returned the same rows under every mode.

## Findings

- **A `limit` query over a disjoint history now costs its answer, not its window.** On the tails, `limit 50` over the whole window fell from 252 to 320 ms to 13 to 20 ms, reading 26 entries of 55,000; `limit 1,000` to about 30 ms reading 502; the second page costs the same as the first. On 64 disjoint Segments the walk reads one row group where the stock scan read 64 (96 to 8.6 ms). The rows read are `limit + 1` plus one source's worth, as L-04 predicted.
- **It completes the tail bound that L-03 began.** With the index alone, a whole-window query reached the stock high-water mark and the random set left 398 to 414 MiB resident, because every selected entry was cloned; with the walk the server stays at 56 to 59 MiB through the shapes and the random set, against the stock server's 242 to 257. The random set runs twelve times faster than stock and three times faster than the index alone.
- **Overlap takes it back, as predicted, and the adversarial state shows the floor.** When every Segment's range covers every other's, every source's minimum lies below the threshold, so the walk reads everything the stock scan read, plus about 10 to 20 % for ordering, footer reads and one-group reads (140 against 117 ms for `limit 1,000`). Even there `limit 50` read 37 of 64 groups (71 against 120 ms). The gain is a function of overlap, from one source at zero overlap to no gain at total overlap, and the sealer's global sort gives zero overlap within a Segment; across Segments only backlog drains overlap, and only for the draining nodes.
- **Shapes that cannot stop pay for the walk.** A rare text search that never fills the heap, and `limit 10,000` over 3,700 metric points, read every source as before and are 20 to 80 % slower on the tails (468 against 260 ms for the text search), because the walk decodes entries one at a time in key order rather than frames in file order. These shapes need a budget (L-05) or a cheaper per-entry path; the walk should fall back to file order when the heap cannot fill, which is known after the first pass over the bounds.
- **No answer changed**: 1,200 random queries with pages, including the drained backlog and the overlapping Segments, matched the stock server field for field.

## Limits

- One host, warm page cache, synthetic workloads, eight repetitions per shape.
- Segments of 1 MiB with one row group each; at the product's 64 MiB Segments the row groups within a Segment are disjoint, so the Segment-state numbers understate the gain for a single large Segment and overstate the per-source overhead.
- The prototype orders by the row-time bound only; the page's `after` key is applied at the time component. A total-order bound per source would skip more on the second page of a dense second.
- The walk holds the index lock for the whole query, as in run L-03.

## Reproduce

[topk.py](data/topk/topk.py.txt) takes an output root; it expects the stock and prototype servers under `bin/stock` and `bin/proto`, the L-03 tails and the retention-scale files. Results: [topk.json](data/topk/topk.json), [run.log.txt](data/topk/run.log.txt).
