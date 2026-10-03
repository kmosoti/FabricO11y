# Scaling design: from this machine to more resources

Status: **design, derived from measurements**; nothing here is implemented unless it says so. It uses the constants measured on one 4-CPU container in [ingest run 01](../experiments/benchmarks/ingest-run-01.md), [spindle run 01](../experiments/benchmarks/spindle-run-01.md), [query walk run 01](../experiments/benchmarks/query-walk-run-01.md), [text filter run 01](../experiments/benchmarks/text-filter-run-01.md), [block tail run 01](../experiments/benchmarks/block-tail-run-01.md) and [storage layout run 01](../experiments/benchmarks/storage-layout-run-01.md), and asks what each component needs for a target such as 500 hosts sending 500 MB/s in all.

## The measured constants

| Component | Constant on this machine | Bound by |
| --- | --- | --- |
| Spindle collection | about 31 MB/s of log text on a fraction of a core | file reads and encoding; not limiting |
| Spindle delivery | about 3.6 MB/s of text, 13 Batches/s, 77 ms per Batch | one Batch in flight per Strand and the server's 50 ms group window |
| Spindle encoding | 2.7 bytes of Batch per byte of text on 130-byte lines | per-line attributes (path, device, inode, offsets) |
| Server ingest with sealing | 48, 75, 81 MB/s at 1, 2, 3 sealing workers on 3 CPUs | sealing CPU; about 27 MB/s per core |
| Sealer memory | about 5.5 times a 64 MiB journal file per concurrent build | today's in-memory build ([ADR-0022](../decisions/ADR-0022-build-segments-by-external-merge-sort.md) flattens it to about 45 MiB) |
| Segment bytes | about 0.41 of the journal on real text | the custody copy and both projections ([storage direction](storage-direction.md)) |
| Selective queries | 10 to 50 ms whatever the retained volume (walk plan) | one source's overshoot |
| Text search with no match | under 5 ms on 64 Segments with filters; 28 ms on a 64 MiB tail with blocks | filter reads, one per row group or block |

## One host

A host sending 1 MB/s of text is inside today's per-node delivery ceiling (3.6 MB/s) on 15% of a core. The levers for a busier host, in order of cost:

1. **Fewer encoded bytes per line** (no protocol change): carry the file path and device once per Batch as a scope attribute and only the offsets per line. On 130-byte lines that cuts the Batch about in half, so the same Batch rate carries about twice the text. It changes the attributes a log row carries, so it is a [product contract](../PRODUCT-CONTRACT.md) fidelity question first.
2. **A shorter commit wait**: the server holds every group open 50 ms to share its two syncs among Strands. Closing a group as soon as no other submission is waiting, or after at most 50 ms, keeps the sharing under load and lets a lone Strand commit in a few milliseconds. It changes when groups close, not their sync order, but it is the commit path ([ADR-0005](../decisions/ADR-0005-ack-after-durable-commit.md)) and needs its own decision and fault runs.
3. **More than one Batch in flight per Strand**: a window of N Batches multiplies the per-node ceiling by about N. It changes the delivery rule ([ADR-0013](../decisions/ADR-0013-deliver-batches-in-order-with-bounded-dedup.md)), its oracle and its model, and is the most expensive lever.

Collection itself needs nothing until well past 30 MB/s per host.

## One server

At 27 MB/s per sealing core, 500 MB/s needs about 19 cores of sealing, plus commit, TLS and queries: about 24 to 32 cores on one server. Before that is reasonable:

1. **Flat-memory sealing** ([ADR-0022](../decisions/ADR-0022-build-segments-by-external-merge-sort.md)): with 45 MiB per build, 16 workers need under 1 GiB where today they would need 5.6 GiB.
2. **Cheaper sealing per byte**: the build decodes every OTLP Batch and writes three tables and the custody copy. Re-blocking a canonical record stream ([storage direction](storage-direction.md), hypothesis D7) instead of re-encoding, and keeping one canonical copy (D3), remove most of that work and about 60% of the Segment bytes.
3. **The disk**: 500 MB/s of journal writes and about 200 MB/s of Segment writes sustained, 17 TiB of Segments a day, call for NVMe and a retention budget sized in days, not the 20 GiB default.

## Several servers

When one server's cores or disk are not enough, shard by Strand: each Spindle's credential maps to one server (by enrollment), every Strand's custody, deduplication and ordering stay on that server, and nothing in delivery changes. A query fans out to every server and merges:

- `limit k` queries take the k smallest keys of the per-server answers; each server stops early with the walk, and the merge is the same threshold rule one level up (a server whose smallest key is above the merged heap's threshold contributes nothing more);
- page tokens carry one snapshot per server;
- completeness is the conjunction of the servers' answers, and freshness the union of their maps.

The query specification in the core (`fabric_core::query::spec`) already states the merge's correctness: a threshold walk over sources with sound bounds equals the definition (theorem T7), and a server's answer is such a source.

## The 500-host target, line by line

| Need | Today on this machine | What reaches it |
| --- | --- | --- |
| 1 MB/s from each of 500 hosts | 3.6 MB/s per host ceiling | nothing; inside the ceiling |
| 500 MB/s of server ingest | 81 MB/s on 3 cores | about 24 to 32 cores with flat-memory sealing, or 4 to 6 servers of 8 cores sharded by Strand; less with cheaper sealing |
| 24 h of retention | 20 GiB default | about 17 TiB of NVMe (about 7 TiB with one canonical copy) |
| Interactive queries | 10 to 50 ms for selective shapes; seconds for an hour-wide no-match search at this volume | the walk, the filters and the block tail as built; per-server fan-out keeps each server's share fixed |

## What to measure next

1. The adaptive group window on this machine (one decision, fault runs, the history protocol).
2. Ingest with the flat-memory sealer at 1 to 8 workers on a larger host.
3. Two servers sharded by Strand with a fan-out query, against one server, on the registered workload.
