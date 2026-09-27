# Finite transport credit and ownership check

## Question and model

Can a receiver grant bounded permission to send while a producer keeps its event until a post-commit durable ACK, including a lost-ACK retry of the same identity? The small [TLA+ model](../../../formal/transport/README.md) explores this safety question before a runnable [H1 packet-slot ablation](../ablation/receiver-credit-h1-run-01.md). CREDIT is permission for one modeled scheduled packet; it is never itself a durable ACK.

The model has two producers, one `(producer, sequence)` identity each, one whole packet per identity, two bytes per message, a two-byte receiver-admission cap, and a global budget of two outstanding credits plus scheduled packets in flight. `Commit` creates the pending ACK only after admission. `DeliverAck` lets the producer forget its retained copy. A lost ACK allows one retry with the same identity; the receiver responds to a duplicate committed identity without committing it twice. `Reject` is observable when admission would exceed its cap. The model permits arbitrary stuttering and proves no liveness property.

## Checked properties and counterexamples

`Safe.cfg` checks type bounds, credit conservation, admission bounds, explicit rejection, source retention until ACK, commit-before-ACK, single commit per identity, and dedup response only for a committed identity. `EarlyAck.cfg` adds an intentionally broken action that creates an ACK before commit. `OverGrant.cfg` adds an intentionally broken action that exceeds the credit budget. `RetryWitness.cfg` asks TLC to refute an observer that says no dedup response can occur; its counterexample is a reachability witness, not a safety failure.

Executed from the repository root on 2026-09-26:

```sh
TLA_JAR=/home/kmosoti/.cache/fabric_o11y/tla/tla2tools-v1.7.1.jar \
  JAVA_BIN=/home/kmosoti/.cache/fabric_o11y/tla/jdk-21.0.12.1+1-jre/bin/java \
  bash formal/transport/check.sh
```

The wrapper exited **0**. TLC's safe configuration exited **0** after 130 distinct states. The expected EarlyAck, OverGrant, and retry-witness configurations each exited **12** for their designated invariant: `AckSafety`, `CreditBound`, and `NoDedupResponse`, respectively. The OverGrant trace reaches two outstanding credits plus one in-flight packet against a budget of two. This injected defect demonstrates that the `CreditBound` checker can fail. Exact configurations and checks are in [formal/transport](../../../formal/transport/README.md).

## Mapping and limits

The model's `retained` set maps to a producer's retained message copy, `credits` and `inFlight` to scheduling permission and data in transit, `admitted` to finite receiver memory, `committed` to the receiver's authoritative stored identity, and `acked` to a producer-observed durable acknowledgement. The [Stage 5 local log](../../architecture/storage.md) implements a local commit boundary, but neither the model nor the H1 simulator executes that Rust log. The simulator uses multiple packets per 64 KiB message and one receiver-credit budget of nine packets; the TLA+ model deliberately uses whole-packet identities and a budget of two. Passing TLC therefore does not prove the Rust simulator's accounting, real-network retry/loss handling, physical durability, or unbounded behavior. Those need separate executable probes and eventually real-host validation.

The model leaves grant allocation, credit expiry, admission-byte release, repeated loss, concurrent duplicate packets, and fairness unspecified. The [model README](../../../formal/transport/README.md) lists these open questions. No formal result here selects Homa, SIRD, or a Fabric transport.
