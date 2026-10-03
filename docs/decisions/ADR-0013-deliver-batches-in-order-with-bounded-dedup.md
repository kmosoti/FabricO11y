# ADR-0013: Deliver batches in order with bounded per-stream deduplication

## Status

Accepted on 2026-09-27 for alpha phase 2 (plan decisions D4 and D5). Implementation and its checks are recorded in the phase ledger.

## Context

The [alpha safety contract](../PRODUCT-CONTRACT.md#custody-and-safety-contract) requires: identity and bytes durable before first send; retries reuse both; the server syncs data and dedup state before ACK; equal identity with equal bytes is one logical commit; equal identity with different bytes is an error; equal content at another sequence is distinct. [ADR-0005](ADR-0005-ack-after-durable-commit.md) says the sender forgets only after an ACK that follows a durable commit. The server must hold bounded state for up to 1,000 node identities.

## Decision

A stream is `(node_id, generation)`. The node sends at most one batch per stream at a time, oldest unacknowledged first, as the exact bytes stored in its spool. The server's commit thread keeps, per stream, the last committed sequence and the SHA-256 of its bytes, and rebuilds both by replaying its own journal. Its answer to sequence `s` with last committed `l`:

| Case | Answer |
| --- | --- |
| `s = l + 1` | commit, then `ack`, `committed_through = s` |
| `s = l` and equal hash | `ack`, `committed_through = l` (lost-ACK retry) |
| `s = l` and different hash | `conflict`; nothing is replaced |
| `s < l` | `ack`, `committed_through = l` |
| `s > l + 1` | `gap`, `committed_through = l`; the node resends from `l + 1` |
| bad credential, malformed, oversize, commit queue full | unauthorized, bad request, too large, unavailable; nothing committed |

An `ack` is sent only after the group containing that batch, or the earlier batch it acknowledges, has completed data sync and marker sync. The node persists its ACK cursor by synced rename and deletes a closed spool file only when every batch in it is at or below the cursor. A `conflict` or `gap` stops delivery for that attempt and is reported on the node's standard error; the node retries with backoff, so an unresolved conflict stays visible and nothing on either side is overwritten.

The server groups commits: one frame holds up to 1 MiB of batches or whatever arrived within 50 ms, followed by one data sync and one marker sync. The individual mode is a group of one, so both modes have identical durability semantics.

Amended 2026-10-03 (quiet rule): a group also closes when no submission has arrived for 2 ms. With one Batch in flight per Strand a lone sender could never bring a second submission while its first was held, so every one of its Batches paid the whole window; under load arrivals are closer than 2 ms and groups still fill to 1 MiB or the window. Durability is unchanged: a group is synced before any of its answers is sent. Tested by `a_lone_submission_is_not_held_for_the_window` and its negative control in `store.rs`; measured in [sealer profile run 01](../experiments/benchmarks/sealer-profile-run-01.md).

## Alternatives considered

- A full set of committed sequences per stream. It permits out-of-order sending but grows without bound or needs its own compaction.
- Content-hash deduplication. It would merge equal payloads at different sequences, which the contract forbids.
- Several batches in flight per stream. More throughput per node, but the registered rate is 2 logs/s per node and the rule above would need a window.

## Evidence

The rule keeps O(1) state per stream. A model check and the independent delivery oracle are the planned evidence; see the phase ledger for results.

## Consequences

One in-flight batch per stream bounds node throughput by round-trip time, far above the registered rate. A lost sequence on the server side is visible as a `gap` answer rather than silent. Deleting whole spool files keeps reclaim simple but retains up to one file of acknowledged data.

## Validation

Falsify by an ACKed batch missing after server restart, a duplicate logical record after a retry, a replaced payload after a conflict, or a node that forgets an unacknowledged batch. The delivery oracle's mutation controls must fail on each.

## Related

[ADR-0012](ADR-0012-add-a-server-crate-in-a-workspace.md), [delivery view](../architecture/delivery.md), [completion plan](../ROADMAP.md#work-carried-from-the-completion-plan).
