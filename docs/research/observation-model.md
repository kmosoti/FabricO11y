# A cost model of the Observation record

Status: **exploratory research**, tier 6 of the [source-of-truth order](../README.md#source-of-truth). The model is a small standard-library Python module, [tools/model/observation_model.py](../../tools/model/observation_model.py), with its calibration bounds in [test_observation_model.py](../../tools/model/test_observation_model.py). It prices the [Observation record and FOB1 encoding](../architecture/observation.md) ([ADR-0023](../decisions/ADR-0023-define-an-observation-record-with-a-canonical-encoding.md), proposed) as closed forms from the block layout, fits the machine's constants to measurements, and turns them into capabilities on the registered two-CPU profile. It exists so that a design question (block size, attribute shape, what a CRC costs, how many nodes a server carries) is answered by changing a parameter and re-running, and only then by building. Its first use found and fixed a three-fold cost in the codec before anything shipped.

## 1. The structure, as arithmetic

Every term below is read off the [block layout](../architecture/observation.md#the-block). `vl(x)` is the length of the shortest LEB128 varint, `⌈bits(x) / 7⌉` with a floor of one byte; `Δ(x)` is `vl(zigzag(x))`, the length of a step of magnitude `x`.

For a block of `N` records with `k` distinct node ids and `S` distinct strings:

| Part | Bytes |
| --- | --- |
| header and CRC | `4 + vl(N) + 4` |
| node table | `vl(k) + 16k` |
| string table | `vl(S) + Σ (vl(len) + len)` over the `S` strings |
| times | `vl(t₀) + (N − 1) · Δ(j)` where `j` is the typical second-order step (0 for a regular series, the jitter otherwise) |
| strands | `(N − 1) · (vl(node id) + Δ(Δgeneration) + Δ(Δsequence) + Δ(Δindex))` plus the first record's absolute values |
| tags | `N` |
| locators | `24 N · share with locators` (+ 8 with a parent) |
| attributes | `N · (1 + a · (id + 1 + id))` for `a` attributes per record, `id = vl(S − 1)`, string values by id |
| line payload | `1 + vl(event id) + vl(len) + len(body)` |
| point payload | `vl(name id) + vl(unit id) + 1 + [Δ(start)] + 1 + value` where an integer value is `Δ(v)` and a double 8 |
| span payload | `vl(name id) + Δ(end − start) + 2` |

Consequences that fall out of the arithmetic before any measurement:

- **The floor for a bare line is 10 bytes** at any block size above a few hundred: one time step, four strand steps, a tag, an attribute count, a severity, an event id, a length. Everything else is the body.
- **Block size amortises the tables.** With one attribute and real-text defaults, a one-record block costs 163 bytes per record, 16 records 109, 256 records 90, 4,096 records 84, 65,536 records 83.5. Above 256 records the remaining overhead is the per-record floor, not the block.
- **An attribute costs 3 bytes** when its key and value repeat within the block (two ids and a tag), plus its share of the table; a value that never repeats (a file offset as a string) costs its length plus a 2 to 3-byte id. The same value as an integer costs `Δ(v)`: 2 to 4 bytes and no table entry.
- **A regular series costs one byte per timestamp**; a jitter of 1 µs costs 3, of 1 s costs 5.
- **Dictionary ids widen with the table**: one byte below 128 strings, two below 16,384, three above. A block with 65,536 high-cardinality string values pays three bytes per reference.

Compression is modelled as three components under their own ratios, because they behave differently under a general compressor: the log text, the point values, and everything else (ids, steps, tags). The ratios are fitted, not derived: real lines contiguous in a block compress about 9:1 under Zstd 3 (the Parquet body column, in smaller pages, reached 7:1); the soak generator's lines (half repeated bytes, half random) 2.6:1; random 63-bit gauge values 1:1; the rest about 4:1.

## 2. The machine

Time per record is a linear model fitted by least squares on 92 measured points ([fobbench.csv](../experiments/benchmarks/data/observation-model/fobbench.csv), [source](../experiments/benchmarks/data/observation-model/fobbench.rs.txt)): a per-block cost, a per-record base, a per-body-byte cost, a per-attribute cost, an extra per attribute whose value is new to the block, and extras for points and spans.

| Constant | Encode | Decode | What it is |
| --- | ---: | ---: | --- |
| per block | 285 ns | 440 ns | six column buffers, two dictionaries, the output |
| per record | 135 ns | 100 ns | the fixed columns |
| per body byte | 1.1 ns | 0.9 ns | two copies and the CRC (3.0 and 3.3 before the slicing CRC, below) |
| per attribute | 55 ns | 235 ns | a hash lookup per key and value; two `String` clones on decode |
| per new string value | 50 ns | 190 ns | a table insert |
| per point, per span | 0 | 160 ns, 65 ns | name and unit clones |

Median error of the fit: 15 % on both sides; the worst points are one-record blocks, where the per-block cost dominates and varies most. The byte model is exact: 0.2 % median error over the same 92 points, 2 % and 8 % on the two journals of [encoding run 01](../experiments/benchmarks/observation-encoding-run-01.md), 3 % and 13 % after compression ([calibration.txt](../experiments/benchmarks/data/observation-model/calibration.txt)).

From these, with the registered profile (two cores for Fabric, here taken at 3 GHz), a 25 % CPU share for the codec stage, a 200 MB/s disk, the 4 GiB journal cap and 20 GiB retention: records per second, journal and Segment bytes per second, hours of retention and of journal fill, the server's CPU share (one decode to verify and one Zstd pass to seal per record), the node's microseconds per second, and the memory a decoded block occupies (200 bytes of struct per record plus its strings and attributes).

## 3. The first iteration the model paid for

The fitted per-byte cost on the bytes-up tower was 3.3 ns, three times the earlier crate's, and the model attributed it to the CRC, the only per-byte term that had changed: a bytewise table CRC runs at two to three cycles per byte, and a block is hashed once on each side. The classic remedy, slicing by eight (eight 256-entry tables, derived from the first at compile time), was written against the same bit-serial definition, checked by the exhaustive test, the properties and a Kani harness, and re-measured ([before](../experiments/benchmarks/data/observation-model/fobbench-bytewise-crc.csv), [after](../experiments/benchmarks/data/observation-model/fobbench.csv)):

| Case (4,096-record blocks) | encode before | after | decode before | after |
| --- | ---: | ---: | ---: | ---: |
| bare line | 197 ns | 150 ns | 131 ns | 90 ns |
| 64-byte line | 387 | 224 | 347 | 176 |
| 256-byte line | 952 | 378 | 936 | 375 |
| 1,024-byte line | 3,328 | 1,084 | 3,499 | 997 |
| 64-byte line, 5 attributes | 700 | 470 | 1,390 | 1,327 |
| point, 1 attribute | 189 | 113 | 487 | 376 |

The per-byte term fell from 3.3 to 0.9 ns, as the model predicted a CRC change would, and the per-attribute term did not move, as it predicted it would not.

The second iteration took the terms the first left largest on the encode side: 650 ns per block (unsized buffers) and 105 ns per attribute plus 375 ns per new value (a `BTreeMap` intern). The buffers are now sized from the records and the intern is an open-addressing table over an FNV-1a hash, both built in the crate so it stays on `core` and `alloc` ([before](../experiments/benchmarks/data/observation-model/fobbench-btree-intern.csv), [after](../experiments/benchmarks/data/observation-model/fobbench.csv)):

| Case | encode before | after | decode before | after |
| --- | ---: | ---: | ---: | ---: |
| one bare line per block | 757 ns | 314 ns | 669 ns | 460 ns |
| 4,096 lines, 64 B, 10 attributes | 1,051 | 508 | 2,062 | 2,053 |
| 4,096 lines, 64 B, 3 attributes, every value new | 2,035 | 609 | 1,815 | 1,659 |
| 4,096 spans, 3 attributes | 414 | 218 | 729 | 730 |
| 4,096 lines, 1,024 B | 1,084 | 1,051 | 997 | 1,067 |

The per-block encode cost halved, the per-attribute encode cost halved, the new-value cost fell seven-fold, and the decode side did not move, as the model said it would not: its 235 ns per attribute is the clone of two `String`s, and the next iteration is a decoder that borrows from the block.

## 4. What the model says about configuration

- **Blocks of 256 records or more.** Below that the tables dominate; above 4,096 the gain is under one byte per record. A block per 1 MiB journal file (3,000 to 10,000 records on the measured workloads) is in the flat region; a block per Batch (two to thirty-four records) is not, which the per-Batch measurements of run 01 showed (94 against 79 bytes per record raw).
- **The record cap.** `MAX_RECORDS` is 65,536. A decoded block of 4,096 real-text records with five attributes occupies about 2.7 MiB; 65,536 would occupy about 43 MiB, half the sealer's 80 MiB ceiling ([bounded sealer](../milestones/bounded-sealer.md)). A product cap of 8,192, the Parquet row-group size, keeps a decoded block under 6 MiB and loses nothing in bytes.
- **Attributes shape the cost more than bodies do.** On real text a line's body costs 130 bytes raw and 14 compressed; five string attributes cost 15 bytes raw and about 1.5 µs of codec time, four fifths of it decode-side cloning. The Spindle's five `log.file.*` attributes should be two strings (path, and device as a string if it must) and three integers.
- **Capacity on the two-CPU profile** ([capabilities.txt](../experiments/benchmarks/data/observation-model/capabilities.txt)), real text with one attribute, two lines and two points per node per second:

| Nodes | Records/s | Journal MB/s | Segment MB/s | Retention at 20 GiB | Journal fill at 4 GiB | Server CPU share |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 100 | 413 | 0.03 | 0.01 | 48 days | 35 h | 0.0 % |
| 1,000 | 4,133 | 0.34 | 0.05 | 4.8 days | 3.5 h | 0.2 % |
| 10,000 | 41,333 | 3.4 | 0.52 | 11.5 h | 0.4 h | 1.7 % |
| 100,000 | 413,333 | 34 | 5.2 | 1.2 h | 2.5 min | 17 % |

  The codec stage is not the limit anywhere in that range: at a 25 % share of two cores it carries about 145,000 nodes; the disk about 500,000. Retention is: at 10,000 nodes, 20 GiB holds eleven and a half hours of real text, and the 4 GiB journal fills in 24 minutes, so the sealer must keep up at 3.4 MB/s in and 0.5 MB/s out. With five attributes per line the figures are 65,000 nodes for the codec and 8 hours of retention; with the synthetic soak workload (512-byte lines, random gauges) 3.8 hours.

## 5. Limits

- One host, one compiler, one container; the constants are this machine's. The structure of the model transfers; the numbers want refitting on the target host from the same bench.
- The time fit is a planning tool, good to about 20 % in the middle and worse at one-record blocks and at high attribute cardinality; it does not model cache effects, the allocator, or contention with the commit thread on two CPUs.
- The compression ratios come from two measured points and three fitted parameters; they describe those workloads, not real logs in general.
- The model prices the codec, the journal and the Segment's custody copy. It does not price the Parquet projections, queries, TLS, the commit path or replay; the server's CPU share above is the codec and compression stages only.
- Nothing here is **Measured** in the [evidence-state](../QUALIFICATION.md#evidence-states) sense; the capability tables are predictions to be falsified by the registered soak when the record is adopted.

## 6. How to iterate

Change a `Workload`, `Machine` or `Fleet` field and run `python3 -B tools/model/observation_model.py`; the calibration bounds in `test_observation_model.py` fail if a change to the codec or the model moves them apart. A change to the codec is measured by rebuilding the bench ([fobbench.rs](../experiments/benchmarks/data/observation-model/fobbench.rs.txt)) and refitting; a change to the structure is a new term in `block_bytes`, which the byte calibration checks to the byte.
