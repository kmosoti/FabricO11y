# Observation encoding, run 01

Status: **Exploratory.** Sizes and codec speed of the version-one Observation record's FOB1 block encoding ([fabric-observation](../../../crates/fabric-observation/src/lib.rs), [ADR-0023](../../decisions/ADR-0023-define-an-observation-record-with-a-canonical-encoding.md), proposed) against the raw OTLP Batch bytes Fabric stores today, measured on 2026-10-02 over the journals of [storage layout run 01](storage-layout-run-01.md). No protocol was registered; nothing here is **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense. The product does not emit or store FOB1; this run says what it would cost if the Spindle did.

The question: if the node emitted its observations in one canonical block encoding instead of OTLP protobuf, how many bytes would a record cost in the journal and in a Segment's custody table, and what does encoding and decoding cost?

## Method

A scratch subcommand of the sealer-study generator ([source](data/observation-encoding/sealbench-fob.rs.txt)) reads the first 64 sealed 1 MiB journal files of a workload, and for every Entry: counts the raw Batch bytes; extracts the rows the stock server extracts (`rows::extract`); builds one `Observation` per line (severity 0, no event, the line's string attributes) and per point (name, unit, gauge or sum, the string attributes); encodes them as one FOB1 block per Batch, decodes it back and asserts equality; and concatenates. Per file it also encodes all of the file's records as one block (at most 65,536 records). Zstd level 3 is applied three ways: to each Batch or block alone, to each file's concatenation of Batches or of per-Batch blocks (what `batches.parquet` does, one Zstd page per row group), and to the per-file block. Encode and decode time is the sum over all per-Batch blocks, on CPUs 2 and 3 while another run used 0 and 1.

What FOB1 drops relative to the OTLP bytes is exactly what the stock projections drop: resource attributes, scope, non-string attribute values, severity and event name (set to zero and empty here), and the OTLP framing. That is why the conversion cannot be done at the server without losing custody of the node's bytes; the comparison is between two things the node could send.

Workloads: synthetic steady and adversarial (two 512-byte half-random lines per node per second, 32 random-valued gauges every 15 s), and real text (the same shape with bodies drawn from 15,994 real log lines).

## Results

Bytes per record (a line or a point), over 229,030 records (synthetic) and 620,340 (real text):

| Encoding | synthetic steady | synthetic adversarial | real text |
| --- | ---: | ---: | ---: |
| raw OTLP Batch bytes | 285.9 | 285.9 | 100.1 |
| raw Batches, Zstd per Batch | 133.9 | 137.7 | 74.4 |
| raw Batches, Zstd per file (today's custody table, before Parquet overhead) | 103.1 | 108.8 | 18.2 |
| FOB1, one block per Batch | 279.2 | 283.0 | 93.9 |
| FOB1 per Batch, Zstd per Batch | 132.7 | 138.4 | 74.4 |
| FOB1 per Batch, Zstd per file | 105.8 | 113.1 | 17.8 |
| FOB1, one block per file | 265.1 | 268.7 | 79.4 |
| **FOB1 per file, Zstd** | **101.9** | **109.7** | **14.5** |

Totals for 64 files: synthetic raw 62.4 MiB, FOB1 per file 57.9 MiB, both about 22.3 to 22.5 MiB under Zstd; real text raw 59.2 MiB, FOB1 per file 47.0 MiB, Zstd 10.8 MiB against 8.6 MiB.

Codec cost: encode 870 to 940 ns per record, decode 450 to 500 ns per record, including the per-Batch block's dictionaries and the equality assertion's clone.

## Findings

- **On real text the block encoding saves a fifth before compression and a fifth after it.** 79 against 100 bytes per record raw; 14.5 against 18.2 under Zstd per file. The saving is the OTLP framing and the per-record repetition of attribute keys and node identity that the block's dictionaries and delta columns remove; Zstd recovers most of the rest either way, which is why the compressed gap is no larger than the raw one.
- **On the synthetic workload the two are the same**, within 1 % compressed and 7 % raw, because the 512-byte bodies (half repeated bytes, half random) are the record and nothing encodes them smaller. The synthetic workload cannot distinguish encodings; the real-text state can.
- **Compressing per Batch is the layout to avoid, whatever the encoding**: 74 bytes per record against 14.5 to 18.2 per file, on both encodings. A journal that compressed Batches one at a time would gain little; a block per file, or per row group, is where the bytes go.
- **As a custody table it would be about half of today's on real text.** `batches.parquet` is 16.1 MiB for these 64 files ([storage layout run 01](storage-layout-run-01.md)), of which 11.1 MiB is Batch bytes and 4.6 MiB the per-Batch hash; the per-file FOB1 blocks are 8.6 MiB and carry one CRC each, with a hash per block if custody needs one. The projections would stay as they are, so the Segment would fall from 26.0 to about 18.4 MiB (−29 %). On synthetic text the saving is about 5 % of the Segment.
- **The codec is cheap enough to sit on the node's path**: under a microsecond per record to encode, half that to decode, against the Spindle's one-second poll and the server's 0.6 µs per row of Parquet materialisation.

## Limits

- Severity and event name were zero and empty, as Fabric's projections have them; a node that filled them would add a byte and a dictionary id per line.
- One block per 1 MiB file; a 64 MiB file would hold several blocks of 65,536 records, with the same per-record figures and a little less dictionary overhead.
- FOB1 blocks are not Parquet: no row-group statistics, no column-wise reads without decoding the block; the figures speak to the custody copy and the wire, not to the projections.
- Synthetic metric values are random 63-bit integers; real gauges would compress under the delta column and the figures for points are a floor.
- One host, one run, warm page cache.

## Reproduce

Data under [data/observation-encoding](data/observation-encoding/): the three JSON results and the measuring subcommand's source. The subcommand runs in the sealer-study scratch workspace against the journals of storage layout run 01.
