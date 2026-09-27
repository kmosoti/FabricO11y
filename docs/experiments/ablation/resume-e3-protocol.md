# E3R: snapshot-bound residual answers

Status: registered before implementation and candidate evaluation. This is the next
cell in the [end-to-end completion contract](end-to-end-prototype.md), following
[E1R](coverage-e1-run-01.md). The [fixed API](../../../tools/storage-probe/RESUME_API.md)
defines the Rust boundary and exact expected behavior.

## Claim

Under E1's trusted builder, independently retained immutable anchor, collision-resistant
hashing and trusted exact evaluator assumptions, compatible partial answers compose
to the exact own-snapshot positional full-scan result when every block resolves.
Retries are idempotent. Changed bindings or conflicting per-block results fail
atomically. Distinct physical positions are not deduplicated by EventId or row hash.

Rust `Option<Vec<MatchedRow>>` or an equivalent private enum can distinguish unresolved
work from a resolved empty result. Full snapshot/query/version identity travels
with pages and residual requests. The client derives outstanding work from the
authenticated complete layout, not from whatever subset a retry happened to return.

## Fixed corpus

- S1-shaped mixed and shuffled-Log corpora; seeds 201, 202, 203; 2,048 events;
  64-row blocks; 10% duplicate EventIds without collapsing physical positions.
- 128 queries per snapshot, fixed PRNG seed 9 and the S1 eight-predicate cycle.
- 1,000 availability masks per snapshot (PRNG seed 7), exercised for every query;
  retry each page 1–3 times in a deterministic permuted order.
- 50 successor snapshots per original snapshot with position reuse/compaction;
  replay predecessor residuals against these successors and require rejection.
- In 5% of masks keep a candidate block permanently unavailable/corrupt. Such
  blocks cannot resolve or disappear merely because another page arrived.
- For every snapshot/query, inject a conflicting value at a populated physical
  coordinate where one exists, changed predicate/tokenizer/order binding, and
  changed block digest. Use explicit small fixtures where a query has no matches.

The oracle author pins the corpus and independent scalar full scan before candidate
implementation. It may construct page fixtures from a precomputed exact full answer
to isolate the merge algebra; the execution/resume adapters also receive independent
end-to-end in-memory tests. The full mask/query product must still execute; fixture
caching does not remove test cases. Use release mode for this larger finite corpus.

## Gate and non-vacuity

Metric: contract violations, threshold **0**. Check exact positions and row digests,
closure status, ascending unresolved ordinals, conflict rejection, rejection without
partial state mutation and all clean controls. Invalid input is an error, not an
empty complete answer. Empty datasets, duplicate identical events, out-of-range
coordinates and malformed availability/residuals have additional small tests.

The checker must fail for at least these injected defects: omit snapshot binding;
silently keep the first value on a conflicting retry; deduplicate by EventId or full
row digest. Preserve counterexample inputs and real failing exits. A compatible
merge is a per-block join: unresolved yields to a verified resolved list, equal
lists are idempotent, unequal resolved lists conflict. This gives order independence
and monotone resolution under the stated assumptions; tests cover finite instances,
not arbitrary execution or liveness.

No latency benefit, disk recovery, data-retention guarantee or malicious-executor
proof follows from this cell. Those limitations remain explicit in the final
end-to-end audit. Freeze performance workloads separately before measuring cost.
