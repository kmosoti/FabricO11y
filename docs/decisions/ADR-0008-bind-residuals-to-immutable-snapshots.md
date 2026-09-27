# ADR-0008: bind residual answers to immutable snapshots

## Status

Experimental; the finite E3R correctness gate passed for selected candidate B. This does not select a performance strategy or add an application query service.

## Context

[E1R](../experiments/ablation/coverage-e1-run-01.md) authenticates a full block layout
and makes missing query work explicit. A retry also needs to preserve that work
across pages. EventId is not a unique physical row identity, and compaction can
reuse positions. Silently keeping the first conflicting answer would hide a
broken executor or a changed snapshot.

## Decision

Use the [registered residual API](../../tools/storage-probe/RESUME_API.md) in the
separate research package. Bind each page and request to the independently retained
anchor, exact query, tokenizer version and ordering version. An accumulator keeps
the full authenticated layout and one optional resolved result per block. It derives
outstanding work from that layout, even if a request omits some pending blocks.

A physical coordinate is a block ordinal and row offset within that bound snapshot.
Each emitted row includes a full-event digest. A repeated resolved block must have
the same entire ordered result, including empty results and all digests. Reject
changed, omitted or additional rows. Validate a whole page before changing state.

## Alternatives considered

- EventId or content-hash deduplication collapses distinct physical rows.
- A union of request residuals can lose work when a request omits a block. Keep the
  complete authenticated universe in the accumulator instead.
- First-value-wins hides conflicts and makes behavior depend on arrival order.
- Accepting changed predicates or snapshot roots turns retry into a different
  query; those must start a separate accumulator.

## Correctness argument and assumptions

Fix an immutable snapshot, trusted valid summaries and exact evaluator, an
independently retained anchor and collision-resistant hashing. A block state is
unresolved or a resolved ordered list. Joining unresolved with a list returns the
list; joining equal lists is idempotent; joining unequal lists is an error. For
compatible pages this operation is associative, commutative and idempotent.
Applying it independently to a complete authenticated partition gives a monotone
set of resolved blocks. Completion requires every block to resolve. Concatenating
resolved lists in ordinal order preserves all physical matches in arrival order.
Atomic validation prevents a conflict from resolving unrelated work as a side effect.

This is a conditional argument about the merge algebra. Authentication does not
prove the evaluator executed a scan. A dishonest initial Scanned result remains
outside the trust contract. No availability/liveness, raw-retention, wire parsing
or checkpoint durability property follows from it.

## Evidence and validation

The [E3R protocol](../experiments/ablation/resume-e3-protocol.md) fixes six snapshots,
768 query bindings, 1,000 masks per binding, retries, conflicting rows, successor
snapshots and permanent-unavailability cases. Independent tests were frozen before
implementation. The [run 01 result](../experiments/ablation/resume-e3-run-01.md)
records candidate B's full-corpus exit 0: 768,000 cases, 1,535,994 retries,
24,450 permanent-unavailable cases and zero contract violations. Three injected
defects failed their tests; independent GPT and Claude probes approved B. Original
candidate A/B full-package sessions had no recoverable final exits and are not
evidence of a pass. The main package adds only Serde wire derives and attributes to
B's Rust body, as checked in the run's integration artifact.

## Consequences

Retries can retain exact partial progress with explicit unresolved work. Keeping
resolved lists costs memory proportional to result size and retaining full metadata
costs memory proportional to block count. Receipt and merge costs still need
measurement. The separate S2 research prototype adds an external publication and
checkpoint lifecycle; this E3 decision alone establishes neither disk durability nor
raw retention.

## Related

- [Query research architecture](../architecture/query.md)
- [End-to-end completion contract](../experiments/ablation/end-to-end-prototype.md)
- [Coverage decision](ADR-0007-experiment-with-coverage-receipts.md)
