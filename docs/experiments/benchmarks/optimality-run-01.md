# Optimality run 01: stream order, block locality, one copy against the projections

Status: **Exploratory.** Three measurements made on 2026-10-02 for the [optimality bounds](../../research/optimality-bounds.md): hypothesis B3 of the [suite](../../research/hypotheses.md) (a real stream against the random draw), the block locality of tokens in real streams, and the text-search half of hypothesis D3 (one canonical copy against the Parquet projections). No protocol was registered; nothing here is **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense. Scripts: [locality.py](data/optimality/locality.py.txt); the `fobscan` subcommand of the research generator ([tools/research](../../../tools/research/README.md)).

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

## Limits

- The samples are 2,000 lines each; stream order within a sample is real, the mix across samples is not, and the locality block is 100 lines.
- The FOB1 conversion dropped point values; the time comparison stands for lines, the byte comparison is given both ways.
- The Parquet scan materialises every row (`LogRow` with its allocations), as the stock path does; a projection reader that searched the body column without materialising would be faster than 117 ms, and so would a block search that stopped decoding at the body. Both sides have the same headroom.
- One host, warm cache.
