# ADR-0024: Answer history queries by a walk over source bounds

## Status

Part 1 accepted by the repository owner on 2026-10-02 and implemented in the server as the opt-in plan `query_plan=walk` ([query.rs](../../crates/fabric-server/src/query.rs), [tail.rs](../../crates/fabric-server/src/tail.rs); [query walk run 01](../experiments/benchmarks/query-walk-run-01.md)); the default stays `scan` until gate 1's protocol re-run and soak pass. Parts 2 and 3 accepted for implementation by the owner on the same day, each under its gates. Proposed on 2026-10-02. Every mechanism below first existed as a research prototype behind an environment switch in [tools/research](../../tools/research/README.md) and is measured in [threshold run 01](../experiments/benchmarks/topk-run-01.md), [budget run 01](../experiments/benchmarks/budget-run-01.md), [real-corpus query run 01](../experiments/benchmarks/real-corpus-query-run-01.md) and [optimality run 01](../experiments/benchmarks/optimality-run-01.md). Nothing here is product behavior. Accepting part 1 changes query code only; part 2 is a [product contract](../PRODUCT-CONTRACT.md) change (an index beyond row-group statistics, which [ADR-0020](ADR-0020-store-sealed-history-as-parquet-segments.md) rules out); part 3 depends on [ADR-0023](ADR-0023-define-an-observation-record-with-a-canonical-encoding.md) and is a wire- or persisted-format change under the [scope rule](../../AGENTS.md#scope-rule). Each part is accepted, rejected or deferred on its own.

## Context

The stock query path reads every source that overlaps the window: every unsealed journal entry at 4.7 µs each, and every Segment row group, fully materialised. On a 64 MiB real-text tail every query costs 570 to 770 ms whatever it asks; on 64 Segments of real text, 100 to 150 ms. The [optimality bounds](../research/optimality-bounds.md) state each registered shape's floor (the data any algorithm must read) and the [executable query specification](../formal/query-semantics.md) states the answer every implementation must return, with theorems T1 to T8 as properties.

## Decision

**Part 1: the walk (query code only).** Order every source (tail entry or row group) by the lowest key it can hold, offer its rows to the bounded heap of `limit + 1` keys, and stop when the heap's largest key is below the next source's lower bound (the threshold algorithm, `threshold_walk` in the specification). Keep a 64-frame decode cache for tail entries, a per-process cache of Segment manifests and row-group bounds (a named Segment is immutable; the directory is listed per query), and the key index of the tail. Optionally bound a request by rows examined and name the boundary key (`budgeted_walk`, ledger L-05).

**Part 2: a trigram filter per row group (a contract change).** Write, at seal time, a 2^16-bit bloom over the byte trigrams of each row group's bodies into the Segment, and skip groups whose filter lacks a needle's trigram.

**Part 3: a canonical block tail (depends on ADR-0023).** Hold the unsealed tail as FOB1 blocks of at least about 2,600 records, each a walk source with its key bounds, read through `decode_view`; with part 2, a filter per block.

## Evidence

| Shape, real text | stock | part 1 | parts 1 and 2 | parts 1, 2 and 3 |
| --- | ---: | ---: | ---: | ---: |
| tail, logs `limit 50` | 626 ms | 39 | | 55 |
| tail, text with no match | 614 | 567 | | 48 |
| tail, empty window | 608 | 12 | | 13 |
| Segments, logs `limit 50` | 101 to 141 | 9 to 10 | | |
| Segments, text with no match | 103 to 146 | 114 to 126 | 7 to 10 | |
| Segments, empty window | 7 to 10 | 4 to 6 | | |

Answers: 0 mismatches against the stock server in every differential of the runs cited (several thousand random queries with pages), and the specification's theorems hold as properties. The stock server is the comparison, not the judge: the specification is.

## Gates for acceptance

1. Part 1: the walk implemented against `threshold_walk` and `budgeted_walk` with HIST-1/2 equivalence to the specification on the registered workloads; the history protocol re-run with the walk; the registered soak passing (it failed on sealer memory, [soak run 01](../experiments/benchmarks/soak-run-01.md)), since the caches hold memory per process.
2. Part 2: a product-contract amendment in its own commit with its reason; the filter's false-positive rate and bytes recorded per Segment; a negative control in which a corrupted filter drops a matching row and the open-time check refuses the Segment.
3. Part 3: ADR-0023 accepted for the wire or the journal; a block durability and replay design; A3's registered statistic met through the server, which blocks alone do not meet (the no-match search is 0.26 to 0.28 of stock; 0.08 with part 2).

## Consequences

Selective shapes cost their answer instead of the window: the walk reads one source's overshoot past the floor. The shapes that cannot stop (a rate, a search with no match) keep the scan's cost under part 1 and lose it only with parts 2 and 3. The caches trade a few MiB per process for the per-query metadata reads. Under total overlap (every source can hold the first keys) the walk reads everything, which is that shape's floor too.

## Alternatives considered

- **An inverted index** (Quickwit, Elasticsearch, ClickHouse's `tokenbf_v1` is the nearest relative): reaches the same floor for absent and rare tokens on real streams at two orders of magnitude more bytes than the filter; the filter needs no tokenizer and composes with the walk's key order.
- **File-order scanning when nothing stops** (A4): removes frame re-decoding but forgoes the early stop; the frame cache keeps both.
- **A node-presence set per row group** (ledger L-07): skips a fraction e^(−g/N) of g-row groups, nothing at 100 nodes and 8,192 rows (measured), 44 % at 10,000 nodes; deferred to fleet scale.
- **A sorted table of Segment bounds**: adds nothing measurable at 64 Segments once metadata is cached.
