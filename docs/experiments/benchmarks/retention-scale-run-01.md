# Retention scale and Segment dispositions, run 01

Status: **Exploratory.** This run is the first experiment of the [research ledger](../../research/ledger.md) (entry L-02). No protocol was registered before it ran, so it is not **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense and decides no gate. It ran on 2026-10-01 and 2026-10-02 with the stock server built from the code at `58b694d`, and a research prototype of the same server with Segment-level dispositions, selected by an environment variable and never part of the product.

Three questions:

1. How does a query's cost grow with the number of retained Segments? The registered history gate (2 s) was measured on three or four Segments; the contract's retention allows about 320 of 64 MiB.
2. Can a Segment be skipped soundly from the facts its manifest already holds, and how much does that save?
3. Are the two traps found in the [frontier map](../../research/frontier-map.md#3-current-indexes-and-pruning) real: that receive-time bounds do not bound row times when a node's clock runs ahead, and that a node absent from the freshness map may still have gap rows?

## Method

**States.** The sealer-study generator ([source](data/retention-scale/sealbench-driver-skew.rs.txt)) wrote journals of the steady workload (100 nodes, two 512-byte log lines per node per second, 32 gauge points every 15 s, a gap text every 500th Batch) in 1 MiB files, so that hundreds of Segments fit this container: 320 MiB gave 319 sealed files. For each point, the first N files were linked into an empty state directory and the stock server was started on it; it replayed the journal, and its sealer built one Segment per file and reclaimed the files. The server was then stopped and restarted, so that every measurement below ran on a state of N Segments and an empty journal. Each Segment holds about 8 s of data, 756 KB on disk, and one row group per table.

**Dispositions.** The prototype ([diff](data/retention-scale/query-disposition-prototype.diff.txt)) decides per Segment, from the manifest alone and before opening any file:

| Mode | Skip the logs and metrics scans when | Skip the gaps scan when |
| --- | --- | --- |
| `sound` | the newest time per node, maximised over nodes, is below `from_ns`; or the queried node is absent from the freshness map | `received_max_ns < from_ns` or `received_min_ns >= to_ns` |
| `naive` (negative control) | `received_max_ns < from_ns`; or the queried node is absent | the same, or the queried node is absent |

The receive bounds and freshness of a skipped Segment still fold into the answer's evidence fields.

**Shapes.** Seven queries, each ten times over one keep-alive connection, the median reported: an empty window in the past; logs of the last 10 s; one node's logs over the last 60 s; a rare text search over the last 60 s; one metric over the fleet for the last 15 s; a rate over the last 60 s; and logs with `limit 50` over the whole retained window. "Last" is measured from the state's newest receive time. At 319 Segments the stock server ran twice, before and after the prototype, to show the effect of cache order.

**Differential.** A sixteen-Segment state from a hostile variant of the workload: nodes 0, 10, …, 90 have clocks 5 s ahead of the server's (every Segment's newest row time exceeds its receive bound by 4.1 to 4.6 s), and nodes 7, 32, 57 and 82 send Batches that carry only a gap text, so they appear in no freshness map. 440 queries: 400 drawn at random (kind logs, logs, metrics or rate; `from_ns` a Segment's receive maximum plus 2 s before to 8 s after; a window of 1 to 120 s; the node filter absent, a node with rows, a random node id or an absent node; limits of 1 to 10,000; a `contains` on 30 % of log queries) and 40 aimed at the gap-only nodes; every page token followed for up to three pages. The same queries ran against the stock server, the `naive` prototype and the `sound` prototype on the same state, and the full answers were compared field by field.

**Memory.** A separate start on the 319-file journal recorded the server's resident and high-water memory after replay and before any query, then after one empty-window query, which decodes the whole unsealed tail.

## Results

Median milliseconds per query; stock, then the `sound` prototype; at 319 Segments also the stock server's second pass:

| Shape | 1 Segment | 8 | 64 | 319: stock | 319: sound | 319: stock again |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| empty window | 0.98 / 0.95 | 1.52 / 1.24 | 5.82 / 4.63 | 26.7 | 19.1 | 25.7 |
| logs, last 10 s | 3.22 / 3.43 | 5.97 / 6.26 | 9.12 / 5.82 | 28.8 | 15.2 | 28.5 |
| one node, last 60 s | 2.28 / 2.18 | 17.5 / 15.6 | 19.9 / 17.2 | 39.2 | 24.5 | 38.1 |
| text, last 60 s | 2.20 / 1.96 | 14.3 / 13.9 | 19.3 / 16.4 | 40.9 | 23.1 | 38.1 |
| fleet metric, last 15 s | 2.69 / 2.68 | 4.27 / 3.92 | 9.15 / 5.11 | 34.0 | 12.7 | 30.0 |
| rate, last 60 s | 2.07 / 1.94 | 6.71 / 6.74 | 12.4 / 9.63 | 36.2 | 16.4 | 36.9 |
| logs, `limit 50`, whole window | 2.82 / 2.94 | 17.6 / 16.4 | 122 / 86 | 579 | 563 | 507 |

Every shape returned the same rows under both servers at every point.

Start-up: time to the first answered query with the journal still unsealed was 0.17 s, 0.23 s, 1.5 s and 7.09 s for 1, 8, 64 and 319 files; the separate memory run splits the last figure into 1.72 s of replay and 4.64 s for the first query over the 319 unsealed files. Sealing all files took 0.0, 0.2, 0.6 and 3.41 s. With the Segments built and the journal empty, every start was ready in 0.15 s.

Memory: 18 MiB resident after replaying the 320 MiB journal; after one empty-window query over it, a high-water mark of 876 MiB and 714 MiB still resident. The harness run that queried during sealing reached 873 MiB.

Differential, 440 queries and their pages:

| Comparison | Answers that differ | Of which |
| --- | ---: | --- |
| `naive` against stock | 73 | 31 differ in rows, and in every one each missing row belongs to a clock-ahead node (29 of them also differ in later pages); 42 differ in gaps, and they are exactly the 42 queries on gap-only nodes, each of which lost every gap; 0 differ in any other field |
| `sound` against stock | 0 | pages included |

The 440 queries took 11.3 s on the stock server, 7.8 s on `naive` and 8.4 s on `sound`.

## Findings

- **Narrow windows do not threaten the gate at full retention.** The fixed cost is about 0.08 ms per Segment on the stock server (26.7 ms for 319), and about two thirds of it is listing the directory and parsing 319 manifests, which the disposition still does; skipping the gaps table and the footers saves the rest (19.1 ms). At 64 MiB Segments each footer is larger, but the shape of the cost is the same. Extrapolated, 320 Segments cost tens of milliseconds, not seconds.
- **Wide windows grow with the rows retained, and skipping cannot help.** `limit 50` over the whole window took 579 ms over 319 Segments of 1 MiB because every row group in the window is materialised before the heap decides. A 64 MiB Segment holds about 64 times the rows, so the same shape over 20 GiB of retention extrapolates to tens of seconds, far beyond the 2 s gate. Only stopping the scan at the heap's threshold (ledger L-04) addresses it, and the registered protocol does not contain this shape.
- **The traps are real, and the sound rule avoids them.** The receive-bound rule dropped rows from clock-ahead nodes in 31 queries; the node-absence rule dropped every gap of the gap-only nodes in 42 queries. The rule that uses the newest time per node for rows, and the receive bounds for gaps only, changed no answer and no page in 440 queries.
- **The disposition saves between a third and two thirds of the narrow-window cost at 319 Segments** (fleet metric 34.0 to 12.7 ms, rate 36.2 to 16.4 ms, last-10-s logs 28.8 to 15.2 ms), and nothing on the wide-window shape. Against a 2 s gate it is not material. It is worth having because it is the first place the pruning invariant is written down, and because L-03 and L-04 need the same rule per tail entry and per row group.
- **Query time and memory are proportional to the unsealed tail.** One query that could match nothing took 4.64 s and about 858 MiB over a 320 MiB tail, about 3.2 ms and 2.7 MiB per MiB, and the memory stays resident. This is the largest cost found by the audit: with a sealer that falls behind, or with `journal_bytes` at its 4 GiB allowance, a single query can take minutes and gigabytes. It raises the priority of a selectable tail (ledger L-03) above every other mechanism.
- **The second stock pass agrees with the first within 10 %** on every narrow shape; the wide-window shape varied by 14 %, which bounds what the cache order contributed.

## Limits

- Segments of 1 MiB hold one row group per table; a 64 MiB Segment holds about fourteen, so per-Segment costs at the real size are larger by roughly the footer size and the row groups a window touches. The per-Segment slope here is a lower bound.
- One host, warm page cache, a synthetic workload, 10 repetitions per point.
- The hostile state exercises one skew (5 s ahead) and one kind of gap-only node; a node whose clock runs behind the server's is not a trap for lower-bound skipping and was not generated.
- The first attempt at this run stalled because the generator produced 319 files for 320 MiB and the harness waited for a 320th Segment; the first differential attempt ran against a state the old generator had produced, without skew or gap-only nodes, and found no mismatch in either mode. Both attempts are in the logs beside the data; neither result is used above.

## Reproduce

[retscale.py](data/retention-scale/retscale.py.txt) takes an output root and the Segment counts, or `diff` for the differential alone; it expects the stock and prototype servers under a `bin/` directory and the generator built from the sealer-study harness with the skew workload. The prototype's change to `query.rs` is the [diff](data/retention-scale/query-disposition-prototype.diff.txt). Results: [retention-scale.json](data/retention-scale/retention-scale.json) (every point, both passes, memory, the full differential with each mismatch classified) and [differential-queries.json](data/retention-scale/differential-queries.json).
