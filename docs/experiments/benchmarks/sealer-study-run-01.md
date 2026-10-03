# Sealer study, run 01

Status: **Exploratory.** This study compared candidate algorithms for building a Segment, to choose one. It ran before any protocol was registered, so its results are not **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense and decide no gate. The decision it informed is [ADR-0022](../../decisions/ADR-0022-build-segments-by-external-merge-sort.md). The protocol that decides the implementation is registered in the [bounded-sealer milestone](../../milestones/bounded-sealer.md#registered-acceptance-protocol), before the implementation exists.

The question: how can the sealer turn one sealed journal file into a Segment without holding the whole file in memory, while producing the same answers to every query?

Origin: [soak run 01](soak-run-01.md) failed its `no_rss_growth` gate. Its analysis named the sealer's working set, about ten times a journal file, as the cause.

## Method

**Candidates.** All six prototypes call the server's own row extraction ([rows.rs](../../../crates/fabric-server/src/rows.rs)), table schemas and Parquet writer settings (Zstd level 3, at most 8,192 rows per row group). They differ only in how rows are ordered and buffered. Option G (smaller journal files) is today's code with a different configuration value.

| Id | Candidate | How it orders the log rows and metric points |
| --- | --- | --- |
| Current | `segment::build` at `58b694d` | loads the whole file, sorts all rows, builds each file in a buffer |
| A | chunk sort | sorts each row-group-sized chunk alone |
| C | watermark window | a bounded min-heap reorders a window of rows; rows behind the watermark go to separate late row groups |
| D | range partition | pass 1 reads keys only and picks splitters; pass 2 spills rows into time buckets; each bucket is sorted and written in order |
| E | external merge sort | sorts byte-capped runs, spills them, then merges the runs with a heap of one row per run |
| H | window plus merge | C for the in-order stream; late rows go to an external merge, giving a second sorted sequence |
| G | 8 MiB journal files | today's code, sealing every 8 MiB |

The raw Batches table and the gaps table keep journal order in every candidate and stream in 4 MiB chunks. Every in-memory buffer in A, C, D, E and H is capped in estimated bytes (16 MiB for a window, run or bucket; 8 MiB for an output row group) as well as in rows. Spill files are not synced.

**Workloads.** Each is one 64 MiB sealed journal file of Groups, built by a seeded generator (`0xA11FA001`) modelled on the soak's fleet simulator: 100 nodes, two log lines per node per second, 32 metric points every 15 s, a gap text every 500th Batch.

| Workload | What it stresses |
| --- | --- |
| steady | the soak shape: log bodies of 512 bytes, alternating a repeated byte and seeded entropy; rows arrive in time order |
| outage | 20 of the 100 nodes are offline for 240 s of the roughly 550 s the file covers, then drain their backlog, so their old timestamps arrive late (14,138 rows are late at a window of 16,384) |
| adversarial | every row's time is random within a 520 s window, the worst case for any reorder window |
| bigrows | 16 KiB log bodies, which makes a limit counted in rows differ greatly from a limit counted in bytes |

Each of the first three has 110,200 log rows and 118,400 metric points; bigrows has 4,070 and 6,400.

**Measurements.** Each configuration ran three times, each in its own process, on a 4-CPU container, with input and output on tmpfs. Peak heap is the largest excess of live allocated bytes over the process's starting level, counted by a global allocator that wraps the system allocator. The peak was identical across the three runs of every configuration. `VmHWM` and the process I/O counters (`rchar`, `wchar`) were also recorded. One extra series ran five algorithms three times each on a real ext4 disk (`/dev/vda`), with the page cache dropped before each run. Memory scaling used steady journal files of 16, 32, 64, 128 and 256 MiB.

**Correctness checks.** For every configuration the candidate Segment was read back through `segment::scan_logs`, `scan_metrics`, `scan_gaps` and `scan_batches` and compared with the Segment the current sealer built from the same file:

- the same set of rows in each table, and equal manifests apart from file hashes;
- the same row order in the file (a hash over rows in file order);
- `segment::verify` passing on the candidate's own manifest hashes;
- no temporary file left in the Segments directory;
- query read amplification: for sliding 60 s and 10 s windows (half-window steps), the rows in the row groups whose time range overlaps the window, per row in the window, computed from the Parquet row-group statistics that `segment::prune` uses.

Determinism: the outage and adversarial workloads were built twice by each of the six candidates, and the manifests, which carry every file's SHA-256, were identical across the two runs in every case.

## Results

Peak heap, MiB, building one Segment from one 64 MiB file:

| Algorithm | steady | outage | adversarial | bigrows |
| --- | ---: | ---: | ---: | ---: |
| Current | 355.9 | 355.0 | 356.0 | 300.0 |
| A chunk sort | 29.6 | 29.5 | 29.1 | 39.5 |
| C window 16,384 | 43.8 | 49.1 | 48.0 | 60.4 |
| D range partition | 41.6 | 41.6 | 42.6 | 49.4 |
| E external merge | 42.5 | 42.5 | 42.7 | 43.0 |
| H window plus merge | 43.8 | 49.1 | 63.5 | 64.6 |

Peak heap against file size, steady workload, MiB:

| Algorithm | 16 MiB | 32 MiB | 64 MiB | 128 MiB | 256 MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| Current | 91.8 | 180.7 | 355.9 | 700.0 | 1,388.9 |
| A chunk sort | 27.9 | 29.6 | 29.6 | 29.8 | 30.5 |
| D range partition | 40.1 | 40.2 | 41.6 | 41.9 | 48.0 |
| E external merge | 42.5 | 42.5 | 42.5 | 44.9 | 44.9 |
| H window plus merge | 41.4 | 43.8 | 43.8 | 44.0 | 44.7 |

D's key index, one entry per row, was 3, 6, 12, 24 and 48 MiB at those sizes, which is why its curve rises.

Query read amplification, log rows, 60 s windows (rows read per row returned; the current layout is the baseline):

| Algorithm | steady | outage | adversarial | bigrows |
| --- | ---: | ---: | ---: | ---: |
| Current | 1.66 | 1.66 | 1.62 | 1.00 |
| A chunk sort | 1.66 | 2.23 | 9.27 | 1.00 |
| C window 16,384 | 1.66 | 1.81 | 4.98 | 1.00 |
| D range partition | 1.66 | 1.66 | 1.62 | 1.00 |
| E external merge | 1.66 | 1.66 | 1.62 | 1.00 |
| H window plus merge | 1.66 | 1.81 | 2.11 | 1.00 |

Wall time, seconds, median of three, tmpfs (indicative only):

| Algorithm | steady | outage | adversarial | bigrows |
| --- | ---: | ---: | ---: | ---: |
| Current | 1.41 | 1.52 | 1.29 | 0.73 |
| A chunk sort | 0.91 | 0.86 | 0.89 | 0.48 |
| C window 16,384 | 1.10 | 1.10 | 1.14 | 0.54 |
| D range partition | 1.26 | 1.23 | 1.14 | 0.55 |
| E external merge | 1.08 | 1.10 | 1.17 | 0.55 |
| H window plus merge | 1.21 | 1.13 | 1.44 | 0.44 |

On a real disk, steady workload, three runs: Current 1.22 to 1.62 s; E 1.07 to 1.21 s; H 1.19 to 1.29 s; D 1.24 to 1.41 s; A 0.94 to 1.08 s.

Bytes moved per Segment, steady workload, MiB:

| Algorithm | Read | Written | Spill |
| --- | ---: | ---: | ---: |
| Current, A, C and H | 64 | 46 | 0 |
| D range partition | 201 | 119 | 72.9 |
| E external merge | 137 | 119 | 72.9 |

H spilled 4.3 MiB on outage and 41.6 MiB on adversarial. Process `VmHWM` for E ranged from 57 to 70 MiB, which is the counted peak plus the process's own baseline and allocator overhead.

Eight 8 MiB journal files (option G), built one after another, peaked at 49.0 MiB for today's code, against 26.5 MiB for A and 33.0 MiB for E. Memory still scales with the file; it only shrinks with it.

### Findings

- **Every candidate kept every row.** All four tables held the same rows as today's Segment, with equal manifests, on every workload, and `segment::verify` passed.
- **Order mattered.** E and D reproduced today's row order on every workload. A, C and H did not once rows arrived out of order, and the cost is permanent: queries read up to 5.7 times more rows (A, adversarial), 3.1 times (C) and 1.3 times (H).
- **E reproduced today's logs and metrics files byte for byte** on steady, outage and adversarial. On bigrows only the row-group boundaries differ, because the byte cap closes row groups early; the decoded rows are equal. D never matched byte for byte, because its row groups end at bucket boundaries.
- **A limit in rows is not a limit in memory.** In an earlier pass of this study, before byte caps, C, D, E and H peaked at 168 to 191 MiB on bigrows: its 4,070 log rows never reached a 16,384-row window or a 32,768-row run, so the whole file's log rows stayed in memory. A 16 MiB cap in estimated bytes fixed it. The raw results of that pass were overwritten by the later runs, so only these figures remain. The bigrows workload is kept as a regression workload for that reason.
- **E's memory is flat in the file size** (42.5 to 44.9 MiB from 16 to 256 MiB). Today's is 5.4 to 5.7 times the file at every size.
- **Speed is not the constraint.** Every candidate built a 64 MiB Segment in 0.4 to 1.9 s. In [soak run 01](soak-run-01.md) a journal file takes about 9 minutes to fill at 100 identities, so a seal is about 0.2 % of that time.

## Limits

- The workloads are synthetic. Real log bodies may compress differently, which changes output size and Zstd time but not the memory bound.
- Estimated bytes in `approx_bytes` are a model (struct size plus string lengths plus 64 bytes per attribute), not exact. The measured peak is the evidence for the ceiling, not the estimate.
- Timings come from one 4-CPU container, mostly on tmpfs, where fsync costs nothing. One disk series is the exception. They compare candidates with one another and say nothing about the target host.
- The study did not run the sealer beside a live commit thread, so the effect of spill I/O on ACK p99 is unmeasured. The [registered protocol](../../milestones/bounded-sealer.md#registered-acceptance-protocol) measures it.
- Not prototyped: sealing incrementally while the journal file is still active, sealing in a child process, a different allocator, and pipelining the stages across threads. ADR-0022 gives the reasons.

## Reproduce

The prototype is not product code. Its source is kept beside the data as text so the study can be rerun on the revision it names: [module](data/bounded-sealer/sealbench-module.rs.txt) (to be placed at `crates/fabric-server/src/sealbench.rs`, with `pub mod sealbench;` in `lib.rs` and the helpers in `segment.rs` made `pub`), [driver](data/bounded-sealer/sealbench-driver.rs.txt) (`crates/fabric-server/examples/sealbench.rs`) and the [matrix script](data/bounded-sealer/matrix.py).

```sh
cargo build --release -p fabric-server --example sealbench
target/release/examples/sealbench gen steady /dev/shm/sb/steady 67108864 67108864
python3 -B matrix.py 3
```

`gen` takes `steady`, `outage`, `adversarial` or `bigrows`, an output directory, a byte target and a journal file size. Raw results are in [data/bounded-sealer](data/bounded-sealer/): `matrix.json` (every run and every check), `scale.jsonl`, `disk.jsonl` and `g.jsonl`.
