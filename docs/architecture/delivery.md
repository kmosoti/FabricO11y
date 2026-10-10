# Delivery ownership

## Purpose and provenance

This page connects the original local delivery model to the network implementation described below. The default [Rust demo](../../src/main.rs) still prints volatile batches. The optional `write` path uses [EventLog](../../src/log.rs) to establish a local durable commit before printing `committed event N`. The separate Spindle/server path implements network delivery and ACKs.

The question is when a sender may discard its retryable copy. [ADR-0005](../decisions/ADR-0005-ack-after-durable-commit.md) chooses the rule: the sender may forget an event only after it receives an acknowledgement that follows a durable receiver commit. Here, successful `append(&Event)` is the receiver-side commit, and the CLI's commit line is a local observation after success.

## One-event path

<!-- diagram: ../diagrams/state-machines.mmd -->
```mermaid
stateDiagram-v2
    [*] --> Held
    state "Upstream retains copy" as Held
    state "Upstream + volatile receiver" as Buffered
    state "Upstream + durable receiver" as Committed
    state "ACK received; upstream still retains" as Acked
    state "Durable receiver only" as Released
    Held --> Buffered: receive
    Buffered --> Held: receiver crash
    Buffered --> Committed: durable commit
    Committed --> Acked: ACK received
    Acked --> Released: upstream forgets
```

The diagram shows the primary path for one event in the target protocol. The [TLA+ model](../../formal/delivery/DeliveryOwnership.tla) also allows a retry after a receiver crash or lost acknowledgement. In the model, an ACK is an acknowledgement *received by the sender*; a missing or lost ACK leaves the sender's copy in place. The local CLI has no network step corresponding to ACK delivery.

## Network delivery path

The HTTP batch route admits up to 16 requests before body extraction. Excess
requests receive the existing unavailable response with status 503 and
Retry-After 1. This pool is independent from the two query slots. Once extracted,
the request's exact bytes move into a Submission and the original HTTP body is
dropped before waiting for ACK. The downstream 64 MiB queue remains separately
bounded; admission is permission to use resources, while durable ACK transfers
custody. A cancelled reply receiver does not roll back a submitted commit. The
[transition investigation](../experiments/benchmarks/catalog-transition-ownership-findings.md)
records witnessed partial-body refusals and the limits of that evidence.

Implemented: `fabric-node run` (the Spindle) sends the oldest unacknowledged Spool batch, as its exact stored bytes, to the [Fabric Server](../../crates/fabric-server/src/lib.rs) over HTTPS with a bearer token. The server's single commit thread applies the [ADR-0013](../decisions/ADR-0013-deliver-batches-in-order-with-bounded-dedup.md) rule, appends new batches to its own [frame log](../../crates/fabric-frame/src/frame.rs) as one grouped frame (50 ms or 1 MiB), and answers only after that frame's data and marker syncs. The Spindle then writes its ACK cursor by synced rename and may delete sealed Spool files at or below it. A lost ACK is a retry of the same identity and bytes, which the server acknowledges again without a second record. The same Strand and sequence with different bytes is refused and never replaces the committed batch. A credential label binds to one Spindle identity on its first commit.

The commit path is split by layer ([ADR-0015](../decisions/ADR-0015-adopt-a-hexagonal-architecture.md)):

| Layer | Code | Responsibility |
| --- | --- | --- |
| core | [`fabric_core::delivery`](../../crates/fabric-core/src/delivery.rs) | `decide_delivery` maps committed Strand state, the incoming Batch and the binding state to `Commit`, `Duplicate`, `Stale`, `Conflict`, `Gap` or `Forbidden`; `GroupPlan` applies it in order within one commit group |
| ports | [`DurableJournal`, `Clock`](../../crates/fabric-ports/src/lib.rs) | what the use case needs from storage and time |
| app | [`commit_group`](../../crates/fabric-app/src/delivery.rs) | decide every offer, commit accepted offers once, and turn every non-forbidden answer into `Unavailable` if nothing became durable |
| adapter / composition root | [`Store`](../../crates/fabric-server/src/store.rs) | SHA-256 of the exact bytes, the frame append and syncs, replayed Strand and binding state, the system clock, the HTTP answer |

The extraction is guarded by an exhaustive differential test against a frozen transcription of the base decision loop ([crates/fabric-app/tests/delivery.rs](../../crates/fabric-app/tests/delivery.rs)). One counterexample was kept: at the base, `last + 1` overflowed for a Strand at `u64::MAX`; the kernel uses `next_sequence`, so an exhausted Strand accepts no successor.

Within `GroupPlan`, the first credential/Spindle binding stays inline. Additional
distinct reverse bindings use a compact list until there are 32 total; the 33rd
promotes the list into credential and Spindle indexes. Repeated accepted Batches
do not add another binding. The first staged forward mapping and every accepted
reverse mapping survive promotion, including when supplied durable facts change;
explicit durable credential facts still take precedence. These are ephemeral
group decisions; persistence and ACK ownership remain in the application and
adapter layers. The [algorithm comparison](../experiments/benchmarks/catalog-algorithm-round-findings.md)
records why indexing every small group was rejected and the compact prefix was
retained. The 32-binding crossover is a measured local choice, not a universal
optimum.

[Fault runs](../experiments/formal/alpha-phase2-delivery-faults.md) graded by the [delivery oracle](../../tools/qualification/DELIVERY_ORACLE.md) pass for three Spindles under server kills, Spindle kills and an outage; the [ten-process run](../experiments/benchmarks/alpha-phase2-delivery-run-01.md) measured ACK latency in both commit modes. Those records belong to the revisions they name.

## Model-to-code mapping

| Model action | Current Rust counterpart | Limit |
| --- | --- | --- |
| `Receive` | `EventBuffer::try_push` places an owned event in volatile memory | The generator and buffer share one process. |
| `Commit` | `EventLog::append(&Event)` syncs an event frame, then writes and syncs its commit marker | Assumes a filesystem that honors successful sync and a synced parent directory entry. |
| `Acknowledge` | `write` prints `committed event N` only after append succeeds | This is local output, not a transmitted ACK. |
| `Forget` | The command drops the event after successful append | On error it exits; manual rerun reconstructs the synthetic source from the same seed/count. |
| `ReceiverCrash` | Separate `write` and `replay` process invocations, plus simulated torn tails in tests | Abrupt power loss is not exercised. |

The CLI compares recovered events against the full deterministic generator prefix. That gives this particular source a retry path after ordinary process termination, provided no storage I/O error occurred. A failed marker sync can leave a visible marker without a reliable durable copy; this file cannot reveal the earlier error on reopen. After such an error, rebuild from an independent source on healthy storage. Arbitrary senders still need stable identity, retained upstream data, and a duplicate policy.

## Safety boundary

The two checked invariants are:

1. Every acknowledged event is in the durable set.
2. Every modeled event is still held upstream or is in the durable set.

An in-memory queue does not count as durable. For this local log, successful file sync establishes the implementation's commit boundary under its filesystem assumptions, including durable ancestor path names and no unresolved storage sync error. The model assumes stable distinct event identities, a retry path, and a durable record that is not lost after commit. The synthetic CLI supplies a repeatable sequence only when invoked again with the same settings and locked generator version; `EventId` does not establish global uniqueness.

## Failure behavior and limits

Before commit, a receiver crash clears its volatile copy while the sender retains its copy for retry. After commit, a crash leaves the durable copy. If an ACK is lost, the sender may retry; a general implementation must decide how duplicate deliveries are recognized. Sender crashes and loss of the upstream copy are outside this model; it assumes that copy remains available until `Forget`. No fairness assumption forces an ACK to arrive, so the model makes no eventual-progress claim.

The [formal investigation](../experiments/formal/delivery-ownership.md) records the finite TLC check and an early-ACK counterexample. It does not verify Rust, fsync, filesystem recovery, network behavior, or a byte-level memory bound. The [Stage 5 implementation checks](../experiments/formal/delivery-rust-stage5.md) exercise restart and torn-tail recovery under a live operating system; they still cannot prove physical power-loss durability. See the [storage view](storage.md) for the format and failure assumptions.

The [Homa/SIRD transport study](../experiments/ablation/receiver-driven-transport.md) investigates receiver GRANT/CREDIT scheduling for a future network boundary. Its [network-limited H1](../experiments/ablation/receiver-credit-h1-run-01.md) and [M2 prefix](../experiments/ablation/unscheduled-prefix-m2-run-01.md) simulations, plus the [finite transport model](../experiments/formal/transport-credit-ownership.md), keep CREDIT separate from the modeled post-commit durable ACK. Those results do not implement a network sender or prove durability in this Rust application.
