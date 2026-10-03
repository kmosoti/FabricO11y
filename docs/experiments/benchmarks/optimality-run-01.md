# Optimality run 01: stream order, block locality, one copy against the projections, the row-group text filter

Status: **Exploratory.** Nine measurements made on 2026-10-02 for the [optimality bounds](../../research/optimality-bounds.md): hypothesis B3 of the [suite](../../research/hypotheses.md) (a real stream against the random draw), the block locality of tokens in real streams, the text-search half of hypothesis D3 (one canonical copy against the Parquet projections), the walk's no-stop overhead (A4), a trigram filter per row group (L-25, hypothesis C5) the whole-tail decode of a canonical block tail (A3, the fixed-cost half) a per-process cache of Segment metadata (the empty-window gap), the canonical block tail through the server (A3) and the node-presence set settled from the data (L-07). No protocol was registered; nothing here is **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense. Scripts: [locality.py](data/optimality/locality.py.txt); the `fobscan` subcommand of the research generator ([tools/research](../../../tools/research/README.md)).

## B3: a real stream against the random draw

Eight Loghub 2,000-line samples, each in its own order and shuffled, compressed in 1,000-line blocks with Zstd 3 and 19; bytes per line ([locality-1000.json](data/optimality/locality-1000.json)):

| Sample | Zstd 3, stream order | shuffled | gain | Zstd 19, stream | shuffled |
| --- | ---: | ---: | ---: | ---: | ---: |
| Apache | 5.4 | 8.7 | 1.60 | 3.7 | 5.9 |
| HDFS | 29.2 | 31.6 | 1.08 | 21.9 | 23.3 |
| Hadoop | 9.2 | 14.5 | 1.57 | 7.1 | 11.0 |
| Linux | 7.5 | 12.7 | 1.68 | 5.9 | 9.9 |
| OpenSSH | 8.3 | 12.9 | 1.56 | 5.8 | 9.1 |
| Spark | 7.2 | 12.0 | 1.66 | 5.3 | 7.6 |
| Thunderbird | 15.7 | 20.4 | 1.30 | 11.8 | 16.0 |
| Zookeeper | 12.6 | 15.3 | 1.21 | 8.8 | 10.5 |
| lines-weighted | 11.9 | 16.0 | 1.34 | 8.8 | 11.7 |

**Verdict.** H0 (a gain under 1.1×) is rejected on seven of eight samples. H1 as stated (at least 1.5× on at least six) is not met: five reach it, the weighted gain is 1.34×. Every storage figure measured on the random draw is pessimistic by a fifth to two thirds, depending on the source.

## Block locality of tokens

For each token, the share of 100-line blocks that hold at least one matching line, in stream order and shuffled ([locality-100.json](data/optimality/locality-100.json)); only tokens below 10 % of lines are informative:

| Sample, token | lines | blocks touched, stream | shuffled |
| --- | ---: | ---: | ---: |
| Hadoop, `ERROR` | 7.6 % | 60 % | 100 % |
| Hadoop, `Exception` | 0.4 % | 10 % | 30 % |
| Hadoop, `timeout` | 0.3 % | 5 % | 25 % |
| Linux, `kernel` | 3.9 % | 5 % | 100 % |
| OpenSSH, `closed` | 1.8 % | 45 % | 90 % |
| Thunderbird, `session` | 2.2 % | 25 % | 95 % |
| Zookeeper, `ERROR` | 0.7 % | 10 % | 55 % |
| Zookeeper, `session` | 11.7 % | 35 % | 100 % |

**Verdict.** Real streams cluster: a token at a few percent of lines touches 5 to 60 % of blocks in stream order and nearly all of them shuffled. A block-level text filter (an n-gram set or bloom per row group) would skip half to nineteen twentieths of the blocks for such tokens on real streams and nothing on the random draw. The samples are 2,000 lines, so the block here is 100 lines; nothing is known about locality at 8,192-row groups from this data.

## One copy against the projections (D3, the text-search half)

The 64 real-text journal files of [storage layout run 01](storage-layout-run-01.md) (620,340 records: 300,020 lines, 320,320 points) converted to one FOB1 block per file and compressed with Zstd 3, against the 64 Parquet Segments the stock server sealed from the same journals. A whole-history substring search over bodies, four tokens, eight passes, the median ([fobscan.jsonl](data/optimality/fobscan.jsonl)); CPUs 2 and 3:

| Token | hits | FOB1: decompress, view-decode, search | of which decode alone | Parquet `logs.parquet` scan |
| --- | ---: | ---: | ---: | ---: |
| `INFO` | 105,975 | 163 ms | 133 | 117 ms |
| `ERROR` | 3,054 | 148 | 152 | 124 |
| `Exception` | 282 | 141 | 139 | 137 |
| `zq9` | 0 | 149 | 148 | 123 |

Bytes read: FOB1 5.3 MiB for every record of both kinds (point values set to zero in this conversion, which removes the 3 MiB of random gauge values the generator produces and nothing else; with them the blocks are 8.6 MiB, [encoding run 01](observation-encoding-run-01.md)); Parquet 6.3 MiB for the lines alone, beside the 16.1 MiB custody table and the 3.3 MiB metrics table the Segment also holds.

**Verdict.** One canonical copy answers a full text search within 1.2 to 1.4 times the projection's time, with identical hits, from a third of the bytes of the three tables it would replace (8.6 against 25.7 MiB with the random point values, 5.3 against 25.7 without them). The cost is almost entirely the fixed decompress-and-decode pass (133 to 152 ms); the search itself is about 10 ms. H0 of D3 ("some registered shape over 2× slower, or bytes over 70 %") is rejected for the text-search shape; the other thirteen shapes of D3 need block-level key bounds in a manifest and were not run.

## A4: the walk's no-stop overhead

The walk prototype with a 64-frame decode cache in place of its single cached frame, and with an optional file-order pass, against stock on the real-text 64 MiB tail (149,585 entries); eight repetitions, the median; a 100-query random differential with pages ([a4.json](data/optimality/a4.json), [a4.py](data/optimality/a4.py.txt)):

| Shape | stock | walk, key order, 64-frame cache | walk, file order for text filters |
| --- | ---: | ---: | ---: |
| logs `limit 50` | 607 ms | 40 [26 of 149,585] | 44 |
| text, 35 % of lines | 614 | 40 [124] | 575 [all] |
| text, 1 % | 596 | 56 [4,412] | 584 [all] |
| text, 0.1 % | 584 | 227 [50,556] | 568 [all] |
| text, no match | 570 | 556 [all] | 579 [all] |
| rate | 609 | 217 | 231 |
| random set, 100 queries with pages | 134 s | 6.8 s | 9.8 s |
| mismatches | | 0 | 0 |

**Verdict.** The hypothesis named the order; the measurement names the cache. In key order consecutive entries interleave among a few frames, and the single-frame cache of runs L-04 and L-21 re-decoded a frame for nearly every entry (1,246 ms for the no-match search in run L-21); a 64-frame cache brings that to 556 ms, under the stock scan's 570, and halves the 0.1 % search (461 to 227 ms). File order removes the re-decoding too but forgoes the early stop, so a 35 % search that stops after 124 entries in key order reads everything in file order (575 ms). H0 of A4 (the overhead is per-entry work whatever the order) is rejected; H1 as stated (a file-order fallback brings the no-match shape to stock) is not the remedy; key order with a frame cache is, and it is the prototype's default from this run on.

## L-25: a trigram filter per row group

The walk prototype with a 2^16-bit bloom filter per row group over the byte trigrams of its bodies (two positions per trigram, 8 KiB per group, built on the first query by a pass over the group and held in memory; `FABRIC_PROTO_TEXT_FILTER=on`), against the walk alone and against stock, on two real-text Segment states of 64 Segments each (one row group of about 4,690 lines per Segment; 300,140 and 300,020 log rows): one sealed from the corpus in **stream order** (each line after its own predecessor, the 1 MiB journal of run L-26's generator option) and one from the **random draw** of run L-21. Twelve `contains` tokens of graded frequency, `limit 100`, the whole window; eight repetitions, the median; a 200-query random differential with pages per state, a third of the logs queries carrying one of the tokens ([l25.json](data/optimality/l25.json), [l25.py](data/optimality/l25.py.txt)):

| Token | Lines of 16,000 | Stream order: stock | walk [groups read of 64] | walk + filter [read of kept] | Random draw: stock | walk [read of 64] | walk + filter [read of kept] |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `INFO` | 5,629 | 112 ms | 10 [1] | 10 [1 of 64] | 130 ms | 10 [1] | 13 [1 of 64] |
| `WARN` | 2,206 | 102 ms | 12 [2] | 11 [2 of 53] | 135 ms | 10 [1] | 14 [1 of 64] |
| `blk_` | 2,005 | 100 ms | 12 [1] | 11 [1 of 32] | 121 ms | 10 [1] | 13 [1 of 64] |
| `jk2_init` | 848 | 105 ms | 12 [1] | 9 [1 of 24] | 138 ms | 10 [1] | 11 [1 of 64] |
| `PacketResponder` | 603 | 107 ms | 13 [1] | 9 [1 of 28] | 124 ms | 11 [1] | 10 [1 of 64] |
| `ntpd` | 571 | 104 ms | 15 [3] | 9 [1 of 27] | 128 ms | 12 [1] | 10 [1 of 64] |
| `Failed password` | 520 | 110 ms | 15 [2] | 9 [1 of 25] | 130 ms | 12 [1] | 12 [1 of 64] |
| `ERROR` | 164 | 116 ms | 14 [2] | 10 [1 of 46] | 144 ms | 15 [2] | 12 [2 of 64] |
| `session opened` | 143 | 117 ms | 12 [2] | 10 [1 of 44] | 123 ms | 15 [3] | 16 [3 of 64] |
| `Exception` | 15 | 98 ms | 49 [23] | 33 [15 of 39] | 152 ms | 46 [22] | 52 [22 of 63] |
| `zq9` | 0 | 107 ms | 118 [64] | 8 [0 of 0] | 143 ms | 123 [64] | 10 [0 of 0] |
| `no such token anywhere` | 0 | 103 ms | 114 [64] | 7 [0 of 0] | 146 ms | 126 [64] | 9 [0 of 0] |

Building the 64 filters cost 210 ms on the stream state and 251 ms on the random one (0.7 to 0.8 µs per row, 512 KiB held: 1.1 to 1.8 bytes per row, 13 % of the compressed logs table at this row-group size; at the product's 8,192-row groups it is 1 byte per row). The 200-query differentials: 0 mismatches for the walk and 0 for the filter on both states. Resident memory: 33 to 34 MiB for walk and filter against 21 to 44 for stock.

**Verdict.** Three findings, one of them against the derivation that put L-25 on the ledger.

1. **The no-match search reaches the block floor whatever the order.** Every group's filter rejects an absent token, so the walk reads no group at all: 114 to 126 ms become 7 to 10 ms (the cost is now the manifests and bounds), 12 to 15× on both states. This is the shape the walk could not stop and the one that cost 556 ms on the 64 MiB tail (A4); it is the shape an index exists for.
2. **For a present token the filter's gain is in stream order and is set by the row group's grain, not the token's.** In stream order the filter drops 11 to 40 of 64 groups for tokens at 0.9 to 14 % of lines and 25 for the one at 0.1 % (`Exception`: 23 groups read become 15, 49 ms become 33). On the random draw it drops nothing for any present token (1 group for `Exception`), as the locality measurement predicted: a token drawn at random into 4,690-line groups lands in every group. The groups here are 47 times the 100-line blocks of the locality measurement, so the fractions skipped (17 to 62 %) are below the ones that measurement promised for 100-line blocks (40 to 95 %); hypothesis D4 (finer row groups) and this filter are the same lever seen twice.
3. **The walk already made the present-token shapes cheap; the filter is for what the walk cannot stop.** With `limit 100` the walk stops after 1 to 3 groups for every token above 0.1 % of lines (10 to 15 ms against stock's 100 to 150, which reads all 64 groups), so the filter changes those shapes by a millisecond or two. Its value is the two shapes whose answer is small but whose candidates are everywhere: the rare token and the absent one.

**Against prior art.** An inverted index over the same 300,140 lines would hold a posting per distinct token per line; the filter holds 1 to 2 bytes per row, is built at 0.8 µs per row with no tokenizer, and reaches the index's floor (read only blocks that can hold the token) exactly where real streams are local. Where they are not (the random draw), neither would help a present token at this grain, and only finer blocks would. The bloom's false-positive rate at 2^16 bits and two positions is about 7 % per group for a one-trigram needle at 10,000 distinct trigrams per group and falls geometrically with the needle's trigrams, which the 64-of-64 rejections of both absent needles show. The filter is an index beyond row-group statistics and so a product-contract question (ledger [L-25](../../research/ledger.md#l-25-a-text-filter-per-row-group)); this run says what it buys, not whether the contract should admit it.

## A3, the fixed-cost half: the 64 MiB real tail as canonical blocks

The real-text 64 MiB tail of run L-21 (149,585 journal entries: 619,570 records, 299,570 lines) converted by `fobscan` to ten FOB1 blocks of at most 65,536 records (point values dropped, as in the D3 half) and compressed with Zstd 3; eight passes, the median, CPUs 2 and 3 ([fobscan-tail.jsonl](data/optimality/fobscan-tail.jsonl)). The stock figures are the A4 table's full-tail shapes on the same journal, through the server on CPUs 0 and 1:

| Pass over the whole tail | Bytes | Time |
| --- | ---: | ---: |
| stock server, logs `limit 50` (decodes every entry) | 64 MiB journal | 607 ms |
| stock server, text with no match | 64 MiB | 570 ms |
| FOB1 blocks: decompress and view-decode every record (keys only) | 4.1 MiB (46.4 MB raw) | 140 to 146 ms |
| FOB1 blocks: the same plus a substring search over every body (`INFO`, `Exception`, `zq9`) | 4.1 MiB | 140 to 153 ms |

**Verdict.** The decode of the whole tail is the fixed cost that every shape the walk cannot stop pays, and as canonical blocks it is 4.0 to 4.3× below the stock OTLP decode (and 3.7 to 4.0× below the walk's 556 ms no-match search with the frame cache). The [cost model](../../research/observation-model.md) predicts 82 ms for the view decode (619,570 records at 72 ns, 46.4 MB at 0.8 ns per byte) and about 45 ms for Zstd at 1 GB/s, 127 ms against 140 to 146 measured, within 15 %. The compressed tail is 16× smaller than the journal that holds the same records.

What this half does not show, and why A3 stays open: these are decode passes in a process, not server answers; there is no key filter, heap, HTTP or JSON here, and the stock side carries all of them (the walk's `limit 50` answer on the same tail is 40 ms, so the answer's own cost is small against the decode's). A block tail would also have to be written by the server as it receives, which is the ADR-0023 wire question, and the budget of the A3 statistic (one fifth of stock on three shapes with a clean differential) needs the server path. The figure says the fifth is within reach: 146 of 607 is 0.24, before the server's early stop and the frame cache that A4 added to the walk apply to blocks too.

## The empty-window gap: Segment metadata read on every query

The bounds page put the empty-window shape at Ω(S) against a floor of Ω(log S), because every query lists the Segments, reads each manifest and, in the walk, opens each Parquet footer for its row-group bounds. The walk prototype with `FABRIC_PROTO_META_CACHE=on` keeps both per process, keyed by the Segment's path; a named Segment never changes and the directory is still listed on every query, so retention is seen as before. The fourteen L-21 shapes on three 64-Segment states, eight repetitions, the median, and a 200-query random differential with pages against stock per state ([metacache.json](data/optimality/metacache.json), [metacache.py](data/optimality/metacache.py.txt)); the shapes most sensitive to fixed cost, in ms:

| State | Shape | stock | walk | walk + cache |
| --- | --- | ---: | ---: | ---: |
| real text, stream order | `empty_past` | 7.3 | 6.8 | 5.9 |
| real text, stream order | `logs_last_10s` | 10.4 | 9.3 | 8.4 |
| real text, stream order | `host_last_60s` | 11.8 | 11.7 | 10.2 |
| real text, stream order | `logs_limit50_full` | 101.4 | 9.0 | 9.6 |
| real text, stream order | `metric_limit100_full` | 111.4 | 11.1 | 7.5 |
| real text, stream order | `text_none_full` | 106.7 | 100.9 | 103.4 |
| real text, stream order | random set, 200 queries with pages (s) | 18.92 | 9.88 | 9.57 |
| real text, random draw | `empty_past` | 10.0 | 7.8 | 4.4 |
| real text, random draw | `logs_last_10s` | 10.7 | 11.9 | 6.7 |
| real text, random draw | `host_last_60s` | 14.1 | 14.1 | 9.9 |
| real text, random draw | `logs_limit50_full` | 141.1 | 9.8 | 9.6 |
| real text, random draw | `metric_limit100_full` | 109.5 | 12.9 | 8.7 |
| real text, random draw | `text_none_full` | 144.4 | 123.2 | 121.4 |
| real text, random draw | random set, 200 queries with pages (s) | 19.41 | 10.93 | 8.54 |
| synthetic, every Segment overlapping | `empty_past` | 6.6 | 6.8 | 4.7 |
| synthetic, every Segment overlapping | `logs_last_10s` | 6.6 | 8.5 | 4.2 |
| synthetic, every Segment overlapping | `host_last_60s` | 92.4 | 94.9 | 95.3 |
| synthetic, every Segment overlapping | `logs_limit50_full` | 93.0 | 64.3 | 69.7 |
| synthetic, every Segment overlapping | `metric_limit100_full` | 69.5 | 67.1 | 67.1 |
| synthetic, every Segment overlapping | `text_none_full` | 98.5 | 102.6 | 125.0 |
| synthetic, every Segment overlapping | random set, 200 queries with pages (s) | 31.83 | 32.87 | 32.98 |

Differentials: 0 mismatches for the walk and 0 for the walk with the cache on all three states (600 queries).

**Verdict.** The gap was smaller than the bounds page assumed, and most of it was not the metadata. The cache takes the empty-window shape from 6.6 to 10.0 ms (stock) to 4.4 to 5.9 ms and the short-window shapes down by 1 to 4 ms on the real-text states, but what remains is about 4 ms whatever the shape: the cheapest shape that reads data (`logs_last_10s` on the overlapping state) costs 4.2 ms with the cache. So the empty-window answer now sits at the per-request floor of this server (the journal listing, TLS, the JSON answer), not at Ω(S); a sorted table of Segment bounds would save what binary search saves over a 64-entry in-memory scan, which is nothing measurable at this S. At the product's 320 Segments of 64 MiB the uncached reads would be five times larger (about 25 to 40 ms by the 0.08 ms per manifest and the footer reads), which is where the cache matters. On the overlapping state the walk does not help any wide shape, as its floor says (every Segment can hold the first keys), and the spread between repeated configurations there (up to 25 % on the text shapes) is the noise of this container, not an effect of the cache, which removes reads and adds none.

## A3 through the server: the real tail answered from canonical blocks

The walk prototype with `FABRIC_PROTO_TAIL_BLOCKS=on`: every tail frame's log records converted once to FOB1, n consecutive frames of a file per Zstd-compressed block (`FABRIC_PROTO_TAIL_BLOCK_FRAMES=n`), each block a source of the walk bounded by its selected entries' keys, read through one reused decompressor and `decode_view`, with a row materialised only after the window, node, text and threshold checks on borrowed fields. The conversion runs on the first query and is timed apart (it stands in for a server that writes blocks on receipt; 0.6 to 2.0 s for the 619,170 records, about 1 to 3 µs per record including the OTLP decode the receipt path does anyway). The real-text 64 MiB tail of run L-21 (149,585 entries in 29,917 frames), the fourteen L-21 shapes, eight repetitions, the median, and a 200-query random differential with pages against stock for every configuration ([a3-sweep.json](data/optimality/a3-sweep.json), [a3-filter.json](data/optimality/a3-filter.json), [a3.py](data/optimality/a3.py.txt)); in ms:

| Configuration | `empty_past` | logs `limit 50` | text, 0.1 % | text, no match | random set (s) | peak RSS (MiB) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| stock | 608 | 628 | 616 | 609 | 268.39 | 279 |
| walk over the OTLP tail with the key index | 13 | 38 | 214 | 580 | 12.37 | 75 |
| blocks of 1 frames | 11 | 48 | 152 | 335 | 14.14 | 152 |
| blocks of 8 frames | 13 | 46 | 104 | 215 | 13.09 | 132 |
| blocks of 32 frames | 11 | 43 | 96 | 184 | 14.15 | 131 |
| blocks of 128 frames | 12 | 47 | 99 | 173 | 13.37 | 131 |
| blocks of 512 frames | 12 | 49 | 85 | 161 | 14.65 | 127 |

| Configuration (second run) | `empty_past` | logs `limit 50` | text, 0.1 % | text, no match | random set (s) | peak RSS (MiB) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| stock | 608 | 626 | 643 | 614 | 265.65 | 281 |
| walk | 12 | 39 | 219 | 567 | 12.47 | 79 |
| blocks of 32 frames | 16 | 48 | 98 | 193 | 13.48 | 138 |
| blocks of 128 frames | 13 | 50 | 95 | 173 | 13.67 | 132 |
| blocks of 32 frames + trigram filter | 12 | 66 | 68 | 56 | 14.13 | 182 |
| blocks of 128 frames + trigram filter | 13 | 55 | 82 | 48 | 14.19 | 132 |

Differentials: 0 mismatches for every configuration of both runs (2,200 queries with pages against stock).

**Verdict.** Three findings.

1. **The block's grain is the whole effect, and the cost model predicts it.** One block per journal frame (about 21 records) was slower than the OTLP walk in a first build that created a Zstd context per block (853 ms for the no-match search; 28.5 µs per block); with one reused decompressor it is 335 ms, and the sweep falls as the per-block fixed cost is amortised: 215 ms at 8 frames, 184 at 32, 173 at 128, 161 at 512. Fitting cost = blocks × c_block + records × c_rec to the no-match column gives c_block ≈ 6 µs and a floor of about 160 ms, which is the decode pass of the fixed-cost half (140 to 146 ms) plus the walk's per-query bookkeeping. The grain that balances the two terms is c_block / c_rec ≈ 6 µs / 0.13 µs ≈ 50 records per block; above about 2,600 records (128 frames) the remaining gain is under 10 % (173 to 161 ms at 512 frames). The selective shapes pay for coarse blocks in overshoot instead: `limit 10,000` rises from 126 ms at 128 frames to 149 at 512, and the node shape from 15 to 22 ms.
2. **A3 as registered is not met; H0 as worded is rejected.** Under blocks alone the empty-window shape is 0.02 of stock and `limit 50` 0.08, but the no-match search is 0.26 to 0.28 (161 to 173 against 609 to 614 ms), above the one fifth the statistic requires. The block tail is 3.5 to 3.8× faster than stock on the no-match search and 2.2 to 2.5× faster than the walk on the rare token, so H0's wording (within 2× of the OTLP tail) does not hold either. The registered decision rule decides: H0 is not rejected.
3. **With L-25's filter per block, every A3 shape passes.** A trigram bloom built with each block rejects every block for an absent token: the no-match search falls to 48 to 56 ms (0.08 to 0.09 of stock) with no record decoded, and the rare token to 68 to 82 ms. That is two mechanisms together (A3 and C5), not A3 as registered, and it inherits C5's product-contract question; it is recorded as the configuration that reaches every floor of the tail shapes measured here.

**Against prior art.** Loki's chunks and Elasticsearch's stored fields are blocks of compressed raw lines decoded whole; the measurement says the decoding format, not the blocking, set the stock tail's cost (4.7 µs per entry against 0.13 µs per record plus 6 µs per block), and the block's grain is a computable trade-off between a per-block constant and the overshoot of selective shapes, not a tuning knob. ClickHouse's granule (8,192 rows) and Parquet's row group sit on the same curve far to the coarse side; the tail's optimum is far finer because its sources must also serve `limit k` early stops.

## L-07: a node-presence set per row group, settled from the data

A node-presence set lets a node query skip a row group that does not hold the node. Time-sorted rows from N nodes emitting at comparable rates put a given node in a group of g rows with probability 1 − (1 − 1/N)^g ≈ 1 − e^(−g/N), so the fraction of groups a presence set can skip for one node is about e^(−g/N). Measured on the two real-text 64-Segment states merged into one time-sorted table of 300,140 and 300,020 lines from 100 nodes, the size of one 64 MiB real-text journal file ([l07-nodes.json](data/optimality/l07-nodes.json)):

| Rows per group | distinct nodes per group, stream order (min / mean / max) | random draw | predicted e^(−g/N) skippable |
| ---: | --- | --- | ---: |
| 1,024 | 54 / 99.8 / 100 | 100 / 100 / 100 | 0.004 % |
| 4,096 | 100 / 100 / 100 | 100 / 100 / 100 | 0 |
| 8,192 | 100 / 100 / 100 | 100 / 100 / 100 | 0 |

**Verdict.** At this fleet size the presence set would skip nothing at the product's grain, so L-07's node half is worth nothing here; the stream's bursts show only at 1,024 rows. The formula says where that changes: at N = 10,000 nodes a node is absent from 44 % of 8,192-row groups and from 90 % of 1,024-row groups (D4), so the set pays at fleet scale and with finer groups, at N bits per group (1.2 KiB at 10,000 nodes, under 0.2 bytes per row). The time-bounds half of L-07 is unaffected.

## Limits

- The samples are 2,000 lines each; stream order within a sample is real, the mix across samples is not, and the locality block is 100 lines.
- The FOB1 conversion dropped point values; the time comparison stands for lines, the byte comparison is given both ways.
- The Parquet scan materialises every row (`LogRow` with its allocations), as the stock path does; a projection reader that searched the body column without materialising would be faster than 117 ms, and so would a block search that stopped decoding at the body. Both sides have the same headroom.
- The L-25 states hold one row group per Segment; the filter's skip fractions for present tokens are those of 4,690-line blocks and would differ at 8,192 rows (the product's) or at the finer groups of D4. The filter is built per query process and held in memory; a product filter would be written at seal time and read from the manifest, which this run does not cost.
- The block tail is built in memory from the OTLP journal on the first query; a server that wrote blocks on receipt (the ADR-0023 wire decision) would also pay the write and the durability of the block, which no run here costs. The L-07 table assumes comparable per-node rates; a skewed fleet changes the exponent per node.
- One host, warm cache. The stock figures in the A4 table are about 15 % below those of run L-21 on the same state (570 to 614 against 656 to 749 ms), within the variance seen between runs on this container.
