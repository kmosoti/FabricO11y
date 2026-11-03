# Typed investigations

Fabric investigations use the version 1 contract in `fabric::query`. The Rust
client, `fabric-query` command and `fabric-mcp` stdio server share that contract.
The receiver authenticates each credential and checks source scope before any
aggregation or evidence lookup. Queries do not accept SQL, file paths or commands.

Set `FABRIC_ENDPOINT` to the authenticated loopback receiver address and
`FABRIC_CREDENTIAL_FILE` to a private owned credential file with read access. Send a request on
standard input:

```sh
fabric-query query <<'JSON'
{"version":1,"query_id":"inspect-run","scope":["origin-a"],"snapshot":{},"filter":{},"operation":{"operation":"list_runs"},"cursor":null}
JSON
```

Use `fabric-query plan` with the same input to inspect conservative selected
segment and row counts. `fabric-query evidence ID` retrieves original canonical
JSON for an authorized source. `fabric-query progress` reads canonical source
frontiers. `fabric-query cancel QUERY_ID` requests cancellation. Start
`fabric-mcp` with the same environment to expose these operations over standard
MCP stdio. Standard output carries protocol results; diagnostics use stderr.

Operations include run listing, run summary, tool summary, service summary,
time-bucket activity and evidence. Filters accept a run key, resource, inclusive
start timestamp and exclusive end timestamp. All identifiers, timestamps, counts,
durations and source prefixes wider than 32 bits use decimal strings in JSON.
Duration sums remain exact beyond `u64`; means carry an exact numerator and
denominator. Missing durations yield null extrema, sum and mean. No cost or causal
relationship is inferred from observations.

Version 3 run keys use the global UUID carried between processes. Version 2 keys
include origin, producer, generation and local run name; matching local names do
not merge runs. Tool and service summaries describe observations, not implied
parent-child or causal relationships.

At the receiver, an empty snapshot captures the canonical source prefixes when
the first request arrives. The analytical projection waits for that fence within
the request deadline. Responses expose the canonical authority, chosen snapshot, requested source scope,
canonical frontier, analytical checkpoint, schema registries, transform version,
observed gap records, consumed budgets and truncation. Projection freshness must
be assessed by comparing its prefixes with canonical frontiers. Completeness is
relative to the stated source prefixes, not all events that may exist elsewhere.
A truncated page is explicitly incomplete. Subsequent pages carry a signed cursor
bound to the canonical authority, query, filters, scope, budgets and snapshot. Cursors expire after
15 minutes. Reuse the same request with the returned cursor; ingestion does not
change its snapshot. An unavailable historical snapshot fails explicitly.

The receiver permits one active analytical worker. Defaults are a 256 MiB
DataFusion execution pool, 512 MiB spill allowance, ten seconds, 100 returned
rows and 256 KiB output. Hard request maxima are 30 seconds, 1,000 rows, 1 MiB
output and one million conservatively selected rows. Pool and spill accounting
are separate from total process memory. Selected Parquet metadata estimates scan
work before execution. An exhausted scan or execution budget returns an error;
partial aggregate totals are never reported as complete. Cancellation retains
the worker reservation until execution cleanup finishes.
The single active query uses two runtime worker threads and targets two
DataFusion execution partitions sharing that pool. It permits at most eight
concurrent local object-store requests, including the initial per-file metadata lookup.
The projection supplies its fixed Arrow schema directly to the reader.

The rebuildable projection uses Arrow/Parquet 59 and DataFusion 55 outside the
SDK crate. Canonical source rows are decoded into fixed columns; original JSON is
retained unchanged. Files are partitioned by origin within each ingestion batch, with run-set and
time-range metadata for pruning. Within each file, rows are sorted by run and time
with bounded row groups and run-key Bloom filters. Apply coalesced canonical
batches to avoid small files. Each publication syncs immutable Parquet files and atomically replaces a
checksummed manifest containing the checkpoint and per-origin contiguous
prefixes. The manifest binds the projection to its canonical authority; reusing a
cache for a different authority requires rebuilding it. Reopening verifies every
retained segment checksum and refuses corrupt
or incompatible state. Interrupted files absent from the manifest are removed
on reopen. An uncertain publication requires reopening before further work.
Canonical history supplies replay for rebuilding the projection.

Observed messages and original JSON are untrusted data. Their contents never
change query permissions or execute instructions. MCP tools declare read-only
annotations; source authorization is enforced by Fabric, independently of those
annotations.

Projection state directories must be owned by the current user, private, and
ordinary directories rather than symlinks. Opening an existing public directory
fails without changing its permissions. Manifest, lock and retained segment paths
must be owned regular files; symlinks and special files are rejected before
opening them. This API assumes one trusted exclusive writer and no concurrent
hostile path replacement by that same directory owner.

Projection application accepts at most 8,192 canonical rows and 8 MiB of original
JSON per batch, checked before decoding or copying. The manifest is limited to
16 MiB and JSON depth 16. Reads check the regular file's size and storage quota
before allocating its buffer; streaming checksum and bounded serialization refuse
publication before exceeding that limit. A projection limit failure leaves
canonical custody intact and requires reducing the batch or rebuilding the
projection as appropriate. It never permits canonical payload reclamation on its
own.

Use `FABRIC_CREDENTIAL_FILE` for a private owned credential file instead of placing
its contents in MCP host configuration. `FABRIC_CREDENTIAL` remains supported;
setting both is rejected. The [MCP configuration example](../.codex/examples/fabric-mcp.toml)
uses the file option. The same settings configure the CLI. This file path is host
configuration and is not a tool argument.

For Greptime-backed investigations the receiver uses the same typed contract and
exact source fences. Native physical segment selection is unavailable, so
`selected_segments` is null. Native plans conservatively charge every row in the
scoped prefixes. The receiver allows one analytical request at a time; cancellation
is cooperative between bounded HTTP phases and keeps the reservation until the
active phase finishes. The default DataFusion pool and spill settings apply to the
SQLite projection; configure the engine's query pool separately within the same
receiver-group limit. See [canonical custody](canonical-custody.md).
