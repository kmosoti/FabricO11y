# Storage layout measurements, run 01

Status: **Exploratory.** Measurements of what a Segment is made of, byte by byte, and of what alternative encodings, orders and layouts would change, taken on 2026-10-02 from Segments the stock server at `58b694d` sealed. No server code changed; the alternative layouts were produced by re-encoding the sealed tables with `pyarrow` 2.x and `zstandard`. No protocol was registered, so nothing here is **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense. It feeds the [storage direction](../../research/storage-direction.md) and ledger entries L-06 and L-19 to L-21.

The question: where do a Segment's bytes go, how much of that is the synthetic workload's doing, and which of the levers a storage redesign could pull (encodings, sort order, dropping the duplicate copy, template extraction) would move the number, on synthetic and on real log text?

## Method

**Segment states.** Three states of 64 Segments each, every Segment sealed from one 1 MiB journal file by the stock server (so each table is one or a few row groups of 1,630 to 6,400 rows, smaller than the 8,192-row groups of a 64 MiB Segment):

| State | Workload | Journal in | Segments out |
| --- | --- | --- | --- |
| synthetic steady | the soak generator: 100 nodes, two 512-byte lines per node per second, half repeated bytes and half seeded entropy; 32 gauges of random 63-bit integers every 15 s | 64.2 MiB | 47.3 MiB |
| synthetic adversarial | the same lines, every node's clock and backlog different | 64.2 MiB | 50.2 MiB |
| real text | the steady generator with each log body drawn at random from 15,994 real lines (the Loghub 2,000-line samples of Apache, HDFS, Hadoop, Linux, OpenSSH, Spark, Thunderbird and Zookeeper logs); 85 to 192 bytes per line | 64.1 MiB | 26.0 MiB |

**Composition** ([segstat.py](data/storage-layout/segstat.py.txt)): per table and per column, compressed and uncompressed bytes, values and encodings from the Parquet footers, summed over the 64 Segments.

**Re-encodings** ([reencode.py](data/storage-layout/reencode.py.txt)): each table rewritten with `pyarrow`, Zstd level 3, dictionary on, 8,192-row groups, as a control; then level 9; `DELTA_BINARY_PACKED` on the time, group, sequence and integer value columns; rows re-sorted series-first (node, then name for metrics, then time) instead of time-first; the logs table without its body and the body alone; the batches table without its SHA-256 column and the raw bytes alone. Sizes only; nothing was read back.

**Lines alone** ([corpus.py](data/storage-layout/corpus.py.txt)): the eight real samples, each as one body column in Parquet (Zstd 3), as one Zstd stream (levels 3 and 19), as a crude template plus variables split (numbers, hex, addresses and paths replaced by a placeholder; the template column dictionary-encoded, the variables as one string and as a typed integer list plus a string), compressed line by line without and with a 16 KiB Zstd dictionary trained on the first half and measured on the second.

## Results

**What a Segment is made of** (bytes over 64 Segments; the share of the Segment in brackets):

| Table or column | synthetic steady | synthetic adversarial | real text |
| --- | ---: | ---: | ---: |
| `batches.parquet` | 24.5 MiB (52 %) | 25.7 MiB (51 %) | 16.1 MiB (62 %) |
| of which raw `batch` bytes | 22.6 MiB, 428 B per Batch, 2.8:1 | 23.8 MiB, 2.6:1 | 11.1 MiB, 77 B per Batch, 5.4:1 |
| of which `sha256` | 1.7 MiB (3.6 %), 32 B per Batch, uncompressible | 1.7 MiB | 4.6 MiB (17.6 %) |
| `logs.parquet` | 21.1 MiB (45 %) | 21.8 MiB (43 %) | 6.3 MiB (24 %) |
| of which `body` | 20.6 MiB, 196 B per line, 1.3:1 | 20.6 MiB | 5.3 MiB, 18.5 B per line, 6.2:1 |
| of which the key columns (group, node, node_id, sequence, index, observed_ns) | 0.35 MiB, 3.3 B per row | 1.0 MiB, 9.5 B per row | 0.87 MiB, 3.0 B per row |
| `metrics.parquet` | 1.3 MiB (2.8 %), 11.7 B per point | 2.4 MiB (4.7 %) | 3.3 MiB (12.6 %) |
| of which `value_int` | 1.07 MiB, 9.5 B per point, 1.0:1 | 1.07 MiB | 2.9 MiB |
| `gaps.parquet` and manifests | 0.3 MiB | 0.3 MiB | 0.3 MiB |

Rows: 55,315 Batches, 110,630 lines and 118,400 points in each synthetic state; 150,010 Batches, 300,020 lines and 320,320 points in the real-text state (the lines are shorter, so 64 MiB holds more of everything).

**What re-encoding changes** (size against the file on disk):

| Change | synthetic steady | synthetic adversarial | real text |
| --- | ---: | ---: | ---: |
| control (pyarrow, Zstd 3, dictionary) | +0.3 % to +1.4 % | +0.3 % to +1.3 % | −1.5 % to +1.9 % |
| Zstd level 9 | 0 % | 0 % | logs −14.5 %, batches −3.8 %, metrics −1 % |
| delta encoding of integer columns | logs −0.7 %, metrics −16 % | logs −0.4 %, metrics −18 % | logs −12 %, metrics −16 % |
| series-first order | logs +0.3 %, metrics −3 % | logs +0.3 %, metrics −8 % | 0 % |
| series-first and delta | logs −0.8 %, metrics −16 % | logs −0.7 %, metrics −29 % | logs −12 %, metrics −15 % |
| logs without its body | 2.1 % of the table remains | 5.1 % | 14.2 % |
| batches without `sha256` | −6 % | −6 % | −28 % |

**Real lines alone** (bytes per line):

| Sample | raw | Zstd 3 stream | Zstd 19 stream | Parquet body column | template + variables | typed variables | distinct templates | per line, no dictionary | per line, trained dictionary |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Apache | 84.6 | 5.4 | 3.6 | 6.7 | 6.3 | 7.8 | 12 | 89.5 | 37.6 |
| HDFS | 142.9 | 27.0 | 21.4 | 30.3 | 30.2 | 31.3 | 1,017 | 128.1 | 53.0 |
| Hadoop | 191.5 | 8.7 | 6.7 | 11.0 | 10.6 | 10.6 | 277 | 163.9 | 116.6 |
| Linux | 107.2 | 7.5 | 5.6 | 9.7 | 8.5 | 11.2 | 197 | 102.8 | 57.4 |
| OpenSSH | 111.6 | 8.3 | 5.5 | 10.6 | 8.9 | 9.2 | 203 | 111.4 | 44.5 |
| Spark | 97.1 | 7.0 | 5.0 | 9.0 | 8.3 | 8.5 | 390 | 99.2 | 40.7 |
| Thunderbird | 161.6 | 15.3 | 11.5 | 17.8 | 16.8 | 19.0 | 806 | 134.8 | 58.7 |
| Zookeeper | 138.9 | 12.0 | 8.3 | 15.1 | 12.9 | 13.5 | 60 | 128.3 | 40.1 |

## Findings

- **The duplicate copy is the Segment.** The raw Batch bytes in `batches.parquet` and the `body` column in `logs.parquet` are the same text stored twice; together they are 92 % of a synthetic Segment and 63 % of a real-text one. Everything else (keys, times, nodes, metrics, gaps, manifests) is 3 to 5 bytes per row. On real text the raw copy costs twice the projection (77 B per two-line Batch against 2 × 18.5 B), because the Batch carries the OTLP framing and per-record attributes (file path, device, inode, offsets) with every line.
- **The hash is a fixed cost that real workloads make visible.** 32 bytes per Batch is 3.6 % of a synthetic Segment and 17.6 % of a real-text one, where Batches are small. The hash protects replay; whether it must be stored per Batch rather than per row group or per file is a design question, not a measurement.
- **Encodings barely matter for logs; they matter for metrics.** Block compression already finds the templates: the Parquet body column is within 25 % of a whole-file Zstd stream, and a crude template split gains 0 to 15 % over the plain column. Delta encoding takes 16 to 29 % off the metrics table, which is 3 to 13 % of a Segment. Zstd level 9 gains 14 % on real-text logs and nothing on synthetic. Sort order does not change size on logs; series-first helps metrics only when nodes' clocks differ.
- **Compressing records one at a time is the one layout that fails.** A line compressed alone costs as much as the raw line (90 to 164 B); with a trained dictionary 38 to 117 B; in a block 5 to 27 B. The journal tail is uncompressed protobuf, so on real text the tail costs 2.5 times the bytes of its Segment and, from [retention-scale run 01](retention-scale-run-01.md), about 3.2 ms per MiB per query: the unsealed tail is the most expensive place a record can be, in bytes and in time.
- **The synthetic workload hides all of this.** Its half-random bodies compress 1.3:1, so encodings, templates and the hash look negligible; real lines compress 6:1 in the column and 5:1 in the raw Batch, and the layout questions appear. The sealer and query figures measured on the synthetic workload stay valid for time; the storage figures do not transfer.

## Limits

- Real lines were drawn at random from eight 2,000-line samples, so the mix has no temporal structure (bursts, repeated lines in sequence) and compresses somewhat worse than a real stream would; 1 MiB journal files make row groups three to five times smaller than a product Segment's, which also costs compression.
- Sizes only; the read cost of each alternative (delta decode, dictionary lookups, a body fetched from the raw copy) was not measured.
- Metric values are random 63-bit integers, so `value_int` is uncompressible by construction; real gauges and counters would compress under delta or XOR encodings, and the metrics figures here are a floor.
- No Fabric code was changed; every alternative was produced by `pyarrow`, whose Parquet writer differs from `parquet-rs` in defaults (the control row shows the difference, under 2 %).

## Reproduce

Scripts and data under [data/storage-layout](data/storage-layout/): the composition and re-encoding scripts, the generator driver that builds the real-text state ([corpus_seal.py](data/storage-layout/corpus_seal.py.txt), using the sealer-study generator with `SEALBENCH_CORPUS` set to a file of lines), the per-sample results, and the JSON of every number above.
