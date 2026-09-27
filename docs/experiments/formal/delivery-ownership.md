# Stage 4 delivery ownership check

## Question and scope

Can an upstream sender receive an acknowledgement or discard its retryable copy while no durable receiver copy exists? This is a **safety** question about ordering and crash-surviving ownership. The [TLA+ model](../../../formal/delivery/DeliveryOwnership.tla) is a target protocol. Stage 5 added a [local commit](../../architecture/storage.md) but no network ACK or independent sender; this model check does not verify the Rust implementation.

## Model and assumptions

The [safe configuration](../../../formal/delivery/Safe.cfg) uses `Events = {e1, e2}`. Each event identity is distinct. Four sets track upstream copies, volatile receiver copies, durable receiver copies, and ACKs observed by the upstream. The allowed actions are `Receive`, `Commit`, `Acknowledge`, `Forget`, and `ReceiverCrash`; receive can repeat after a crash or a missing ACK. `Commit` is an atomic transition to crash-surviving storage, and durable copies are never lost in this model. `ReceiverCrash` clears only volatile copies. The upstream copy remains available until `Forget`; upstream crashes are not modeled. `Acknowledge` represents delivery of the ACK to the sender, so an unobserved ACK is a step that never occurs. No fairness or eventual progress is assumed.

The checked properties are `TypeOK`, `AckSafety` (`acked ⊆ durable`), and `RetainedOrDurable` (`Events ⊆ upstream ∪ durable`). Volatile memory is excluded from the second property because a receiver crash may erase it. The [early-ACK configuration](../../../formal/delivery/EarlyAck.cfg) and [lost-copy configuration](../../../formal/delivery/LostCopy.cfg) add an unsafe action that can ACK a buffered event before commit.

The model abstracts the current [EventBuffer](../../../src/buffer.rs) as one possible volatile receiver buffer. It does not model queue capacity, FIFO order, bytes, the local demo's printing, a separate upstream process, storage syscalls, or network packets. Stage 5 maps `Commit` to a local file sync; ACK transport remains unimplemented. Repeated receives of a committed ID have set semantics here; a general implementation needs stable identities and an explicit duplicate policy.

## Reproduction and tool identity

Method and properties were chosen before the checks. Run the [checker](../../../formal/delivery/check.sh) from the repository root with the pinned [TLA+ tools release](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.1):

```sh
TLA_JAR="$HOME/.cache/fabric_o11y/tla/tla2tools-v1.7.1.jar" JAVA_BIN="$HOME/.cache/fabric_o11y/tla/jdk-21.0.12.1+1-jre/bin/java" bash formal/delivery/check.sh
```

The recorded run used TLA+ tools release `v1.7.1` (`TLC2 Version 2.16`, rev `cdddf55`), Eclipse Temurin JRE `21.0.12.1+1`, Linux `x86_64` under WSL2, one TLC worker, fingerprint index `0`, and deadlock checking disabled because terminal states and stuttering are permitted. The checker uses a temporary metadata directory. Both downloaded tools were verified by SHA-256. The working tree included uncommitted project work; these hashes identify the checked inputs:

```text
694dc3b0c0f93872da50b41f9ff204d4079bf5e6eab84040d7912018d9c1d415  formal/delivery/DeliveryOwnership.tla
8101311396358b4b301a31a48ee26b95a130089e649135ce0ed7072175dbdc36  formal/delivery/Safe.cfg
67571341b893acb4e8fd28e9edebddebc880f592c5fc45119a03c481f3c01ec8  formal/delivery/EarlyAck.cfg
0d53659cab3fd5051005e4d601ecfd02508a437937355b6a55088cf75c516e08  formal/delivery/LostCopy.cfg
0add11314e47a478788b0f815d513db1c68fb769719e09c94dae21d25a21b3ff  formal/delivery/check.sh
d532ba31aafe17afba1130f92410d9257454ff7393d1eb2fe032f0c07f352da5  tla2tools-v1.7.1.jar
2413149700df0f7d440500a84a8f764c535f21e5a5e87d38328b64eec2c5b500  Temurin 21.0.12.1+1 JRE archive
```

## Results

The checker exited **0** after requiring these three outcomes:

| Configuration | TLC exit | Observation |
| --- | ---: | --- |
| `Safe.cfg` | 0 | No invariant violation; 225 states generated, 64 distinct reachable states, zero left on the queue; maximum search depth 11. |
| `EarlyAck.cfg` | 12 | `AckSafety` violated after receive and early ACK. |
| `LostCopy.cfg` | 12 | `RetainedOrDurable` violated after receive, early ACK, and sender forget. |

### Counterexamples

For event `e1`, the early-ACK trace was:

```text
initial:           upstream={e1,e2}  volatile={}    durable={}  acked={}
Receive(e1):       upstream={e1,e2}  volatile={e1}  durable={}  acked={}
EarlyAcknowledge:  upstream={e1,e2}  volatile={e1}  durable={}  acked={e1}
```

The last state violates `acked ⊆ durable`. With only `RetainedOrDurable` checked, TLC extended that trace by `Forget(e1)`:

```text
Forget(e1):        upstream={e2}     volatile={e1}  durable={}  acked={e1}
```

There is now no retryable upstream copy and no durable receiver copy of `e1`. A following receiver crash can erase the remaining volatile copy. This is why buffer acceptance cannot justify an ACK.

The checker itself was tested with a representative defect: in a temporary copy of the model, the safe `Acknowledge(e)` guard was changed from `e ∈ durable` to `e ∈ volatile`. Running the same `check.sh` exited **1** and printed `Invariant AckSafety is violated`, followed by `Safe model failed.` The repository model was not changed by that probe.

## Interpretation and limits

The safe invariants also have a short inductive argument: they hold initially; receive and crash leave the upstream copy in place; commit adds a durable copy; ACK is guarded by durability; and forget is guarded by an ACK that already implies durability. The TLC run exhausts the finite two-event model as written, subject to fingerprint collision risk; it is not a proof of arbitrary Rust executions. The model assumes that commit survives the crashes considered and that identity is stable. It checks no liveness, fsync behavior, filesystem recovery, network ACK loss rate, or throughput. [ADR-0005](../../decisions/ADR-0005-ack-after-durable-commit.md) records the design consequence; the [Stage 5 checks](delivery-rust-stage5.md) exercise the local implementation boundary separately.
