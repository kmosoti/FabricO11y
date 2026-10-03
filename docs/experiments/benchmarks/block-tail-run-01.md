# Block tail run 01: the product's walk over a canonical block tail

Status: **Exploratory.** Measured on 2026-10-03 for [ADR-0024](../../decisions/ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md), part 3, on the product tree. No protocol was registered; nothing here is **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense. It is the server-path run that hypothesis A3 of the [suite](../../research/hypotheses.md) asked for.

## Setup

As [query walk run 01](query-walk-run-01.md): one release build of the product tree, `query_plan=scan` against `query_plan=walk`, the real-text 64 MiB unsealed tail (149,585 entries, 619,170 records), the fourteen L-21 shapes, eight repetitions, the median, in ms, a 200-query random differential with pages, CPUs 0 and 1. The walk now reads every complete tail block (4,096 records, closed at a frame boundary, Zstd-compressed FOB1 with a trigram filter over its log bodies) through `decode_view`, and only the entries after a file's last complete block through their OTLP bytes ([product_blocks.json](data/block-tail/product_blocks.json), [product_walk.py](data/block-tail/product_walk.py.txt)). The 64-Segment state of the same run was sealed before part 2 and holds no text filters; its figures are in the JSON and do not bear on part 3.

## Results

| Shape | scan | walk without blocks (query walk run 01) | walk with the block tail | block tail against scan |
| --- | ---: | ---: | ---: | ---: |
| logs `limit 50` | 631 | 26.0 | 26.2 | 0.042 |
| logs `limit 1,000` | 625 | 34.5 | 33.2 | 0.053 |
| logs `limit 10,000` | 715 | 160.9 | 125.4 | 0.175 |
| one node, `limit 50` | 569 | 14.9 | 16.9 | 0.030 |
| one metric, `limit 100` | 580 | 22.7 | 15.1 | 0.026 |
| text, 35 % of lines | 604 | 28.5 | 27.5 | 0.046 |
| text, 1 % | 614 | 40.5 | 30.8 | 0.050 |
| text, 0.1 % | 601 | 202.3 | 65.7 | 0.109 |
| text, no match | 614 | 549.7 | 27.7 | 0.045 |
| text, no match, last 60 s | 582 | 34.0 | 13.5 | 0.023 |
| rate, whole window | 596 | 213.0 | 175.1 | 0.294 |
| logs, last 10 s | 621 | 13.3 | 11.4 | 0.018 |
| one node, last 60 s | 595 | 13.1 | 17.8 | 0.030 |
| empty window in the past | 633 | 11.2 | 10.7 | 0.017 |
| 200 random queries with pages (s) | 255.08 | 12.92 | 11.42 | |
| peak RSS (MiB) | 279 | 62 | 113 | |

Differential: 0 mismatches in 400 random queries with pages (200 on the tail, 200 on the Segments).

## Verdict

**A3's registered statistic is met through the server.** The empty-window shape answers at 0.017 of the scan, `limit 50` at 0.042 and the no-match text search at 0.045, all under one fifth, with a clean differential. The no-match search, which the walk could not stop (550 ms), now costs 28 ms: every block's filter rejects the needle, so only the index and the few entries outside complete blocks are read. That is the A3 mechanism together with C5's filter per block, as [optimality run 01](optimality-run-01.md) found was needed; blocks alone measured 0.26 to 0.28 there. The rare token falls from 202 to 66 ms, the 10,000-row page from 161 to 125 ms and the metric shape from 23 to 15 ms, because a block decodes at about 0.13 µs per record against 4.7 µs per OTLP entry. The whole-window rate (175 ms) still reads every metric point, which is its floor.

Memory: peak resident 113 MiB against 62 MiB for the walk without blocks and 281 MiB for the scan; the same tail compressed to 4.1 MB in 65,536-record blocks in optimality run 01, and at this run's 4,096-record blocks its size and the share of the rise due to building them on the first query were not measured separately.

## Limits

- One host, warm page cache, one tail; the target profile is not measured.
- The blocks are derived in memory and rebuilt after a restart (the first query after start builds the index and the blocks); their build cost was not separated from the index's in this run.
- Correctness evidence: `a_block_tail_answers_as_the_scan_does` (small blocks over a mostly unsealed history from three Spindles at identical instants, page for page against the scan, with a full drain; an injected entry-boundary defect in block reading fails it), the unit test that a block the codec refuses is not made, part 1's tests, and this differential.
