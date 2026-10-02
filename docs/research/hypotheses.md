# Hypothesis suite: storage against query

Status: **exploratory research**, tier 6 of the [source-of-truth order](../README.md#source-of-truth). This page turns the question "is the next bottleneck in the storage mechanisms or the query mechanisms?" into tests. Each hypothesis has a null, a statistic, a decision rule and the states it runs on, written before the test runs, so that a result can falsify it. The [ledger](ledger.md) tracks states; this page is the design of the next experiments, ranked by what each would decide. The [charter](frontier-map.md) still holds: one experiment at a time, the stock server as oracle, no answer may change.

## The verdict the evidence supports today

Latency and capacity have different bottlenecks, and both sit on the storage side.

- **Latency is set by the unsealed tail's representation.** The stock path pays 4.7 µs per tail entry on every query, because the tail is OTLP protobuf decoded whole ([real-corpus query run 01](../experiments/benchmarks/real-corpus-query-run-01.md)); real text holds 2.7 times the entries per byte, so a 64 MiB real tail costs about 700 ms per query and the 4 GiB `journal_bytes` allows about 45 s. The walk makes every shape that can stop cost its answer (10 to 50 ms) and leaves the shapes that cannot (a text search with no match, a rate) at 8.3 µs per entry. On Segments the query side is already within 10 to 20 ms where the data lets it stop ([threshold run 01](../experiments/benchmarks/topk-run-01.md)) and bounded by rows read elsewhere. No query algorithm removes a per-entry decode of a format that has to be decoded whole; what the tail holds is the lever.
- **Capacity is set by the custody copy.** Retention hours follow bytes per record in a Segment, and 63 % of a real-text Segment is the payload stored twice plus the per-Batch hash ([storage layout run 01](../experiments/benchmarks/storage-layout-run-01.md)); the [cost model](observation-model.md) puts 20 GiB at 11.5 hours for 10,000 nodes of real text.

The suite below tests that verdict rather than assuming it: the first group would overturn it if the tail's cost turned out to be in the query code; the second decides how much the storage levers are worth; the third keeps the query side honest where it still has terms to remove; the fourth questions the sealing and segmentation design itself, which every run so far has taken as given.

## What counts as an oracle here

Nothing in the current design is the oracle, the stock server included. A differential against it proves only that a prototype answers as today's code does, which is the right guard for a query prototype that must not change answers and the wrong one for a hypothesis about whether today's layout, sealer or Segment is the right design. The references are, in order:

1. **The product contract** and the registered contracts (HIST-1 to HIST-6): a Segment answers every registered query exactly as the journal it was sealed from, rows and order; replay from Segments rebuilds the same state; rates follow the counter contract.
2. **The journal through the independent Python oracles** ([query_oracle.py](../../tools/qualification/query_oracle.py), the delivery and rate oracles), which compute answers from the journal's bytes without the Rust code. A new layout is right when the oracle's answer over the journal equals the server's answer over the new layout.
3. **The stock server**, as a regression reference only, for prototypes that claim to change nothing.

Each hypothesis below names which of these it is judged by. A storage or sealing hypothesis is never judged by the stock server.

## Conventions

- **Statistic.** Median of eight repetitions over one keep-alive connection unless stated; memory as the process high-water mark; bytes from file sizes. A ratio is the test figure divided by the baseline figure on the same state.
- **Guard.** A query prototype runs the 200-query random differential with pages against the stock server; one mismatch fails it whatever the figures say. A layout or sealer prototype runs the Python query oracle over the journal against the server over the new layout, and the HIST-1/2 equivalence on the Segments it builds; one mismatch fails it.
- **States.** The four of run L-21 (real-text and synthetic tails of 64 MiB; real-text and synthetic Segments, 64 of 1 MiB) unless stated; "64 MiB Segments" means the product's size, which no run has used yet.
- **Decision.** The decision rule is written as the condition under which H0 is rejected. A result that rejects neither (noise, a broken run) is "inconclusive", not support.

## Group A: is the tail's cost in the format or in the query code?

**A1. The tail's cost is per entry, not per byte.**
H1: query time over an unsealed tail is proportional to the number of entries at fixed bytes. H0: it is proportional to bytes at fixed entry count.
Statistic: stock time for the empty-window shape on three 64 MiB tails with 64-, 130- and 512-byte bodies (about 300,000, 150,000 and 55,000 entries). Reject H0 if time per entry agrees within 20 % across the three while time per MiB differs by more than 2×. Supported but not yet tested as a ladder by run L-21 (4.7 µs per entry on two tails).
Cost: one generator run per tail, one hour. Decides: whether the unit of the tail's cost is the entry, which every later estimate depends on.

**A2. A field-level decode cuts the per-entry cost below half.**
H1: extracting only the key fields from an entry (times, node, index), without building the OTLP request and its strings, costs under 2 µs per entry. H0: a per-entry cost of 4 µs or more is inherent in protobuf framing.
Statistic: per-entry time of the index build (today 3.4 µs) with a hand-rolled field extractor against `prost` decode, on the real-text tail. Reject H0 if the extractor is below 2 µs per entry and the index it builds is byte-identical.
Cost: a scratch decoder of five fields, two days. Decides: how much of the tail's cost is the format and how much is the library; if H0 holds, the format is the whole cost.

**A3. A canonical block tail removes the per-entry decode.**
H1: a tail held as FOB1 blocks answers the empty-window and no-match shapes in under a fifth of the OTLP tail's time, because a block's keys are read from its columns at the view decoder's 72 ns per record plus 0.8 ns per body byte. H0: a block tail costs within 2× of the OTLP tail.
Statistic: time for the empty-window, `limit 50` and no-match text shapes over a 64 MiB tail stored as FOB1 blocks of one journal frame each, read through `decode_view`, against the stock OTLP tail; same records. Reject H0 if all three shapes are under one fifth of stock and the differential is clean.
Cost: a scratch journal reader over FOB1 frames and a converter from the existing tails, one week; the codec exists. Decides: whether ADR-0023 is a latency change as well as a bytes change, which is the strongest case for the wire decision.

**A4. The walk's no-stop overhead is order, not work.**
H1: decoding the selected entries in file order (one pass over the frames) when the first pass over the bounds shows the heap cannot fill brings the no-match text shape to at most the stock time. H0: the walk's extra cost is in the per-entry extract and stays whatever the order.
Statistic: no-match text and rate shapes on the real-text tail, walk with a file-order fallback against stock (today 1,246 against 721 ms). Reject H0 if the fallback is at or below stock on both shapes.
Cost: a scratch change in the walk, one day. Decides: whether the walk can be promoted without a worst case worse than stock.

## Group B: how much are the storage levers worth?

**B1. A hash per row group keeps custody and returns the bytes.**
H1: replacing the per-Batch SHA-256 with a hash per row group in the manifest reduces a real-text Segment by at least 15 % and a corrupted Batch is still refused on replay. H0: the saving is under 10 %, or a flipped byte in a Batch is not refused, or replay time rises by more than 10 %.
Statistic: Segment bytes before and after on the real-text and synthetic states; replay time of 64 Segments; a flipped-byte negative control. Reject H0 if all three hold.
Cost: a scratch sealer and reader, three days. Decides: ledger L-19.

**B2. The custody copy's excess is framing.**
H1: at least 60 % of the difference between a real-text Batch (77 bytes for two lines) and its body column (37 bytes) is OTLP framing and attributes repeated per record, so a custody encoding that writes them once is bijective and saves at least a quarter of the custody table. H0: the excess is under 40 % framing, or no bijective encoding reaches a quarter.
Statistic: a field census of the real-text Batches (bytes per OTLP field over 150,010 Batches); then a scratch codec's size and an exact round-trip fuzz. Reject H0 if the census shows 60 % and the codec round-trips every Batch byte for byte at 25 % saving.
Cost: the census is a script, one day; the codec a week. Decides: ledger L-20, or that the canonical record (A3) is the only way to one copy.

**B3. A real stream compresses better than the random draw.**
H1: real log lines in their original order (bursts, repeats, templates in sequence) compress at least 1.5 times better under Zstd 3 per 1 MiB block than the same lines drawn at random. H0: the gain is under 1.1×.
Statistic: bytes per line of the eight Loghub samples in file order against shuffled, in 1 MiB blocks. Reject H0 if the ratio is at least 1.5 on at least six of eight.
Cost: a script, one hour. Decides: whether every storage figure measured so far is pessimistic, and by how much.

**B4. Real gauges compress under the delta column.**
H1: metric points with real values (monotone counters, slowly moving gauges) cost under 3 bytes per point in the FOB1 and Parquet metric columns, against 9.5 for the generator's random integers. H0: they cost 6 or more.
Statistic: bytes per point on a journal whose points are replayed from a captured `/proc` series (the Spindle's own sampling over an hour) against the synthetic points. Reject H0 if under 3 on both encodings.
Cost: one capture run of the Spindle plus the measurement, half a day. Decides: ledger L-22, and the metrics share of every capacity figure.

**B5. Segment size does not change the query floor.**
H1: at the product's 64 MiB Segments, the narrow shapes (last 10 s, one node over 60 s, empty window) stay under 20 ms with 320 Segments retained (20 GiB), because the per-Segment fixed cost is 0.08 ms and row groups within a Segment are disjoint. H0: a narrow shape exceeds 50 ms at 320 Segments of 64 MiB.
Statistic: the retention-scale shapes on 320 Segments of 64 MiB real text, stock and walk. Reject H0 if all narrow shapes are under 20 ms.
Cost: 20 GiB of disk and about three hours of sealing; the generator exists. Decides: whether the Segment side needs any further work before the registered gates.

## Group C: what the query side still has to remove

**C1. The walk's overlap floor falls with row-group granularity.**
H1: on 64 MiB Segments whose row groups are disjoint within a Segment, the adversarial workload (every Segment overlapping every other) reads at most `limit / 8,192 + number of Segments` row groups for a `limit` query, against every row group today. H0: it still reads every row group.
Statistic: row groups read for `limit 50`, `1,000` and `10,000` on 16 adversarial Segments of 64 MiB. Reject H0 if reads are within 2× of the bound on all three.
Cost: generation and sealing of 1 GiB adversarial, one hour. Decides: whether the L-04 floor seen on 1 MiB Segments is an artefact of their single row group.

**C2. A boundary inside a source bounds the overlapping case.**
H1: letting the budget stop inside a source and name the heap's own threshold as the boundary bounds every page to within 2× the budget's rows on the overlapping state, where today a page costs the full scan (505 ms at a 2,000-row budget). H0: pages still exceed 10× the budget's row cost.
Statistic: slowest page for the five L-05 shapes on the overlapping state at a 2,000-row budget. Reject H0 if every page is within 2× of 2,000 rows' materialisation cost (about 1.2 ms) plus one source.
Cost: a change in the budget rule, two days, plus the oracle run. Decides: whether L-05's semantics survive the overlapping case.

**C3. Per-page cost is independent of the sources already passed.**
H1: a binary search on the sorted bounds makes a page's fixed cost independent of how many sources lie below its start (today it grows to about 90 ms by the 80th page on a 149,585-entry tail). H0: the per-page cost still grows with the page index.
Statistic: page time against page index for the `limit 10,000` drain on the real-text tail at a 2,000-row budget. Reject H0 if the slope is under 0.1 ms per 10,000 sources passed.
Cost: a scratch change, one day. Decides: whether budgeted drains cost their rows or their position.

**C4. Rates need the counter workload to be tested at all.**
H1: on a workload with monotonic counters, the budgeted rate over `[from, m)` equals the stock rate over the same window and the full-window rate from pages is reconstructible. H0: the rate's boundary semantics differ from the stock rate on the shorter window.
Statistic: the L-05 oracle's rate half on a journal replayed from a captured counter series (B4's capture). Reject H0 if 0 mismatches over 100 random rate queries.
Cost: the capture of B4 plus the L-05 run, half a day. Decides: the one half of L-05's oracle that run 01 could not exercise.

## Group D: is the sealing and segmentation design right?

Every run so far has measured inside the design of [ADR-0020](../decisions/ADR-0020-store-sealed-history-as-parquet-segments.md) and [ADR-0022](../decisions/ADR-0022-build-segments-by-external-merge-sort.md): a Segment per 64 MiB journal file, four Parquet tables, time-first sort, 8,192-row groups, a sealer thread that builds each Segment after the fact, retention by whole Segment. None of that is the oracle. These hypotheses are judged by the contract and the Python oracle over the journal, never by the stock server.

**D1. The Segment's unit should be time, not journal bytes.**
H1: Segments cut by a time span (every N minutes of receive time, bytes permitting) give every window query a contiguous Segment set and no cross-Segment overlap except from late arrivals, so the walk's floor on the overlapping workload falls below a tenth of today's reads. H0: cutting by time gains under 2× on reads, because late arrivals (outage drains) overlap whatever the cut, or costs more than 10 % in bytes from smaller files.
Statistic: row groups read and Segment bytes for the L-04 shapes on the adversarial and outage workloads sealed both ways. Oracle: the Python query oracle over the journal. Reject H0 if reads fall by 10× on the adversarial state and bytes grow under 10 %.

**D2. Cross-Segment merging (compaction) pays for itself.**
H1: merging four adjacent Segments into one globally sorted Segment (a second sealing level) removes their mutual overlap, so the walk reads a quarter of the row groups on overlapping workloads, at a write amplification under 2× over the retention window. H0: the query gain is under 2× or the amplification over 3×.
Statistic: reads for `limit` shapes before and after a four-way merge on the outage workload; bytes written per byte retained over a simulated day. Oracle: HIST-1/2 on the merged Segment against the journal. Reject H0 if both bounds hold.

**D3. The projections are not needed once the custody copy is a canonical block.**
H1: a Segment that holds FOB1 blocks once, with per-block key bounds in the manifest and no `logs.parquet` or `metrics.parquet`, answers every registered shape within 2× of today's projections through `decode_view` (0.8 ns per byte, 72 ns per record), including the text search that falsified L-06 against raw OTLP, at no more than 60 % of today's bytes. H0: some registered shape is over 2× slower, or bytes are over 70 %.
Statistic: the fourteen L-21 shapes on a real-text Segment state rebuilt as FOB1 blocks plus a manifest, against today's Parquet Segments; bytes of both. Oracle: the Python query oracle over the journal for every shape. Reject H0 if every shape is within 2× and bytes under 60 %. This is the hypothesis that would retire the sealer's projection build and the duplicate copy at once; it depends on A3.

**D4. 8,192 rows per group is too coarse for the selective shapes.**
H1: 1,024-row groups cut the rows materialised by node- and short-window shapes by at least 4× for under 5 % more bytes and under 2× more footer time. H0: the footer and page overhead costs over 5 % in bytes or the per-Segment fixed cost doubles.
Statistic: rows read and bytes for the attribution shapes at 1,024, 4,096 and 8,192 rows per group on the real-text state. Oracle: HIST-1/2 (identical rows and order); the Python oracle for answers. Reject H0 if 4× fewer rows and under 5 % bytes.

**D5. A second-level sort by node inside a time bucket helps node shapes without hurting time shapes.**
H1: sorting rows by (second, node, time) rather than (time, node) keeps row-group time statistics disjoint at the second and lets node-filtered shapes skip at least 4× more rows. H0: time shapes lose over 10 %, or the registered total order (time first) cannot be reconstructed for pages, which would be a contract change.
Statistic: rows read for node and time shapes under both orders; page tokens checked against the Python oracle's order. Oracle: the Python query oracle. Reject H0 if node shapes gain 4× and time shapes lose under 10 % and every page is in contract order. This hypothesis is the one most likely to fail on the contract rather than the figures.

**D6. ADR-0022's memory ceiling is a ceiling in bytes, not in records.**
H1: the external merge sort's 80 MiB ceiling holds on real text, where a 64 MiB file carries 2.7 times the records, because runs are cut by bytes as well as rows. H0: peak memory on a real-text file exceeds the ceiling or the build takes over 2× the synthetic time.
Statistic: peak RSS and build time of the sealer-study harness's merge-sort candidate on the real-text journal against the synthetic one. Oracle: HIST-1/2 on the output. Reject H0 if peak is under 80 MiB and time under 2×. This is a design check the sealer study never ran; it decides whether the accepted design was sized on the wrong workload.

**D7. The sealer should re-block, not re-encode.**
H1: if the node sends FOB1 blocks per Batch (two to thirty-four records), re-blocking them at seal time into per-file blocks recovers the 20 % the per-Batch layout loses (94 against 79 bytes per record) in under a third of the time of today's projection build. H0: re-blocking costs as much as building the projections.
Statistic: bytes per record and sealing time for re-blocking against the projection build on the real-text journal converted to FOB1. Oracle: HIST-1/2 equivalent (every record of the input present once, in key order), checked by decoding both. Reject H0 if 20 % recovered in under a third of the time. Depends on A3.

**D8. Whole-Segment retention is the right granularity.**
H1: deleting whole Segments by age overshoots the configured retention by under one Segment's span (about 20 minutes of real text at 10,000 nodes) and never under-retains. H0: at the product's 64 MiB Segments and real-text rates the overshoot exceeds an hour, or a Segment that straddles the cut is deleted early.
Statistic: retained span against configured span over a simulated day at three fleet sizes, from the retention kernel's own decisions. Oracle: the retention contract (HIST-4) and its kernel properties. Reject H0 if overshoot is under one Segment and under-retention never occurs.

**D9. The open-time checks catch the corruptions that matter.**
H1: the Segment reader's open checks (size, schema, row count against the manifest) refuse every fault-injected truncation and byte flip in a footer, and the sha256 in `verify` is needed only for flips in data pages. H0: a torn footer or a flipped page passes the open checks and answers wrongly.
Statistic: fault injection over 1,000 Segment mutations (truncate, flip in footer, flip in page, flip in manifest). Oracle: the contract's refusal rule and the Python oracle for any answer the reader gives. Reject H0 if every corruption is refused or answered identically to the oracle.

## Ranking

By information value per hour, with the dependency each unlocks:

1. **B3** (one hour): tells whether every storage figure is pessimistic before any storage change is built.
2. **A1** (one hour): fixes the unit of the tail's cost, on which A2 to A4 and the capacity model rest.
3. **D6** (half a day; the harness exists): the accepted sealer design was sized on synthetic records, and real text carries 2.7 times as many; this decides whether ADR-0022 needs revision before implementation.
4. **A4** (one day): decides whether the walk can be promoted; cheap, and its answer stands whatever A2 and A3 find.
5. **D4** (one day): the cheapest segmentation change, and the one most likely to be worth taking regardless of the others.
6. **B1** (three days): the largest measured removable cost with the smallest change; its negative control is the whole risk.
7. **A2**, then **A3**, then **D3** and **D7**: the three readings of the tail's cost and the design that would follow from the third; D3 is the hypothesis that would retire the projection build and the duplicate copy together, and the most expensive to test.
8. **D1**, **D2**, **D5**: the segmentation alternatives, each judged by the Python oracle because each changes what a Segment is; D5 may fail on the contract rather than on the figures.
9. **B5**, **C1**, **D8**, **D9**: the Segment side at product size and the retention and durability checks no run has made.
10. **B2**, **B4**, **C2**, **C3**, **C4**: each decided by the ones above or waiting on a capture.

One experiment runs at a time. B3 is next.
