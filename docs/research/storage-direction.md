# Storage direction: one record spine for logs, metrics and traces

Status: **exploratory research**, tier 6 of the [source-of-truth order](../README.md#source-of-truth). This page answers one question: what layout and algorithm would store logs, metrics and traces well together, and how far is Fabric from it. It rests on what the code does today, on [storage layout run 01](../experiments/benchmarks/storage-layout-run-01.md) (bytes measured on real Segments, synthetic and real text), and on a sourced [prior-art survey](storage-prior-art.md). It changes nothing. Two boundaries hold throughout: the [product contract](../PRODUCT-CONTRACT.md) names traces a non-goal and allows no index beyond Parquet row-group statistics unless a registered gate fails; and the wire format, the journal and the Segment layout are persisted formats that need explicit scope and an ADR to change ([ADR-0020](../decisions/ADR-0020-store-sealed-history-as-parquet-segments.md), [scope rule](../../AGENTS.md#scope-rule)). What follows is a design to hold and a set of [ledger](ledger.md) entries, ranked by what an experiment would teach, not a plan to build.

## Verdict

Fabric already has the spine that the surveyed systems converge on, and lacks three things that the measurements make visible.

What it has. One custody record per Batch (the node's exact bytes, hashed, in the journal and again in `batches.parquet`); derived per-kind tables sorted by one total key `(time, node_id, sequence, index)`; row-group statistics on time; one Segment lifecycle for every kind; answers that say what they covered. That is the "one engine, per-kind tables" family (ClickHouse-based stores, Elastic, the Parquet-on-object-store systems) with a stronger custody story than any of them, and it is the right family: every "one row model for everything" system in the survey still keeps metrics apart, because dense numeric series want delta and XOR encodings that a sparse event layout cannot give them.

What it lacks, by measured weight:

1. **The payload is stored twice, and that is most of a Segment.** The raw Batch bytes and the `body` column are the same text; together they are 92 % of a synthetic Segment and 63 % of a real-text one. On real text the raw copy costs twice the projection, because every Batch carries OTLP framing and the same five file attributes with every line.
2. **A fixed 32-byte hash per Batch** is 3.6 % of a synthetic Segment and 17.6 % of a real-text one, where Batches are small. The manifest already hashes every file.
3. **The unsealed tail is the most expensive place a record can be**: uncompressed protobuf at 2.5 times the Segment's bytes on real text, decoded whole on every query until [L-03](ledger.md#l-03-a-memtable-of-keys-for-the-unsealed-tail) and [L-04](ledger.md#l-04-the-threshold-algorithm-over-source-bounds) land, and bounded only by `journal_bytes`.

Everything else the survey recommends is either present (time-first sort with statistics, Zstd blocks, dictionary encoding of nodes and names), small (delta encoding takes 16 to 29 % off a metrics table that is 3 to 13 % of a Segment), or outside the contract (traces, typed attribute columns, template extraction). The synthetic workload hid all three findings, because its lines are half repeated bytes and half random and compress the same under any encoding; the real-text state is the one to measure against from now on.

## What exists

| Element | Today | Source |
| --- | --- | --- |
| Wire unit | `Batch` v1: `node_id` (16 B), `generation`, `sequence`, `metrics` bytes (OTLP), `logs` bytes (OTLP), `cursors`, `collection_gaps`; 1 MiB cap; tag numbers are the persisted format; no traces slot and `opentelemetry-proto` built without the `trace` feature | [envelope.rs](../../crates/fabric-frame/src/envelope.rs) |
| Journal record | `Entry { label, batch bytes, received_unix_nano }` in a `Group` per 1 MiB or 50 ms; FAB1 frames with CRCs and a commit marker; files of 64 MiB | [store.rs](../../crates/fabric-server/src/store.rs), [frame.rs](../../crates/fabric-frame/src/frame.rs) |
| Segment | `batches.parquet` (group, label, received_ns, sha256, batch bytes; journal order), `logs.parquet` (key, observed_ns, body, attributes JSON), `metrics.parquet` (key, name, unit, sum, monotonic, time_ns, start_ns, value_int, value_double, attributes JSON), `gaps.parquet`; Zstd 3; 8,192-row groups; sorted by `(time, node_id, sequence, index)`; manifest v1 with group range, receive bounds, per-node freshness, file hashes | [segment.rs](../../crates/fabric-server/src/segment.rs) |
| Attributes | string-valued OTLP attributes only, as one JSON string per row; resource attributes, scope, severity, trace and span ids are not projected | [rows.rs](../../crates/fabric-server/src/rows.rs) |
| Replay | reads `batches.parquet` only and checks each row's hash | store.rs `scan_batches` |
| Retention | whole Segments by age or total Parquet bytes | [retention.rs](../../crates/fabric-core/src/retention.rs) |

The projections are derived and must answer identically to the journal (HIST-1 and HIST-2, permanent). The custody copy is what makes a Segment self-sufficient for replay after the journal file is reclaimed.

## What the bytes say

From [storage layout run 01](../experiments/benchmarks/storage-layout-run-01.md), 64 Segments of 1 MiB each:

| | synthetic | real text |
| --- | ---: | ---: |
| Journal in, Segments out | 64.2 MiB, 47.3 MiB | 64.1 MiB, 26.0 MiB |
| raw Batch bytes | 48 % of the Segment, 2.8:1 | 43 %, 5.4:1 |
| `body` column | 44 %, 2.6:1 against the raw text, 196 B per line | 20 %, 7:1, 18.5 B per line |
| per-Batch `sha256` | 3.6 % | 17.6 % |
| all key and time columns of all tables | about 3 B per row | about 3 B per row |
| metrics table | 2.8 % | 12.6 % |
| delta encoding on integer columns | metrics −16 % | logs −12 %, metrics −16 % |
| series-first order | no change on logs; metrics −3 to −8 % | no change |
| Zstd 9 | no change | logs −14 % |
| a line compressed alone, with and without a dictionary | | 38 to 164 B against 5 to 27 B in a block |

Two numbers decide the design. The duplicate payload is the Segment; and block compression already finds most of what template extraction finds (a crude template split gained 0 to 15 % over the plain column on eight real log types; CLP reports twice gzip with its full scheme, which is mostly about searching compressed data).

## What others do

The [survey](storage-prior-art.md) lists fifteen facts that transfer. The ones that bear on Fabric:

- **Per-kind tables under one engine, one lifecycle** is the mainstream (SigNoz, ClickStack, Jaeger on ClickHouse, Elastic, InfluxDB 3, OpenObserve, Parseable, GreptimeDB). OTel Arrow defines the same thing at the wire: a main batch per signal plus attribute tables keyed by parent id, dictionary-encoded, "designed for compatibility with Parquet".
- **Metrics stay separate even where everything else is a wide event** (Honeycomb's metrics datasets; the cost critique's 33,000 against 0.80 dollars a month for the same signal as events and as metrics). Dense series want delta-of-delta and XOR: 1.37 B per point in Gorilla, 1 to 2 B per sample in Prometheus.
- **Sort by a low-cardinality grouping key before time** is reported to matter more than codecs (SigNoz resource fingerprint, Elastic `_tsid` and `host.name`, InfluxDB least-cardinality-first, Husky, Jaeger). Fabric sorts time-first because its registered query key is time-first and its row-group pruning and the threshold walk of L-04 depend on disjoint time ranges per row group; a node-first order would trade that for compression that the measurement shows is nil on logs and 3 to 8 % on metrics. Not worth it at Fabric's query shapes.
- **Cross-kind joins are plain columns**: `trace_id` and `span_id` on logs and spans in every schema surveyed; exemplars with `trace_id` on metric points. Trace-id-first sort only where the workload is "fetch one trace" (Tempo); search-first systems sort by service and keep a bloom filter on `trace_id`.
- **Promote the few hot attributes to typed columns, keep the rest in a typed map with an overflow** (Tempo's dedicated columns after generic attribute columns reached 70 % of block size; ClickHouse JSON subcolumns; Parquet Variant shredding).
- **Nested resource > scope > record storage** avoids repeating resource attributes per record; the raw Batch pays that repetition today, which is why the raw copy costs twice the body on real text.

## The design: one spine, per-kind projections

Hold this shape; build only the parts that a gate or a measured cost justifies.

**The record** ([ADR-0023](../decisions/ADR-0023-define-an-observation-record-with-a-canonical-encoding.md), proposed; built and measured in [fabric-observation](../../crates/fabric-observation/src/lib.rs) and [observation encoding run 01](../experiments/benchmarks/observation-encoding-run-01.md)). Every observation, of any kind, is `(strand, sequence, index, kind, node time, receive time, locators, identity, attributes, payload)`: strand is `(node_id, generation)`; node time is `observed_ns` for a line, `time_ns` (with `start_ns`) for a point, start time (with end time) for a span; receive time comes from the Group; locators are nullable `trace_id`, `span_id`, `parent_span_id`, present on every kind so that a line, a point's exemplar and a span join by equality; identity is the metric name, the span name and status, the line's severity and event name; attributes are a typed map with a small fixed set promoted to columns; the payload is the body, the value, or the span's events and links. The total order stays `(node time, node_id, sequence, index)`, the key the kernel already proves properties about.

**The Segment.** One custody table of raw bytes, stored once; one projection per kind, sorted by the key, with time statistics per row group; one manifest with per-table key bounds and per-node presence ([L-07](ledger.md#l-07-manifest-facts-for-sound-two-sided-skipping)); one lifecycle. A traces projection, if the contract ever admits traces, is a third table of the same shape plus a bloom filter on `trace_id` per row group, and the logs and metrics projections gain their locator columns. The wire `Batch` would need a traces payload slot: a wire change, in its own scope and ADR.

**The payload, once.** The duplicate copy is the one structural cost. Four ways to remove it, with what the measurements say about each:

| Option | What changes | Measured consequence | Verdict |
| --- | --- | --- | --- |
| A. Keep both copies | nothing | 92 % synthetic, 63 % real text, of the Segment is payload stored twice | today |
| B. Drop the raw copy; custody by hash only | replay from Segments impossible; the projections cannot reproduce the node's exact bytes | breaks the replay design and HIST-1/2 | rejected |
| C. Drop `body` from the projection; answer from the raw copy through a key sidecar ([L-06](ledger.md#l-06-raw-bytes-plus-a-key-sidecar-no-projections)) | the logs table shrinks to 2 to 14 % of itself | text search pays the decode: 3.2 ms per MiB of raw bytes, about 200 ms per 64 MiB Segment | falsified for text search; open for selective shapes now that L-03 and L-04 bound the selection |
| D. Keep the body projection; shrink the custody table | hash per row group or per file instead of per Batch; later, a custody encoding that stores the OTLP framing and repeated attributes once per Batch and the lines once | −6 % synthetic, −28 % real text from the hash alone; the framing share is unmeasured | **the candidate**: no query path changes, replay keeps a hash to check, needs an ADR-0020 amendment |

**Encodings.** Delta encoding on the integer columns of the metrics table (reader-transparent in Parquet; still a persisted-format note). Nothing for logs: block Zstd already does what templates would, within 25 % of a whole-file stream. Zstd 9 for the body column is a 14 % saving on real text against sealing CPU on the two-CPU profile; a measurement for the registered soak, not a decision here.

**The tail.** The journal tail is uncompressed protobuf, 2.5 times the Segment's bytes on real text, and the one place where a record costs time proportional to everything around it. The [bounded sealer](../milestones/bounded-sealer.md) keeps it to one file when sealing keeps up; L-03 and L-04 make the queries over it proportional to their answers. Both come before any storage change, because they bound the cost that the storage format cannot.

## What not to do

- Not a single wide-event table for all kinds: the metrics projection is 3 to 13 % of a Segment and wants encodings a sparse row cannot carry; every surveyed system that tried kept metrics apart.
- Not a node-first or series-first sort: it breaks the disjoint-row-group property that pruning and L-04 depend on, for a compression gain measured at zero on logs.
- Not a log-template engine (CLP-style) now: the measured gain over block compression is 0 to 15 % at Fabric's row-group sizes; the known gain of the full scheme is in searching compressed archives at petabyte scale, not in bytes at 20 GiB of retention.
- Not a trace store: out of contract; the design above says where it would go so that nothing built now forecloses it.
- Not typed attribute columns yet: the Spindle emits five file attributes and one metric attribute, all strings; the JSON blob costs 0.0 MiB per 64 Segments. The OTAP attribute-table shape is the one to adopt when a consumer sends real attribute sets.

## Ledger entries

New entries, ranked by information value (what the cheapest experiment would decide):

1. **L-19. Hash per row group instead of per Batch** (option D, first step). Measured 6 to 28 % of a Segment. The experiment: a scratch sealer writes the custody table without `sha256` and a per-row-group hash in the manifest; replay verifies per row group; HIST-1/2 differential; replay time compared. It decides whether the custody table can lose its per-row check without losing custody.
2. **L-20. Custody encoding that stores framing once** (option D, second step). Unmeasured share; the real-text state says the raw copy is twice the body. The experiment: measure the OTLP framing and repeated-attribute share of real Batches; if it is most of the difference, a custody table of `(header once, lines)` that reproduces the exact bytes is a codec, pure, adapter-support by ownership.
3. **L-21. Real-corpus query run.** Every query figure so far is on synthetic lines; text-search selectivity, body decode and the Segment-level costs may differ on real text. One run of the attribution shapes on the real-text state, stock server. It calibrates L-06's option C.
4. **L-22. Delta encodings for the metrics projection.** Measured 16 to 29 % of 3 to 13 %. A writer-property change and an ADR note; low value, low cost; wait for a consumer whose metrics volume makes it matter.
5. **L-23. Traces as a third projection.** Design held above; nothing to run until the contract changes.

L-06 is updated with the real-text numbers: the duplicate is 63 % of a real-text Segment, not 45 %, and the raw copy is the larger half. L-18 gains the tail's byte ratio.

## What runs next

One experiment at a time, per the [charter](frontier-map.md). After L-05, the storage question with the highest information value per hour is L-21, because every storage decision above and the open half of L-06 depend on how real text behaves in the query path, and the state already exists. L-19 follows if L-21 does not change the picture.
