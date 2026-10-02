# Query walk run 01: the product's walk plan against its scan plan

Status: **Exploratory.** Measured on 2026-10-02 for [ADR-0024](../../decisions/ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md), part 1, on the product tree, not on a research patch. No protocol was registered; nothing here is **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense, and the registered history protocol has not been re-run with the walk.

## Setup

One release build of `fabric-server` from the product tree, started once per plan with `query_plan=scan` (the default) or `query_plan=walk`, pinned to CPUs 0 and 1 of the four-CPU container. Two states from the research loop ([tools/research](../../../tools/research/README.md)): the real-text 64 MiB unsealed tail of [real-corpus query run 01](real-corpus-query-run-01.md) (149,585 entries) and 64 real-text Segments sealed in stream order ([optimality run 01](optimality-run-01.md)). The fourteen shapes of run L-21, eight repetitions, the median, in ms; a 200-query random differential with pages between the two plans per state; peak resident memory over the run ([product_walk.json](data/query-walk/product_walk.json), [product_walk.py](data/query-walk/product_walk.py.txt)).

## Results

| Shape | tail: scan | tail: walk | Segments: scan | Segments: walk |
| --- | ---: | ---: | ---: | ---: |
| logs `limit 50` | 633 | 26 | 102 | 8 |
| logs `limit 1,000` | 635 | 34 | 110 | 15 |
| logs `limit 10,000` | 711 | 161 | 187 | 95 |
| one node, `limit 50` | 615 | 15 | 98 | 10 |
| one metric, `limit 100` | 633 | 23 | 111 | 8 |
| text, 35 % of lines | 610 | 29 | 97 | 8 |
| text, 1 % | 612 | 40 | 98 | 9 |
| text, 0.1 % | 604 | 202 | 110 | 41 |
| text, no match | 638 | 550 | 99 | 103 |
| text, no match, last 60 s | 621 | 34 | 13 | 10 |
| rate, whole window | 640 | 213 | 112 | 106 |
| logs, last 10 s | 650 | 13 | 10 | 7 |
| one node, last 60 s | 614 | 13 | 12 | 9 |
| empty window in the past | 640 | 11 | 8 | 4 |
| 200 random queries with pages (s) | 268.03 | 12.92 | 19.06 | 8.63 |
| peak RSS (MiB) | 281 | 62 | 41 | 40 |
| first query after start (ms) | 1150 | 559 | 96 | 108 |

Differential: 0 mismatches in 400 random queries with pages (every field of every page, page tokens included).

## Verdict

The product walk reproduces the prototype's figures within the container's noise. On the tail the shapes that stop early cost their answer (11 to 41 ms against 610 to 650 ms; the 10,000-row page 161 ms and the 0.1 % text search 202 ms, which stop later, against 711 and 604) and the random set falls from 268 s to 12.9 s, a 21× reduction, while peak memory falls from 281 MiB to 62 MiB, because the scan materialises every row of the tail on every query and the walk holds a 72-byte index entry per journal entry and decodes only what it reaches. The shapes that cannot stop keep the scan's order of cost: the no-match search (550 against 638 ms on the tail, 103 against 100 on Segments) and the whole-window rate on Segments (106 against 112), as the [optimality bounds](../../research/optimality-bounds.md) say; parts 2 and 3 of ADR-0024 are what remove them. The first query after start builds the tail index (559 ms on the tail, one scan's worth).

## Limits

- One host, warm page cache, one state of each kind; the target profile is not measured.
- The default stays `scan`: making `walk` the default is gated in ADR-0024 on the registered history protocol re-run with the walk and on the soak.
- The correctness evidence is the equivalence tests in `crates/fabric-server/tests/history.rs` (seeded queries page for page against the scan plan, a running server sealing between queries, the oracle on the walk's own answers, the corrupt-Segment and sealing-crash cases), whose fixture was strengthened until an injected off-by-one in the stop rule failed it, and this differential.
