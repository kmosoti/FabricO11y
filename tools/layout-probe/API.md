# S3/S4 hybrid columnar projection contract

Status: registered before implementation. This separate research crate depends on
storage-probe's event codec and coverage digest. It does not change FOL2 or S2.
Use Arrow/Parquet 60.0.0; keep those dependencies in this package only.

```rust
use std::{io, num::NonZeroUsize};
use fabric_o11y::Event;
use storage_probe::{Query, coverage::Digest};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Codec { Plain, Zstd }
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableAnchor { pub sha256: Digest, pub rows: usize, pub version: u32 }
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hit { pub position: usize, pub digest: Digest }
pub fn encode(rows: &[Event], row_group: NonZeroUsize, codec: Codec)
    -> io::Result<(Vec<u8>, TableAnchor)>;
pub fn decode(bytes: &[u8], expected: &TableAnchor) -> io::Result<Vec<Event>>;
pub fn query_full(bytes: &[u8], expected: &TableAnchor, query: &Query)
    -> io::Result<Vec<Hit>>;
pub fn query_projected(bytes: &[u8], expected: &TableAnchor, query: &Query)
    -> io::Result<Vec<Hit>>;
// Only indexed tokens are accelerated; all other queries use the full projection.
pub fn build_postings(rows: &[Event], table: &TableAnchor, tokens: &[String])
    -> io::Result<(Vec<u8>, Digest)>;
pub fn query_postings(bytes: &[u8], table: &TableAnchor, query: &Query,
                      index: Option<(&[u8], Digest)>) -> io::Result<Vec<Hit>>;
```

Use these exact fields in one Arrow RecordBatch schema, in this order:
`event_time` Int64 non-null; `tenant` UInt64 non-null;
`payload_kind` UInt8 non-null (0=Log, 1=Gauge);
`log_body` Utf8 nullable (Some including empty for Log, null for Gauge);
`raw_event` Binary non-null (S2 version-1 single-event JSON envelope);
`row_digest` FixedSizeBinary(32) non-null (E1 full single-event digest).
Parquet metadata includes `fabric.layout.version=1`. Row-group limit is caller
supplied. Codec Plain uses uncompressed pages; Zstd uses level 3. Dictionary and
other writer properties remain fixed defaults and are recorded with results.

This is a hybrid layout: predicate columns plus a complete raw-event column for
late materialization. It duplicates some fields; include that overhead in costs.
It is not a fully normalized columnar schema. Arrival order and identical duplicate
Events remain distinct. No ID-based sorting/deduplication. Every scalar variant,
attribute order/duplicate, exact string, i64/u64 boundary and float bit survives.
Empty rows produce a valid table; reject wrong version, count, hash, schema, nulls,
invalid payload kind, raw-event count or decoded projection/raw inconsistency.
Decoder verifies each raw row's digest and predicate fields as well as the file hash.

The table anchor is produced only by the trusted encoder and retained independently.
All query/decode paths verify the SHA-256 of the whole file and expected version/count
before producing results. This costs a full byte pass even for projected queries;
report it. The input slice is immutable during each operation. A projected query
reads only predicate and digest columns through Parquet's projection API after
file authentication. It must not decode raw_event. Full query materializes all rows
and uses an exact scalar predicate. Predicates match S1: inclusive event time,
optional exact tenant and case-sensitive Unicode-whitespace token in Log body only.
Wrong/missing raw table cannot be masked by a postings hit or an empty query.

Postings encode version=1, full TableAnchor, registered token list and sorted unique
physical positions for each token. Use deterministic JSON; duplicate requested token
names are normalized, and only exact registered tokens are represented. Builder
requires row count match. The builder is trusted to receive the same rows used by
encode; it cannot establish file equivalence from TableAnchor alone. The caller
retains a SHA-256 of the entire index independently. A valid index narrows projected
candidates only for a registered queried token; other predicates remain exact.
Missing index, wrong digest/version/table binding, malformed positions, wrong row
count or malformed JSON falls back to query_projected. No invalid optional index
may produce an empty answer. Rebuild derives a fresh index from retained source.

Trusted-builder and collision-resistance assumptions are the same as E1. A malicious
builder or caller supplying a newly computed hash for edited bytes is outside the
claim. This experiment adds no durable publication API. The benchmark uses the same
fresh-file write/sync/parent-sync boundary for JSON and Parquet, preserves files on
failure, then reads the retained source independently for exact comparison.

Postings JSON schema (fixed before candidate implementation):
`{"version":1,"table":{"sha256":[32 u8 values],"rows":N,"version":1},"tokens":[sorted unique strings],"postings":{"token":[sorted unique usize positions]}}`.
The map has exactly one entry for every registered token, including absent tokens
with empty lists. Reject duplicate JSON keys, unknown fields, missing fields,
noncanonical token ordering, extra/missing map entries and out-of-range/duplicate
positions by falling back to projection. Builder rejects unsupported table version
as well as row-count mismatch; the file hash cannot be checked by this builder API.
