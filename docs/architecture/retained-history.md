# Retained history and query (phase 4 contract)

Status: proposed contract for phase 4, written before implementation so an independent exact-scan oracle can be built from it. Nothing here is implemented yet. The [alpha contract](../ALPHA.md#safety-contract) owns the gates.

## Data model

A **record** is one committed batch as the server's journal stores it: the credential label that submitted it (the node name), the server receive time in Unix nanoseconds, and the node's exact `Batch` bytes. From each record the query path derives:

- **log rows:** one per OTLP `LogRecord` in the batch's `logs` request, in request order. Fields: `node` (label), `node_id` (32 hex), `sequence`, `index` (position within the batch, from 0), `observed_ns` (`observed_time_unix_nano`), `body` (the string body), and `attributes` (string attributes of the record, as a map).
- **metric points:** one per `NumberDataPoint` in the batch's `metrics` request, in request order. Fields: `node`, `node_id`, `sequence`, `index`, `name`, `unit`, `kind` (`gauge` or `sum`), `monotonic` (sums only), `time_ns`, `start_ns` (sums only, else 0), `value` (a number: integer or double as stored), and `attributes` (string attributes of the point).
- **gaps:** each `collection_gaps` string of the batch, with `node`, `sequence` and the batch's receive time.

Resource attributes are not needed by the registered queries; `node` identifies the source.

## Storage

Sealed history is a set of immutable **segments**. Each segment is a directory holding a manifest written last (with a directory sync) and Zstd-compressed Parquet files: a `batches` table that retains every record exactly (label, receive time, node identity, sequence, SHA-256 and the exact batch bytes) and projected `logs` and `metrics` tables for queries. A segment covers a contiguous range of server journal groups; the journal files it covers are deleted only after its manifest is durable. A segment without a manifest is incomplete and removed at startup. Queries read sealed segments plus the unsealed journal tail.

**Retention** keeps at most 24 h and at most 20 GiB of sealed segments by default, set by the server keys `retention_s` and `retention_bytes`, and deletes whole segments, oldest first, when either limit is exceeded. Stream deduplication state does not depend on retained data: the server writes a stream checkpoint before it reclaims journal files. The **retained window** is `[oldest retained receive time, newest receive time]`.

## Queries

All times are Unix nanoseconds; ranges are half-open `[from_ns, to_ns)`.

1. **Log search** `{"kind":"logs","node":N?,"from_ns":A,"to_ns":B,"contains":S?,"limit":L,"page":P?}`: log rows with `observed_ns` in range, from node `N` if given, whose `body` contains the substring `S` (exact, case-sensitive, UTF-8) if given. Order: `observed_ns`, then `node_id`, `sequence`, `index`.
2. **Metric history** `{"kind":"metrics","node":N?,"name":M,"from_ns":A,"to_ns":B,"limit":L,"page":P?}`: metric points named `M` with `time_ns` in range, from node `N` if given. Order: `time_ns`, then `node_id`, `sequence`, `index`.
3. **Counter rate** `{"kind":"rate","node":N?,"name":M,"from_ns":A,"to_ns":B}`: for each series (node, name and attribute map) of a monotonic cumulative sum, over consecutive points in range ordered by `time_ns`: if both points share `start_ns` and the value did not decrease, one rate `(v2 - v1) / ((t2 - t1) / 1e9)` per second at `t2`; otherwise one **reset** marker at `t2` and no rate for that interval. Series order: node, then attributes as sorted `key=value` pairs; within a series, time order.

`limit` is 1 to 10,000. Log and metric answers are paginated; rate answers are not.

## Answer envelope

Every answer carries, besides its rows:

- `complete`: `true` only if every segment and journal file that could hold matching rows was read and verified. A missing or corrupt segment makes it `false` and is listed in `unavailable`. A missing or corrupt optional index never does; the query falls back to scanning.
- `retained_from_ns` and `retained_to_ns`: the retained window at the snapshot.
- `freshness`: per node that has any record, the newest `observed_ns` or `time_ns` retained, so a caller can tell a quiet node from a stale one.
- `gaps`: the collection gaps of the queried nodes whose batch receive time lies in the query range, with node and sequence.
- `snapshot`: an opaque token naming the segment set and journal end the answer was computed from.
- `next_page`: for paginated queries, an opaque token or `null`. A page token binds the snapshot: later pages read the same segments and journal end, even if new data arrived or retention deleted segments, and answer `410 Gone` if a bound segment is no longer available.

## Invariants

- Query execution never mutates stored telemetry.
- Row order is total, so pages never repeat or skip a row within one snapshot.
- Every log row and metric point returned can be traced to one retained record's exact bytes.
