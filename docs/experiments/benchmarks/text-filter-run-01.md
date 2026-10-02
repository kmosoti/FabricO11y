# Text filter run 01: the product's seal-time trigram filters

Status: **Exploratory.** Measured on 2026-10-02 for [ADR-0024](../../decisions/ADR-0024-answer-history-queries-by-a-walk-over-source-bounds.md), part 2, on the product tree. No protocol was registered; nothing here is **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense.

## Setup

The real-text journals of [optimality run 01](optimality-run-01.md) (64 files of 1 MiB in stream order) and of [storage layout run 01](storage-layout-run-01.md) (64 files drawn at random from the corpus) sealed again by the product server, so that every Segment carries `text_filter.bin` ([reseal.py](data/text-filter/reseal.py.txt)); sealing took 2.6 and 3.0 s for 64 files. One release build, `query_plan=scan` against `query_plan=walk`, CPUs 0 and 1. Twelve `contains` tokens of graded frequency at `limit 100` over the whole window, eight repetitions, the median, in ms; a 200-query random differential with pages per state, a third of the logs queries carrying one of the tokens ([product_filter.json](data/text-filter/product_filter.json), [product_filter.py](data/text-filter/product_filter.py.txt)).

## Results

| Token | lines of 16,000 | stream: scan | stream: walk | random: scan | random: walk |
| --- | ---: | ---: | ---: | ---: | ---: |
| `INFO` | 5,629 | 97 | 7.2 | 110 | 6.6 |
| `WARN` | 2,206 | 92 | 8.1 | 108 | 6.4 |
| `blk_` | 2,005 | 95 | 6.7 | 111 | 6.6 |
| `jk2_init` | 848 | 97 | 6.9 | 111 | 6.5 |
| `PacketResponder` | 603 | 96 | 7.1 | 107 | 6.8 |
| `ntpd` | 571 | 94 | 6.9 | 113 | 6.6 |
| `Failed password` | 520 | 94 | 7.0 | 112 | 6.8 |
| `ERROR` | 164 | 92 | 7.0 | 114 | 8.6 |
| `session opened` | 143 | 93 | 6.7 | 118 | 10.1 |
| `Exception` | 15 | 97 | 26.2 | 116 | 38.7 |
| `zq9` | 0 | 95 | 4.9 | 129 | 4.8 |
| `no such token anywhere` | 0 | 95 | 4.5 | 131 | 4.5 |
| 200 random queries with pages (s) | | 16.1 | 6.76 | 17.05 | 6.8 |

Differential: 0 mismatches in 400 random queries with pages.

Bytes:

| State | logs table | filters | filters per row | share of the logs table |
| --- | ---: | ---: | ---: | ---: |
| stream order, 300,140 lines | 3.71 MiB | 0.50 MiB | 1.7 B | 13 % |
| random draw, 300,020 lines | 6.30 MiB | 1.00 MiB | 3.5 B | 16 % |

## Verdict

The no-match search, the one shape neither the scan nor the walk could stop, falls from 95 to 131 ms to 4.5 to 4.9 ms on both orders (19 to 29×): every filter rejects an absent needle and no row group is opened. The rarest present token (`Exception`, 15 lines in 16,000) falls from 97 to 116 ms to 26 to 39 ms, by the filter's skips on top of the walk's early stop. The other present tokens cost what the walk alone made them cost (6.4 to 10.1 ms), as optimality run 01 predicted. The filters cost 13 to 16 % of the logs table at these 4,690-line groups (1.7 bytes per row on stream order, 3.5 on the random draw, whose groups hold more distinct trigrams), sized at about eight bits per distinct trigram; the first query after start, which reads and verifies every filter, took 7 ms.

## Limits

- One row group per Segment at this Segment size; at the product's 64 MiB Segments a group holds 8,192 lines and a filter's size per row falls as distinct trigrams saturate, which this run does not measure.
- The scan plan does not use the filters; the gain is the walk's.
- The filters' correctness evidence is the history test `text_filters_skip_only_groups_without_the_needle_and_fall_back_when_corrupt` and the equivalence tests of part 1, which now run over filtered Segments, and this differential.
