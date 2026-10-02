# ADR-0023: Define an Observation record with a canonical block encoding

## Status

Proposed on 2026-10-02. The record type and its codec exist as the crate [fabric-observation](../../crates/fabric-observation/src/lib.rs) in the adapter-support layer, with unit tests, negative controls, property tests and a fuzz target. Nothing in the product emits, sends, stores or reads it. Adopting it on the wire or in a Segment is a wire-format and persisted-format change under the [scope rule](../../AGENTS.md#scope-rule) and [ADR-0020](ADR-0020-store-sealed-history-as-parquet-segments.md), and a [product contract](../PRODUCT-CONTRACT.md) change where it would add a signal; each would be its own decision and its own commit. This ADR records the type, the encoding's guarantees and the reasons, so that the decision to adopt it can be taken against evidence.

## Context

Fabric stores one custody record per Batch, the node's exact OTLP bytes, and derives per-kind projections from it. [Storage layout run 01](../experiments/benchmarks/storage-layout-run-01.md) measured that the payload stored twice (raw bytes and the body column) is 92 % of a synthetic Segment and 63 % of a real-text one, that the raw copy costs twice the projection on real text because OTLP repeats framing and attributes with every record, and that the per-Batch hash is 17.6 % of a real-text Segment. The [storage direction](../research/storage-direction.md) found that the layout the surveyed systems converge on, per-kind tables sorted by time under one lifecycle, is the one Fabric has, and that the missing piece is one record model that all three signals share, so that a line, a point and a span carry the same key, the same locators and the same attribute typing.

OTLP cannot be that record. Its protobuf encoding is not canonical (field order, varint lengths and unknown fields are encoder freedoms), so a hash of the bytes stands for the bytes, not for the records; two encoders can send the same observation as different bytes, and the server can only keep the bytes it received. That is why the custody copy and the projections cannot be the same object today.

## Decision

Define **Observation**, one record for a log line, a metric point or a span:

- key: `time_ns` (the node's time: observed, point, or span start), `node_id`, `generation`, `sequence`, `index`, the order the query kernel already proves properties about;
- locators: optional `trace_id`, `span_id`, `parent_span_id`, the same on every kind;
- attributes: typed (`Str`, `Int`, `Double`, `Bool`, `Bytes`), strictly sorted by key;
- signal: `Log { severity, event, body }`, `Point { name, unit, Gauge | Sum { monotonic, start_ns }, Int | Double }`, or `Span { name, end_ns, status, kind }`.

Define **FOB1**, its block encoding: magic and version; a node-id dictionary and a string dictionary, each in first-use order; six columns (times as delta-of-delta, strands and positions as deltas, one tag byte per record, locators, attributes, payloads), each varint- or dictionary-coded; a CRC-32 trailer. The encoding is **canonical**: every valid block has exactly one byte string, and the decoder accepts only that byte string, rejecting overlong varints, dictionaries with duplicate or unused entries or out of first-use order, unsorted or duplicate attribute keys, NaN, reserved bits, trailing bytes and CRC mismatches. Hence `decode(encode(b)) == b` for every valid block and `encode(decode(x)) == x` for every accepted byte string, which the property tests state and the fuzz target checks on every input.

The crate is pure (no I/O, no clock, `forbid(unsafe_code)`, the core's panic and arithmetic lints), has one dependency (`crc32fast`, already in the frame log), and belongs to adapter support because it is a codec, not a domain decision ([ADR-0016](ADR-0016-keep-a-pure-semantic-core.md): purity does not decide ownership).

## Consequences

- A hash of an FOB1 block is a hash of its records. If the node emitted FOB1, the server's custody copy and the Segment's custody table would be the records themselves, once, and a projection would be a view rather than a second copy. [Observation encoding run 01](../experiments/benchmarks/observation-encoding-run-01.md) measured 14.5 bytes per record under Zstd against 18.2 for raw Batches on real text (the same within 1 % on the synthetic workload), about half of today's custody table, and under a microsecond per record to encode.
- Traces fit without a second model: a span is an Observation with its locators set; a line or a point joins it by equality on the same columns. Admitting traces remains a contract change.
- The decoder's canonicality checks make a block a thing that can be verified as well as read; they also mean an FOB1 reader refuses bytes a lenient protobuf reader would accept, by design.
- Nothing changes until a later decision wires the type into the Spindle's Batch (a new payload slot or a new envelope version), the journal, or a Segment table. Each of those needs its own ADR, scope and compatibility story (version-1 readers keep reading version-1 bytes).
- The mutation property earned its place on its first full run: it found a block with two equal dictionary entries that the decoder accepted and the encoder merged ([CX-FOB1-DUPLICATE-DICTIONARY](../formal/counterexamples.json)); the decoder now refuses repeated entries.
- Verification gates: the crate's tests run under the workspace `test` check; the fuzz target's corpus replay runs under `fuzz-corpus`; its Kani harnesses compile under `cfg(kani)` but are not in the registered `kani-core` check until that policy is extended in its own commit.

## Alternatives considered

- **Keep OTLP as the record and canonicalise it** (re-encode with a fixed field order): the result is still not a function of the observation (unknown fields, repeated attribute order) and the custody hash would be of the server's re-encoding, not the node's bytes.
- **OpenTelemetry Arrow**: the closest prior art (a batch per signal plus attribute tables keyed by parent id, dictionary-encoded); it is a transport layout over Arrow IPC, not a canonical encoding with a one-to-one guarantee, and would bring Arrow onto the node.
- **Parquet as the wire**: row groups, footers and statistics are too heavy for a one-second Batch and Parquet writers are not canonical either.
