# Tail key index, run 01

Status: **Exploratory.** This is the second experiment of the [research ledger](../../research/ledger.md) (entry L-03). No protocol was registered before it ran, so it is not **Measured** in the [evidence-state](../../QUALIFICATION.md#evidence-states) sense and decides no gate. It ran on 2026-10-02 with the stock server built from the code at `58b694d` and a research prototype of the same server, selected by an environment variable and never part of the product.

The question: does an in-memory index of the unsealed journal tail's keys bound the time and memory a query spends on the tail, on tails built to defeat it, without changing any answer?

Origin: [retention-scale run 01](retention-scale-run-01.md) found that a query's time and memory are proportional to the tail (4.64 s and about 858 MiB over 320 MiB), because `History::sources` decodes every frame and keeps every Group before any filter.

## Method

**The prototype** ([diff](data/tail-index/query-tail-index-prototype.diff.txt)). `History` holds a `TailIndex` behind a mutex: per journal entry, its group, frame offset and file, position in the frame, label (interned), receive time, the time bounds of its log rows and of its metric points, and whether it carries gaps. On each query the index is extended from the last indexed offset of each journal file to the end of its complete frames (files are keyed by their first group, so a rotation that renames the active file carries its offset over), and entries whose file is gone or whose group a Segment now covers are dropped. The query then selects entries by label, by the bounds of the table it reads, by the snapshot, and gap-bearing entries by receive time; decodes only the frames those entries live in; and hands the stock code a `journal` holding only the selected entries. The answer's receive bounds and per-node freshness are folded from every entry in the snapshot, not from the selection. Everything after that point is the stock code, so the stock server is the oracle. Rebuilding the index is the same decode the stock path does once per query; nothing is persisted.

**Tails.** Five 64 MiB active journal files from the sealer-study generator ([source](data/tail-index/sealbench-driver-tails.rs.txt)), each the server's own `batches.faj`, never rotated (`journal_file_bytes` 1 GiB):

| Tail | Shape | Why |
| --- | --- | --- |
| steady | the soak workload: 100 nodes, two 512-byte lines per node per second, 32 gauges every 15 s; 55,100 entries | the baseline |
| tiny | 16-byte bodies, 100 Batches per node per second, 10,000 entries per second; 303,500 entries | the most entries per byte: index memory and build time |
| needle | 1,000 identities, each sending every tenth second; 55,140 entries | a node filter that keeps one entry in a thousand |
| outage | 20 nodes offline for 240 s then draining their backlog; 55,100 entries | old row times late in the file |
| skew | 10 nodes with clocks 5 s ahead; 4 nodes sending gaps only; 57,195 entries | row times beyond receive times; entries with no rows |

**Measurements.** For each tail, the stock server and then the prototype on a fresh copy, server on CPUs 0 and 1: time to ready; memory after replay; the first query (a whole-window query that selects every entry) and memory after it; seven shapes ten times each over one keep-alive connection (an empty window in the past; logs of the last 10 s; one node's logs over 60 s; a rare text search over 60 s; one metric over the fleet for 15 s; a rate over 60 s; logs with `limit 50` over the whole window); memory after the shapes; 300 random queries with every page followed up to three hops, compared field by field between the two servers; memory after those. The prototype reports its entry count, byte estimate (entry capacity times entry size, plus labels) and timings on stderr.

**Rotation.** On the steady tail with `journal_file_bytes` 64 KiB, a one-identity simulator appended for 12 s while a thread ran the six narrow shapes in a loop against the prototype; the first append rotated the 64 MiB file, the sealer built its Segment and reclaimed it. Answers were checked for status and `complete`.

## Results

Median milliseconds per query, stock then prototype, and the rows returned (identical under both):

| Shape | steady | tiny | needle | outage | skew |
| --- | ---: | ---: | ---: | ---: | ---: |
| empty window | 221 / **4.1** | 817 / **21** | 236 / **10** | 231 / **4.2** | 227 / **4.8** |
| logs, last 10 s | 217 / **7.9** | 855 / 207 | 226 / **13** | 221 / **7.8** | 227 / **10** |
| one node, last 60 s | 216 / **5.9** | 848 / **39** | 224 / **9.3** | 233 / **5.8** | 234 / **5.3** |
| text, last 60 s | 222 / 29 | 879 / 935 | 234 / 36 | 230 / 31 | 245 / 37 |
| fleet metric, last 15 s | 231 / **7.3** | 911 / 125 | 234 / **12** | 238 / **7.5** | 228 / **8.4** |
| rate, last 60 s | 222 / **13** | 884 / 587 | 236 / **17** | 230 / **13** | 226 / **15** |
| logs, `limit 50`, whole window | 229 / 263 | 887 / 992 | 241 / 291 | 244 / 280 | 242 / 305 |

The 300 random queries with their pages: stock 128 s, prototype 10.7 s (steady); 518 s and 152 s (tiny); 123 s and 12.3 s (needle); 131 s and 10.6 s (outage); 137 s and 11.4 s (skew). **Mismatches: 0 of 1,500 queries and their pages, on every tail.**

The index:

| Tail | Entries | Index bytes | Per entry | Build (first extend) |
| --- | ---: | ---: | ---: | ---: |
| steady | 55,100 | 5.0 MiB | 95 B | 131 ms |
| tiny | 303,500 | 40.0 MiB | 138 B | 544 ms |
| needle | 55,140 | 5.1 MiB | 97 B | 149 ms |
| outage | 55,100 | 5.0 MiB | 95 B | 136 ms |
| skew | 57,195 | 5.0 MiB | 92 B | 141 ms |

Memory, MiB, high-water mark (resident in brackets):

| Point | steady stock | steady prototype | tiny stock | tiny prototype |
| --- | ---: | ---: | ---: | ---: |
| after replay | 7 | 8 | 8 | 9 |
| after the first, whole-window query | 179 (147) | 183 (152) | 395 (205) | 418 (229) |
| after the seven shapes | 219 (219) | 189 (189) | 413 (215) | 440 (250) |
| after the random set | 226 (226) | 190 (190) | 419 (215) | 470 (280) |

Rotation: 1,908 answers during the append, seal and reclaim, 0 errors, every answer `complete`; the index held 55,100 entries before, 55,103 at most, and 12 after the Segment covered the old file.

## Findings

- **For every selective shape the tail's cost disappears.** Empty windows, node filters, short windows and fleet metrics over short windows fall from 216 to 238 ms to 4 to 13 ms on the four 55,000-entry tails, and the random set runs twelve times faster. The prototype's share of a selective query is the frame decodes of the selected entries plus a few milliseconds.
- **Answers are unchanged.** 1,500 random queries and their pages, including skewed clocks, gap-only nodes and drained backlogs, matched the stock server field for field; 1,908 answers during a rotation and seal were complete and error-free, and the index forgot the sealed file's entries.
- **The index is small and cheap to build.** 92 to 97 bytes per entry on ordinary tails, 138 on the tiny tail (interned labels and hash-map overhead weigh more against 16-byte bodies); 5 MiB for a 64 MiB tail, 40 MiB for 303,500 entries. Building it costs about one stock query (131 to 149 ms; 544 ms for the tiny tail), paid once rather than per query.
- **It does not bound a wide window.** A query whose window covers the tail selects every entry and decodes every frame, as the stock path does, and the whole-window `limit 50` shape is 15 to 25 % slower than stock for the extra bookkeeping. The high-water mark after such a query is the same as the stock server's (183 against 179 MiB). The index bounds the cost of *selection*, not of *materialisation*; bounding the latter needs the heap's threshold applied to the index's sorted bounds (ledger L-04 at entry level) or a budget (L-05).
- **The tiny tail is the limit case.** With 303,500 entries in 64 MiB, a 60 s window covers most of the tail, so the text and rate shapes gain little or lose (935 against 879 ms; 587 against 884 ms), the per-entry overhead of selecting and cloning dominates, and resident memory after the random set is 280 MiB against the stock server's 215, because the index (40 MiB) and its selections sit beside the decoded frames. Selective shapes still gain thirty to forty times.
- **Ready time and replay are unaffected**: the index is built lazily by the first query, not at start-up.

## Limits

- One host, warm page cache, synthetic tails, 10 repetitions per shape.
- The prototype holds the index lock for the whole query, so concurrent queries serialise; a product design would extend under the lock and select under a read lock.
- ACK latency while the index extends on the two-CPU profile was not measured; the extension runs on the query's blocking task, not the commit thread, but it competes for the same CPUs. That belongs to the registered soak.
- The byte figure is the entry vector's capacity times the entry size plus labels; it is an estimate of the index's own allocation, not a process measurement. The process measurements above include everything.
- The hostile list's "10,000 one-row Batches per second" was run as two 16-byte rows per Batch at 10,000 Batches per second.

## Reproduce

[tailbench.py](data/tail-index/tailbench.py.txt) takes an output root and the tail kinds; it expects the stock and prototype servers under `bin/stock` and `bin/proto` and the generator built from the sealer-study harness with the tail workloads. Results: [tail-index.json](data/tail-index/tail-index.json) (every point, every memory sample, the index's own report per query, the differential), [run.log.txt](data/tail-index/run.log.txt).
