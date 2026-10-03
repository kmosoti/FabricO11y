# Phase-4 retained-history query oracle

`query_oracle.py` is an implementation-independent exact-scan checker for
the query contract in
[retained-history.md](../../docs/architecture/retained-history.md). It
decodes the Fabric `Batch` protobuf itself with a minimal hand-written wire
decoder (varint / fixed64 / length-delimited / fixed32, unknown fields
ignored) and never imports code from this repository. Nothing under
`crates/fabric-server` or any query/segment implementation exists yet or was
read to write this tool.

## Field numbers (verified, no disagreement found)

Checked against the vendored `opentelemetry-proto-0.33.0` crate sources and
`crates/fabric-frame/src/envelope.rs`. Every number matches the task's list exactly:
`Batch` 1..8 as given; `ExportLogsServiceRequest.resource_logs=1`,
`ResourceLogs.scope_logs=2`, `ScopeLogs.log_records=2`, `LogRecord`
`time_unix_nano=1`, `observed_time_unix_nano=11`, `body=5`, `attributes=6`;
`AnyValue`/`KeyValue` as given; `ExportMetricsServiceRequest.resource_metrics=1`,
`ResourceMetrics.scope_metrics=2`, `ScopeMetrics.metrics=2`, `Metric.name=1`,
`unit=3`, `gauge` oneof tag `5`, `sum` oneof tag `7`; `Gauge.data_points=1`;
`Sum.data_points=1, aggregation_temporality=2, is_monotonic=3`;
`NumberDataPoint.start_time_unix_nano=2, time_unix_nano=3, as_double=4,
as_int=6, attributes=7`.

Traces (added with [ADR-0025](../../docs/decisions/ADR-0025-carry-traces-as-a-third-signal.md), from
`opentelemetry.proto.trace.v1.rs` and `opentelemetry.proto.collector.trace.v1.rs` of the same crate):
`Batch.traces=9`; `ExportTraceServiceRequest.resource_spans=1`, `ResourceSpans.scope_spans=2`,
`ScopeSpans.spans=2`; `Span.trace_id=1`, `span_id=2`, `parent_span_id=4`, `name=5`,
`kind=6` (enum), `start_time_unix_nano=7` and `end_time_unix_nano=8` (fixed64),
`attributes=9`, `status=15`; `Status.code=3`. A span row carries `node`, `node_id`,
`sequence`, `index` (position among the Batch's spans), `trace_id`, `span_id` and
`parent_span_id` as lowercase hex of the bytes (empty when absent), `name`, `kind` and
`status` as the OTLP integers (an absent `Status` is 0), `start_ns`, `end_ns` and string
`attributes`. A `spans` query (`node?`, `from_ns`, `to_ns`, `trace_id?`, `name?`,
`limit`, `page?`) selects `start_ns` in `[from_ns, to_ns)` with exact `trace_id` and
`name`, ordered by `start_ns`, `node_id`, `sequence`, `index`, and is paginated like
`logs`. Freshness counts span start times.

## CLI

```
python3 -B tools/qualification/query_oracle.py --records R --query Q --answer A [--unavailable U]
```

`R` is a JSONL file (one record object per line), `Q`/`A`/`U` are JSON
files. Prints one JSON verdict and exits 0 pass / 1 fail / 2 malformed
input:

```json
{"passed": true, "violations": [], "expected_rows": 5, "answered_rows": 5}
```

`expected_rows`/`answered_rows` are row **counts** (not the row lists) —
chosen as the narrowest reading of "one JSON verdict"; the full expected/
actual rows are recoverable by calling `expected()`/`check()` directly.
Also importable: `expected(records, query, unavailable=None)` and
`check(records, query, pages, unavailable=None)`, taking the same
JSON-shaped Python objects the CLI reads from files (not paths).

Following the same strict-schema convention as
[DELIVERY_ORACLE.md](DELIVERY_ORACLE.md): **duplicate JSON keys, unknown
object fields, wrong fixed types, missing required fields, and
`NaN`/`Infinity`/`-Infinity` are all malformed input (exit 2)** in every
input file, not just the answer.

## Answer JSON format (the adapter contract)

`--answer` is a JSON array of **pages**; each page is the full envelope
plus that page's `rows`. Envelope fields repeat identically on every page
(`snapshot` and `next_page` are the only fields allowed to vary across
pages of one answer; `next_page` is a string or `null` for `logs`/`metrics`
and must be `null`/absent for `rate`, which is always exactly one page):

```json
{"complete": bool, "retained_from_ns": int, "retained_to_ns": int,
 "freshness": {"<node>": int}, "gaps": [{"node","sequence","receive_ns","gap"}],
 "unavailable": [ ... opaque, see rule UNAVAILABLE-SHAPE ... ],
 "snapshot": str, "next_page": str|null, "rows": [ ... ]}
```

Row shapes (field names copied from the contract):

- `logs`: `node, node_id, sequence, index, observed_ns, body, attributes`.
- `metrics`: `node, node_id, sequence, index, name, unit, kind, time_ns,
  start_ns, value, attributes`, plus `monotonic` **iff** `kind=="sum"`
  (absent for `"gauge"` — a presence violation is `MALFORMED`, since the
  contract fixes this shape per kind).
- `rate`: `node, name, attributes, time_ns, reset, rate` (`rate` is `null`
  iff `reset`).

## `--unavailable` format (minimal, oracle-defined)

A JSON list; each item is **either** `{"node": str, "sequence": int}`
(exactly that one record is unavailable) **or** `{"from_ns": int, "to_ns":
int}` (every record with `received_ns` in that half-open range is
unavailable). This is a simplification: the contract does not fix a
correspondence between a real segment and a journal record, so the oracle
models "a corrupt segment" as its constituent records becoming excluded,
identified the only two ways the fixture format can name them.

## Rule IDs

| ID | Statement |
| --- | --- |
| `ROW-DROPPED` | An expected row is missing from the concatenated answer. |
| `ROW-DUPLICATED` | A row's identity `(node_id, sequence, index)` (or, for `rate`, `(node, attributes, time_ns)`) appears more times than expected, anywhere across pages. |
| `ROW-ORDER` | Same row identities present, in the wrong order. |
| `ROW-CONTENT` | Same identity and order, a field differs (includes value-type: an int reported as a float, or vice versa, fails here — doubles compared bit-exact via `struct.pack(">d", ...)`). |
| `PAGE-LIMIT` | A page has more rows than `query.limit`, or a `rate` answer is not exactly one page. |
| `PAGE-NEXT` | `next_page` is non-null on the last page, or null on a non-last page. |
| `PAGE-SNAPSHOT` | Pages of one answer carry different `snapshot` tokens. |
| `ENVELOPE-CONSISTENT` | `complete`/`retained_*`/`freshness`/`gaps`/`unavailable` differ between pages of one answer. |
| `RETAINED-WINDOW` | `retained_from_ns`/`retained_to_ns` != min/max `received_ns` of available records. |
| `FRESHNESS` | A node's freshness != the newest `observed_ns`/`time_ns` among that node's available records. |
| `GAPS` | The gap set (order-independent) != available records' gaps of queried nodes with `receive_ns` in range. |
| `COMPLETE` | `complete` != (no unavailable record could hold a matching row). |
| `UNAVAILABLE-SHAPE` | `unavailable` non-emptiness != `!complete`. |
| `RATE-VALUE` | A non-reset row's `rate` is outside `rel_tol=1e-9` of `(v2-v1)/((t2-t1)/1e9)`. |
| `RATE-RESET` | A row's `reset` boolean doesn't match the contract's reset condition. |
| `MALFORMED` | Any structural/schema problem (see above); always exit 2. |

## Ambiguity decisions (narrowest reading chosen)

1. **Non-string log body.** Contract says "body (the string body)"; if
   `AnyValue` isn't `string_value` (or is absent), body is `""`.
2. **Non-string attributes.** Only `string_value`-valued `KeyValue`s become
   map entries; others are dropped (contract says "string attributes").
3. **Missing `NumberDataPoint` value oneof.** Defaults to integer `0`
   (should not occur in valid OTLP).
4. **Non-gauge/non-sum metrics** (histogram, exponential histogram,
   summary) produce **no** metric-point rows; the contract only defines
   fields for `gauge`/`sum`.
5. **`query.node` matches the row's `node` (label)**, not `node_id`, since
   that is the field literally named `node` in every row.
6. **Two consecutive rate points with equal `time_ns`** (`dt<=0`): treated
   as a reset (undefined otherwise; avoids division by zero).
7. **Empty available-record set**: `retained_from_ns`/`retained_to_ns` are
   both `0` (undefined by the contract).
8. **Rate pairing is restricted to points already in `[from_ns,to_ns)`**:
   the first in-range point of a series never produces an output row (no
   in-range predecessor). An alternative reading — pairing with a
   predecessor outside the range — is *not* implemented.
9. **`freshness`/`retained_from_ns`/`retained_to_ns` are global** (over all
   available records), not restricted to the queried node(s); `gaps` *is*
   restricted to "queried nodes" per the contract's explicit wording.
10. **`complete`'s "could hold matching rows"** is decided using the
    excluded record's real (fixture-known) decoded content — valid only
    because the oracle, unlike a real server, is told the ground truth of
    what an unavailable segment would have contained.

## Open questions (left to a real adapter/reviewer)

- Whether `expected_rows`/`answered_rows` should instead be full row lists.
- Whether `query.page` (resuming from a token) is meant to be graded by
  reissuing the oracle per-page with a sliced expectation; this oracle only
  grades one complete top-to-bottom answer per invocation.
- The exact contents of `unavailable` list items are left unchecked beyond
  "non-empty iff incomplete" — the contract doesn't fix segment-id shape.
- Whether "queried nodes" for `gaps` with no `node` filter really means
  *all* nodes (chosen here) or is otherwise scoped.
