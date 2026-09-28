# Registered comparison: Fabric against Elasticsearch, ClickHouse and a commercial log platform

Status: registered on 2026-09-29 before any comparison measurement. Results go in a separate run record.

## Question

On the registered one-million-record history fixture, how does the Fabric Server's retained history compare with three established systems, each run as a single local node, on exact answers, query latency, load time, bytes at rest and memory?

This is not a like-for-like durability comparison. Fabric acknowledges a batch only after its own two-sync commit; the baselines are loaded in bulk after the fixture exists, with their default durability. The comparison is about answering the same questions over the same records.

## Systems

| System | Version | Source | License note |
| --- | --- | --- | --- |
| Fabric Server | the frozen qualification binaries (`e68d6ce`) | this repository | no license file declared |
| Elasticsearch | 9.5.4 (`elasticsearch-9.5.4-linux-x86_64.tar.gz`, SHA-512 verified) | artifacts.elastic.co | Elastic License 2.0 / AGPL / SSPL; benchmarking is not restricted |
| ClickHouse | 26.9.5.2 stable (`clickhouse-common-static-26.9.5.2-amd64.tgz`, SHA-512 verified) | packages.clickhouse.com | Apache-2.0 |
| A commercial log platform | withheld | vendor download, checksum verified | Its license restricts publishing named benchmark results. At the owner's direction, results are published only under this generic category; the product's name, version, adapter and unredacted results are kept outside this repository |

Each system runs on logical CPUs 0 to 3, bound to loopback, with the defaults below; the loader and query client run on the other CPUs. Systems run one at a time.

## Fixture

The Segment-mode history fixture for seed `0xA11FA001`: 1,000 identities for 250 s, 250,000 batches, about 500,000 log records with 512-byte bodies and 533,000 metric points. The records are read once from `server_dump --records` and decoded by the frozen [query oracle](../../../tools/qualification/QUERY_ORACLE.md) into log rows and metric points. The same rows are loaded into every baseline, so every system holds exactly the records Fabric retained.

## Loading and schema

- **Elasticsearch:** one index for logs and one for metrics, one shard, no replicas; `node` as `keyword`, `body` as the `wildcard` field type (exact substring matching), times as `long`; bulk requests of 5,000 rows, then one refresh.
- **ClickHouse:** `MergeTree` tables ordered by `(node, observed_ns)` and `(name, node, time_ns)`; `body` as `String`; inserts of 50,000 rows through the HTTP interface.
- **Commercial log platform:** logs and metrics as events in two indexes through its bulk ingestion interface, one event per row with the row's fields and its timestamp; nanosecond times compared as fixed-width strings, because its query language evaluates numbers as doubles.
- **Fabric:** already loaded by the fixture run; nothing more is done.

Load time runs from the first request to the moment the loaded row count is visible to a query. Bytes at rest are the system's data directory after loading.

## Queries

The history protocol's registered kinds, with the same seeded parameters, 20 instances each:

- **host logs:** logs from one node in a 60 s window, ordered by time, node identity, sequence and index, limit 1,000;
- **text search:** logs whose body contains a 12-character substring, all nodes, 60 s window, limit 100;
- **metric history:** one metric from one node over the whole fixture, limit 1,000;
- **fleet metrics:** one metric across all nodes in a 60 s window, limit 10,000.

The rate query is omitted: the fixture has no monotonic sums, so every rate answer is empty.

Every system answers with one request returning the first `limit` rows in the registered order: for Fabric, the first page. Latency is request start to that full answer. (Amended before any measurement: the first draft had Fabric follow every page while the baselines returned one page.)

## Exactness

For each instance, the returned rows' identities `(node_id, sequence, index)` and contents are compared with the oracle's expected rows. An answer that differs is reported as inexact, with its latency still shown; an inexact system is never ranked on speed for that query kind.

## Reported, not gated

Per system and query kind: p50 and p99 latency and the count of exact answers out of 20. Per system: load time, bytes at rest, and peak resident memory of the system's processes during the queries. Nothing is selected as a winner from one host.

## Limits

One host, one fixture, single-node defaults, and no tuning beyond the schema above. The baselines are general-purpose systems loaded in bulk; Fabric is purpose-built for this workload and was loaded through its durable delivery path. The commercial platform is reported only under its generic category.
