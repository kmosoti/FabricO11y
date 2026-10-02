# Real-corpus query run, run 01

Status: **Exploratory.** This is the fifth experiment of the [research ledger](../../research/ledger.md) (entry L-21). No protocol was registered before it ran, so it is not **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense and decides no gate. It ran on 2026-10-02 with the stock server built from the code at `58b694d` and the L-04 walk prototype (the L-05 build with its budget off), on CPUs 0 and 1.

The question: every query figure in the ledger comes from synthetic lines. The hypothesis under test was that those figures transfer to real log text within a factor of two, shape by shape, for the stock server and for the walk prototype; it is false if any shape differs by more.

## Method

**States.** Two pairs, real text against synthetic, each the same 64 MiB of journal:

| State | Lines | Entries or Segments | From |
| --- | --- | --- | --- |
| real-text tail | 15,994 real lines drawn at random ([storage layout run 01](storage-layout-run-01.md)), 130 bytes mean | 149,585 entries in one unsealed `batches.faj` | the sealer-study generator with `SEALBENCH_CORPUS` |
| synthetic tail | two 512-byte lines per node per second, half repeated bytes and half random | 55,000 entries | [tail-index run 01](tail-index-run-01.md) |
| real-text Segments | the same lines | 64 Segments of 1 MiB, 300,020 lines, 320,320 points | storage layout run 01 |
| synthetic Segments | the soak lines | 64 Segments of 1 MiB, 110,630 lines, 118,400 points | [threshold run 01](topk-run-01.md) |

**Modes.** Stock; the walk (tail key index plus the threshold walk on the tails, the walk alone on the Segments).

**Shapes.** Fourteen, eight repetitions each over one keep-alive connection, the median reported: logs `limit 50`, `1,000` and `10,000` over the whole window; one node `limit 50`; one metric `limit 100`; four text searches over the whole window at four selectivities (on real text `INFO` in 35 % of lines, `ERROR` 1.0 %, `Exception` 0.1 %, `zq9` none; on synthetic text the tokens `RRRR`, `/fUL`, `RklbD`, `zq9`, of which only the first and last behaved as intended); a text search over the last 60 s with no match; a rate; logs of the last 10 s; one node over the last 60 s; an empty window in the past. Then 200 random queries with every page followed up to three hops, compared field by field between the two modes ([harness](data/real-corpus-query/l21.py.txt), [results](data/real-corpus-query/l21.json), [log](data/real-corpus-query/run.log.txt)).

## Results

Median milliseconds, real text then synthetic, with the ratio; in brackets the sources the walk read out of the sources it had.

**Unsealed tails**

| Shape | stock real | stock synthetic | ratio | walk real | walk synthetic | ratio |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| logs `limit 50` | 701 | 303 | 2.3 | 38 [26 of 149,585] | 14 [26 of 55,000] | 2.7 |
| logs `limit 1,000` | 723 | 284 | 2.6 | 53 [502] | 34 [502] | 1.6 |
| logs `limit 10,000` | 820 | 380 | 2.2 | 171 [5,002] | 196 [5,002] | 0.9 |
| one node `limit 50` | 690 | 243 | 2.8 | 16 [26 of 1,496] | 7 [26 of 550] | 2.2 |
| one metric `limit 100` | 745 | 238 | 3.1 | 21 [102 of 10,000] | 11 [102 of 3,700] | 1.9 |
| text, 35 % of lines | 656 | 259 | 2.5 | 42 [124] | 14 [102] | 3.0 |
| text, 1 % | 706 | 259 | 2.7 | 82 [4,412] | 545 [all] | 0.2 |
| text, 0.1 % | 703 | 248 | 2.8 | 461 [50,556] | 492 [all] | 0.9 |
| text, no match | 721 | 254 | 2.8 | 1,246 [all] | 459 [50,346] | 2.7 |
| text, no match, last 60 s | 749 | 278 | 2.7 | 62 [5,985] | 57 [5,900] | 1.1 |
| rate | 709 | 251 | 2.8 | 289 [10,000] | 110 [3,700] | 2.6 |
| logs, last 10 s | 680 | 278 | 2.5 | 19 [52 of 985] | 8 [52 of 1,000] | 2.4 |
| one node, last 60 s | 685 | 289 | 2.4 | 18 [51 of 60] | 6 [50 of 59] | 2.8 |
| empty window | 619 | 256 | 2.4 | 16 | 5 | 3.1 |

First query (stock decode of the tail; the walk's index build): stock 874 against 332 ms; walk 504 against 181 ms. The 200-query random set with pages: stock 299 against 108 s; walk 15.6 against 9.7 s. High-water mark after the random set: stock 278 against 219 MiB; walk 76 against 57 MiB.

**64 Segments of 1 MiB**

| Shape | stock real | stock synthetic | ratio | walk real | walk synthetic | ratio |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| logs `limit 50` | 130 | 99 | 1.3 | 10 [1 of 64] | 11 [1 of 64] | 1.0 |
| logs `limit 1,000` | 146 | 109 | 1.3 | 16 [1] | 19 [1] | 0.8 |
| logs `limit 10,000` | 216 | 209 | 1.0 | 103 [3] | 136 [6] | 0.8 |
| one node `limit 50` | 119 | 104 | 1.1 | 11 [2] | 13 [4] | 0.9 |
| one metric `limit 100` | 131 | 70 | 1.9 | 11 [1 of 64] | 10 [2 of 45] | 1.1 |
| text, 35 % | 126 | 115 | 1.1 | 10 [1] | 9 [1] | 1.1 |
| text, 1 % | 134 | 100 | 1.3 | 15 [2] | 91 [64] | 0.2 |
| text, 0.1 % | 120 | 91 | 1.3 | 50 [22] | 117 [64] | 0.4 |
| text, no match | 145 | 95 | 1.5 | 147 [64] | 93 [59] | 1.6 |
| rate | 122 | 58 | 2.1 | 133 [64] | 53 [45] | 2.5 |
| logs, last 10 s | 12 | 8 | 1.5 | 13 | 9 | 1.5 |
| empty window | 9 | 9 | 1.0 | 9 | 6 | 1.6 |

Random set: stock 21.9 against 16.1 s; walk 10.3 against 9.0 s. **Mismatches: 0 of 800 random queries and their pages, on the four states.**

## Findings

- **The hypothesis is false on the tail and true per entry.** Every stock shape over the real-text tail costs 2.2 to 3.1 times its synthetic figure, because the same 64 MiB holds 2.7 times as many entries (149,585 against 55,000; real lines are 130 bytes, synthetic 512) and the stock path pays per entry: about 4.7 µs per entry on both tails, whatever the query. The ledger's figure of 3.2 ms per MiB of tail was a figure for 512-byte lines; the right unit is the entry, and real text has 2.7 entries per synthetic one.
- **On Segments the figures transfer within the bound, except where real text means more points.** Logs shapes are within 1.0 to 1.5 times; the metric and rate shapes are 1.9 to 2.5 times because the real-text state holds 2.7 times the points (the generator emits the same points per node per second, and more seconds fit in 64 MiB of short lines) and all 64 Segments carry metrics against 45.
- **The walk transfers, and text selectivity decides its gain.** Shapes that stop read the same number of sources on real as on synthetic text (26 entries for `limit 50`, one row group of 64). A text search's cost is the number of sources read before the heap fills: 124 entries at 35 % selectivity, 4,412 at 1 %, 50,556 at 0.1 %, everything at none. On real text the walk takes the 1 % search from 706 to 82 ms and the 0.1 % search from 703 to 461 ms; the no-match search stays the shape no walk can help.
- **The walk's per-entry overhead is the real-text cost to fix.** When nothing stops it, the walk decodes 149,585 entries one at a time in key order at about 8.3 µs each, against the stock path's 4.8 µs in file order: 1,246 against 721 ms. The index build pays 3.4 µs per entry once (504 ms on real text). Both scale with entries per byte, which real text raises; the file-order fallback named in run L-04, and a cheaper per-entry extract, are worth more on real text than the synthetic figures suggested.
- **Memory follows entries too, in the other direction.** The stock tail decode reached 278 MiB on real text against 219 synthetic; the walk 76 against 57.
- **Answers are unchanged on real text**: 0 mismatches in 800 random queries with pages, including text searches over real bodies.

## Limits

- Real lines were drawn at random from eight 2,000-line samples; a real stream's bursts and repeats are absent.
- Two of the four synthetic text tokens matched nothing or everything, so the selectivity ladder is a real-text result only.
- The real-text states carry 2.7 times the records of the synthetic ones in the same bytes; the comparison is by bytes of journal, as the retention and journal caps are, not by records.
- One host, warm page cache, eight repetitions per shape.

## Reproduce

[l21.py](data/real-corpus-query/l21.py.txt) takes an output root; it expects the stock and L-05 servers under `bin/stock` and `bin/budget`, the real-text tail generated with `SEALBENCH_CORPUS` set to the corpus file, and the four states named above.
