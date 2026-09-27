# ADR-0005: Acknowledge only after durable commit

## Status

Accepted as a delivery contract. Stage 5 implements the local receiver commit boundary; a network ACK and general sender protocol remain unimplemented.

## Context

The buffer can accept an `Event`, but acceptance only transfers in-process ownership to volatile memory. A receiver crash can erase that copy. The sender needs an explicit point at which it may discard its retryable copy without creating a state in which no recoverable copy remains.

## Decision

The receiver may issue an acknowledgement for an event only after a durable commit of that event has completed. The sender may forget its copy only after it receives the acknowledgement. A rejected or merely buffered event remains the sender's responsibility. The local [EventLog](../../src/log.rs) defines commit as a synced event frame followed by a synced commit marker; the CLI prints a commit line afterward. This does not define ACK transport or stable identity for arbitrary senders.

## Alternatives considered and argument

- **Acknowledge on volatile buffer acceptance:** A receive, early ACK, and sender forget leave the only copy in receiver memory. A receiver crash then loses it. The [TLC counterexample](../experiments/formal/delivery-ownership.md#counterexamples) reaches a state with no upstream or durable copy even before that crash.
- **Acknowledge after durable commit:** Initially every event is held upstream. Receive and crash do not remove the upstream copy. Commit adds a durable copy. Acknowledge is guarded by the durable copy. Forget is guarded by the acknowledgement, so removing the upstream copy leaves the durable one. Induction over these actions preserves both invariants, provided the modeled commit really survives the specified crash.
- **Never acknowledge:** The sender retains a retryable copy, which preserves safety under the model but does not permit a completed handoff or bounded upstream retention. It does not meet the desired lifecycle.

## Evidence

The [delivery model](../../formal/delivery/README.md) checks two symbolic events with receive, commit, acknowledgement, forget, retry, and receiver crash actions. [TLC results](../experiments/formal/delivery-ownership.md) cover all 64 reachable states in the safe configuration and find early-ACK counterexamples. This checks the model's finite state space, not the future Rust, filesystem, or network implementation.

## Consequences

Buffer acceptance cannot be exposed as a durability acknowledgement. The local log provides a commit operation whose process-restart behavior can be tested; the sender must still keep data until it observes an ACK. Lost ACKs can cause retries; a general receiver needs stable identity and a duplicate policy. Waiting for a sync may add latency; Stage 6 should measure it.

## Validation

Run the [TLC checker](../../formal/delivery/README.md#run-tlc) and the [Stage 5 implementation checks](../experiments/formal/delivery-rust-stage5.md). The latter map receive, commit, local observation, restart, and replay to the model. Network ACK delivery and an independent sender are still absent. A trace in which a sender has forgotten an event that cannot be recovered would falsify this contract under its filesystem assumptions.

## Related

- [Delivery architecture](../architecture/delivery.md)
- [Current ingestion boundary](../architecture/ingestion.md)
- [Local storage boundary](../architecture/storage.md)
- [ADR-0002: Full-buffer rejection](ADR-0002-reject-full-buffer.md)
